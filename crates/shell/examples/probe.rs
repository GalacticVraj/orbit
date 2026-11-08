//! End-to-end check of orbit-shell against a real Explorer window.
//! cargo run -p orbit-shell --example probe   (steals focus for a few seconds; closes the window it opens)

use orbit_core::Bitmap;
use orbit_shell::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let out = PathBuf::from(r"E:\Project\orbit\target\shell-probe");
    std::fs::create_dir_all(&out).unwrap();
    let dir = std::env::temp_dir().join("orbit-probe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Sub folder")).unwrap();
    for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
        std::fs::write(dir.join(n), n).unwrap();
    }
    let png = dir.join("photo.png");
    write_png(&png, &gradient(320, 200));

    // (c) icons / thumbnails
    for (name, p) in [("png", png.clone()), ("folder", dir.join("Sub folder")), ("exe", PathBuf::from(r"C:\Windows\explorer.exe"))] {
        let t = Instant::now();
        let b = icon(&p, 128).expect("icon");
        println!("icon({name}): {}x{} in {:?}", b.width, b.height, t.elapsed());
        assert_eq!((b.width, b.height, b.bgra.len()), (128, 128, 128 * 128 * 4));
        write_png(&out.join(format!("icon-{name}.png")), &b);
    }
    let t = std::thread::spawn(move || icon(&png, 64).map(|b| b.width)).join().unwrap();
    println!("icon() from a fresh thread: {t:?}");

    // (a) open Explorer on the folder
    let before = cabinets();
    std::process::Command::new("explorer.exe").arg(&dir).spawn().unwrap();
    let hwnd = wait(5000, || cabinets().into_iter().find(|h| !before.contains(h) && title(*h).contains("orbit-probe"))).expect("explorer window");
    unsafe {
        if GetForegroundWindow() != hwnd {
            // Foreground lock: a synthetic Alt press lets a background process take focus.
            keybd_event(VK_MENU.0 as u8, 0, Default::default(), 0);
            keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    std::thread::sleep(Duration::from_millis(1500)); // let the view populate
    println!("explorer window {:?} '{}', foreground: {}", hwnd.0, title(hwnd), unsafe { GetForegroundWindow() } == hwnd);

    // (b) select via reveal(), read back via selection()
    let want = vec![dir.join("alpha.txt"), dir.join("photo.png")];
    let t = Instant::now();
    reveal(&want);
    println!("reveal(existing x2): {:?}", t.elapsed());
    check(&want);

    let fresh = dir.join("delta (new).txt");
    std::fs::write(&fresh, "new").unwrap();
    let t = Instant::now();
    reveal(&[fresh.clone()]);
    println!("reveal(just-created file): {:?}", t.elapsed());
    check(&[fresh]);

    let t = Instant::now();
    reveal(&[PathBuf::from(r"C:\Windows\notepad.exe")]);
    println!("reveal(no matching window, must be a no-op): {:?}, windows before/after: {}/{}", t.elapsed(), before.len() + 1, cabinets().len());

    unsafe {
        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
    }

    // (d) context menu
    let exe = Path::new(r"C:\Orbit Test\orbit.exe");
    register_context_menu(exe).unwrap();
    for key in [r"Software\Classes\*\shell\Orbit", r"Software\Classes\Directory\shell\Orbit"] {
        for v in ["MUIVerb", "Icon", "MultiSelectModel"] {
            println!("{key} {v} = {:?}", reg(key, v));
        }
        println!("{key}\\command = {:?}", reg(&format!(r"{key}\command"), ""));
    }
    unregister_context_menu();
    println!("after unregister: {:?} / {:?}", reg(r"Software\Classes\*\shell\Orbit", "MUIVerb"), reg(r"Software\Classes\Directory\shell\Orbit", "MUIVerb"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn check(want: &[PathBuf]) {
    let mut times = Vec::new();
    let mut got = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        got = selection();
        times.push(t.elapsed());
    }
    let first = times[0];
    times.sort();
    println!("selection() = {got:?}\n  first {first:?}, median {:?}, max {:?}", times[10], times[19]);
    let (mut a, mut b) = (got.clone(), want.to_vec());
    a.sort();
    b.sort();
    println!("  {}", if a == b { "MATCH" } else { "MISMATCH" });
}

fn wait<T>(ms: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() > end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn cabinets() -> Vec<HWND> {
    let mut v = Vec::new();
    let mut h = HWND::default();
    while let Ok(n) = unsafe { FindWindowExW(None, h, w!("CabinetWClass"), None) } {
        v.push(n);
        h = n;
    }
    v
}

fn title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(h, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn reg(key: &str, value: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
    };
    r.is_ok().then(|| String::from_utf16_lossy(&buf[..len as usize / 2 - 1]))
}

/// Semi-transparent-edged colour gradient (premultiplied), to see thumbnails + alpha handling.
fn gradient(w: u32, h: u32) -> Bitmap {
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let a = if x < 20 || x >= w - 20 { 128 } else { 255 };
            let pm = |c: u32| (c * a / 255) as u8;
            bgra.extend([pm(255 * y / h), pm(120), pm(255 * x / w), a as u8]);
        }
    }
    Bitmap { width: w, height: h, bgra }
}

fn write_png(path: &Path, b: &Bitmap) {
    let mut px = b.bgra.clone(); // PNG wants straight alpha
    for p in px.chunks_exact_mut(4) {
        if p[3] != 0 {
            for c in 0..3 {
                p[c] = (p[c] as u32 * 255 / p[3] as u32).min(255) as u8;
            }
        }
    }
    unsafe {
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let stream = f.CreateStream().unwrap();
        stream.InitializeFromFilename(&HSTRING::from(path), GENERIC_WRITE.0).unwrap();
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).unwrap();
        enc.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut()).unwrap();
        let frame = frame.unwrap();
        frame.Initialize(None).unwrap();
        frame.SetSize(b.width, b.height).unwrap();
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt).unwrap();
        frame.WritePixels(b.height, b.width * 4, &px).unwrap();
        frame.Commit().unwrap();
        enc.Commit().unwrap();
    }
}
