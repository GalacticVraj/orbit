//! Orbit host: single instance, Alt+Q hotkey, tray icon, CLI, and action dispatch.
//! Idle state = one hidden window blocked in GetMessageW: 0% CPU, no timers, no polling.
#![windows_subsystem = "windows"]

use orbit_core::{Bitmap, Outcome};
use orbit_ring::Task;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("Orbit.Host");
const WM_TRAY: u32 = WM_APP + 1;
const HOTKEY: i32 = 1;
const TIMER_COLLECT: usize = 1;
const COPY_RING: usize = 0x4F52; // "OR"
const ID_PLUGINS: usize = 10;
const ID_AUTOSTART: usize = 11;
const ID_QUIT: usize = 12;

thread_local! {
    /// Paths forwarded by `--ring` processes (one per file on multi-select), batched for 250 ms.
    static PENDING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static TASKBAR_CREATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    match args.first().map(String::as_str) {
        Some("--install") => exit_with(install().map(|exe| spawn(&exe, "--welcome"))),
        Some("--uninstall") => {
            close_running();
            orbit_shell::unregister_context_menu();
            let r = orbit_setup::uninstall();
            if r.is_ok() {
                message("Orbit has been removed.", MB_ICONINFORMATION);
            }
            exit_with(r)
        }
        Some("--install-plugin") => {
            let r = match args.get(1) {
                Some(p) => orbit_plugins::install(Path::new(p)),
                None => Err("Usage: orbit --install-plugin <folder-or-zip>".into()),
            };
            match &r {
                Ok(name) => message(&format!("Plugin “{name}” installed."), MB_ICONINFORMATION),
                Err(e) => message(e, MB_ICONERROR),
            };
            std::process::exit(r.is_err() as i32)
        }
        Some("--quit") => close_running(),
        Some("--ring") => {
            let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
            if !forward(&paths) {
                run(paths, false);
            }
        }
        Some("--welcome") => run(Vec::new(), true),
        Some("--portable") => run(Vec::new(), false),
        _ => {
            if forward(&[]) {
                return; // already running: the host shows a "still here" balloon
            }
            if !orbit_setup::is_installed_copy() {
                let ask = "Install Orbit for your account?\n\nIt starts with Windows and lives in the tray. \
                           Select files in Explorer and press Alt+Q.\n\nNo = just run it this time.";
                if message(ask, MB_YESNO | MB_ICONQUESTION) == IDYES {
                    match install() {
                        Ok(exe) => return spawn(&exe, "--welcome"),
                        Err(e) => message(&format!("Install failed: {e}"), MB_ICONERROR),
                    };
                }
            }
            run(Vec::new(), false)
        }
    }
}

fn install() -> Result<PathBuf, String> {
    let exe = orbit_setup::install()?;
    orbit_shell::register_context_menu(&exe)?;
    Ok(exe)
}

fn exit_with(r: Result<(), String>) {
    if let Err(e) = r {
        message(&e, MB_ICONERROR);
        std::process::exit(1);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn message(text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let t = wide(text);
    unsafe { MessageBoxW(None, PCWSTR(t.as_ptr()), w!("Orbit"), style | MB_SETFOREGROUND) }
}

fn spawn(exe: &Path, arg: &str) {
    let _ = std::process::Command::new(exe).arg(arg).spawn();
}

fn host_window() -> Option<HWND> {
    unsafe { FindWindowW(CLASS, None).ok() }
}

fn close_running() {
    if let Some(h) = host_window() {
        unsafe {
            let _ = SendMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Hand `paths` to a running Orbit. Returns false if none is running (and none is starting).
fn forward(paths: &[PathBuf]) -> bool {
    unsafe {
        // The first process to grab the mutex becomes the host; latecomers wait for its window.
        let mutex = CreateMutexW(None, false, w!("Local\\Orbit.Singleton"));
        let first = GetLastError() != ERROR_ALREADY_EXISTS;
        if first {
            std::mem::forget(mutex); // held for the life of the host
            return false;
        }
        for _ in 0..40 {
            if let Some(h) = host_window() {
                let mut data: Vec<u16> = Vec::new();
                for p in paths {
                    data.extend(p.as_os_str().to_string_lossy().encode_utf16());
                    data.push(0);
                }
                let cds = COPYDATASTRUCT {
                    dwData: COPY_RING,
                    cbData: (data.len() * 2) as u32,
                    lpData: data.as_mut_ptr().cast(),
                };
                let _ = AllowSetForegroundWindow(ASFW_ANY); // let the ring take focus
                SendMessageW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize));
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        true // a host is starting but never showed up; don't start a second one
    }
}

fn run(initial: Vec<PathBuf>, welcome: bool) {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW, CLASS, w!("Orbit"), WS_POPUP, 0, 0, 0, 0, None, None, hinst, None,
        )
        .expect("host window");
        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        if RegisterHotKey(hwnd, HOTKEY, MOD_ALT | MOD_NOREPEAT, u32::from(b'Q')).is_err() {
            message(
                "Alt+Q is already used by another app, so Orbit can only be opened from the right-click menu.",
                MB_ICONWARNING,
            );
        }
        tray(hwnd, NIM_ADD);
        if welcome {
            balloon(hwnd, "Orbit is ready", "Select a file in Explorer and press Alt+Q.");
        }
        if !initial.is_empty() {
            PENDING.with_borrow_mut(|p| p.extend(initial));
            SetTimer(hwnd, TIMER_COLLECT, 1, None);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        tray(hwnd, NIM_DELETE);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => show_ring(orbit_shell::selection()),
        WM_COPYDATA => {
            let cds = &*(lp.0 as *const COPYDATASTRUCT);
            if cds.dwData != COPY_RING {
                return LRESULT(0);
            }
            let words = std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2);
            let paths: Vec<PathBuf> = words
                .split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(|s| PathBuf::from(String::from_utf16_lossy(s)))
                .collect();
            if paths.is_empty() {
                balloon(hwnd, "Orbit is running", "Select a file in Explorer and press Alt+Q.");
            } else {
                PENDING.with_borrow_mut(|p| p.extend(paths));
                SetTimer(hwnd, TIMER_COLLECT, 250, None);
            }
            return LRESULT(1);
        }
        WM_TIMER if wp.0 == TIMER_COLLECT => {
            let _ = KillTimer(hwnd, TIMER_COLLECT);
            let mut paths = PENDING.with_borrow_mut(std::mem::take);
            paths.dedup();
            show_ring(paths);
        }
        WM_TRAY => {
            let event = (lp.0 & 0xFFFF) as u32;
            if event == WM_CONTEXTMENU || event == WM_RBUTTONUP {
                tray_menu(hwnd);
            } else if event == WM_LBUTTONUP || event == NIN_SELECT {
                balloon(hwnd, "Orbit", "Select a file in Explorer and press Alt+Q.");
            }
        }
        WM_COMMAND => match wp.0 & 0xFFFF {
            ID_PLUGINS => {
                let d = wide(&orbit_plugins::dir().to_string_lossy());
                ShellExecuteW(None, w!("open"), PCWSTR(d.as_ptr()), None, None, SW_SHOWNORMAL);
            }
            ID_AUTOSTART => {
                if let Err(e) = orbit_setup::set_autostart(!orbit_setup::autostart()) {
                    message(&e, MB_ICONERROR);
                }
            }
            ID_QUIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        },
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => PostQuitMessage(0),
        m if m == TASKBAR_CREATED.get() && m != 0 => tray(hwnd, NIM_ADD), // Explorer restarted
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

fn show_ring(paths: Vec<PathBuf>) {
    let mut actions = Vec::new();
    if !paths.is_empty() {
        actions = orbit_engine::actions_for(&paths);
        actions.extend(orbit_plugins::actions_for(&paths));
        actions.extend(orbit_rename::actions_for(&paths));
    }
    let icon = paths.first().and_then(|p| orbit_shell::icon(p, 128));
    let picked = paths.clone();
    orbit_ring::open(&paths, icon, actions, Box::new(move |id: &str, task: Task| dispatch(id, picked.clone(), task)));
}

fn dispatch(id: &str, paths: Vec<PathBuf>, task: Task) {
    if id == "rename.batch" {
        task.dismiss();
        orbit_rename::open(paths);
        return;
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let report = |f: f32| task.progress(f);
        let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Outcome, String> {
            if id == "rename.undo" {
                orbit_rename::undo()
            } else if id.starts_with("plugin:") {
                orbit_plugins::run(&id, &paths, &report)
            } else {
                orbit_engine::run(&id, &paths, &report)
            }
        }));
        let result = work.unwrap_or_else(|_| Err("Something went wrong while processing this file.".into()));
        if let Ok(o) = &result {
            if !o.reveal.is_empty() {
                orbit_shell::reveal(&o.reveal);
            }
        }
        task.finish(result);
    });
}

// ---------- tray ----------

fn tray(hwnd: HWND, op: NOTIFY_ICON_MESSAGE) {
    let mut nid = notify_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) } as u32;
    nid.hIcon = hicon(&orbit_setup::icon_bitmap(size.max(16)));
    copy_into(&mut nid.szTip, "Orbit — Alt+Q");
    unsafe {
        let _ = Shell_NotifyIconW(op, &nid);
        if op == NIM_ADD {
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
        let _ = DestroyIcon(nid.hIcon);
    }
}

fn balloon(hwnd: HWND, title: &str, text: &str) {
    let mut nid = notify_data(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON | NIIF_NOSOUND;
    nid.hBalloonIcon = hicon(&orbit_setup::icon_bitmap(64));
    copy_into(&mut nid.szInfoTitle, title);
    copy_into(&mut nid.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        let _ = DestroyIcon(nid.hBalloonIcon);
    }
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn tray_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let auto = if orbit_setup::autostart() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, w!("Orbit — select files, press Alt+Q"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_PLUGINS, w!("Open plugins folder"));
        let _ = AppendMenuW(menu, MF_STRING | auto, ID_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit Orbit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd); // required so the menu closes when clicking elsewhere
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

/// Premultiplied BGRA -> HICON (icons want straight alpha).
fn hicon(b: &Bitmap) -> HICON {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: b.width as i32,
                biHeight: -(b.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, b.bgra.len());
        for (d, s) in dst.chunks_exact_mut(4).zip(b.bgra.chunks_exact(4)) {
            let a = s[3] as u32;
            for i in 0..3 {
                d[i] = if a == 0 { 0 } else { ((s[i] as u32 * 255 + a / 2) / a).min(255) as u8 };
            }
            d[3] = s[3];
        }
        let mask = CreateBitmap(b.width as i32, b.height as i32, 1, 1, None);
        let info = ICONINFO { fIcon: TRUE, hbmMask: mask, hbmColor: color, ..Default::default() };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}
