//! Windows Explorer integration. OWNER: Agent 2 (Explorer Integration).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::Bitmap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::core::{w, Error, Interface, Result as WResult, HSTRING, PWSTR, VARIANT};
use windows::Win32::Foundation::{E_FAIL, HWND, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::{RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetClassNameW, GetForegroundWindow};

/// Full paths of the items selected in the *foreground* Explorer window (active tab on
/// Windows 11 tabbed Explorer) or on the desktop. Empty if the foreground window is neither
/// or nothing is selected. Called on the UI thread, which is already COM STA-initialised.
/// Must be fast (<20 ms typical): no polling, no enumeration beyond what's needed.
pub fn selection() -> Vec<PathBuf> {
    unsafe { foreground_browser().and_then(|b| selected(&b)).unwrap_or_default() }
}

/// Shell thumbnail (images/videos/PDF previews when Explorer has one) or file icon, `size`x`size`
/// physical pixels, premultiplied BGRA. Works for files and folders. Any thread (init COM itself if needed).
pub fn icon(path: &Path, size: u32) -> Option<Bitmap> {
    let _com = Com::init();
    unsafe {
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let s = size as i32;
        // Cached thumbnail only (Explorer just rendered it, so it's almost always there), else the plain
        // icon: generating a fresh thumbnail costs 100+ ms on the hotkey path.
        let sz = SIZE { cx: s, cy: s };
        let hbm = f
            .GetImage(sz, SIIGBF_RESIZETOFIT | SIIGBF_INCACHEONLY)
            .or_else(|_| f.GetImage(sz, SIIGBF_RESIZETOFIT | SIIGBF_ICONONLY))
            .ok()?;
        let bmp = to_bitmap(hbm, size);
        let _ = DeleteObject(hbm);
        bmp
    }
}

/// Select `paths` in an Explorer window that is *already open* on their folder (or on the desktop
/// if they live there). Never opens a new window. Silently does nothing otherwise. Any thread.
pub fn reveal(paths: &[PathBuf]) {
    let _com = Com::init();
    if let Some(dir) = paths.first().and_then(|p| p.parent()) {
        let _ = unsafe { reveal_in(dir, paths) };
    }
}

/// Per-user (HKCU) "Orbit" entry in the Explorer right-click menu for files and folders,
/// running `"<exe>" --ring "%1"` with Orbit's icon. Must not require admin.
pub fn register_context_menu(exe: &Path) -> Result<(), String> {
    let icon = exe.with_file_name("orbit.ico");
    let cmd = format!("\"{}\" --ring \"%1\"", exe.display());
    for key in MENU_KEYS {
        set(key, "MUIVerb", "Orbit")?;
        set(key, "Icon", &icon.to_string_lossy())?;
        set(key, "MultiSelectModel", "Player")?;
        set(&format!("{key}\\command"), "", &cmd)?;
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

pub fn unregister_context_menu() {
    unsafe {
        for key in MENU_KEYS {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key));
        }
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

// ---------- internals ----------

const MENU_KEYS: [&str; 2] = [r"Software\Classes\*\shell\Orbit", r"Software\Classes\Directory\shell\Orbit"];

fn set(key: &str, name: &str, value: &str) -> Result<(), String> {
    let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), REG_SZ.0, Some(data.as_ptr().cast()), data.len() as u32 * 2)
    }
    .ok()
    .map_err(|e| format!("Could not add Orbit to the Explorer menu: {}", e.message()))
}

/// Balanced CoInitializeEx for "any thread" entry points; RPC_E_CHANGED_MODE (MTA thread) is fine as-is.
struct Com(bool);
impl Com {
    fn init() -> Self {
        Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}

/// Owned absolute PIDL.
struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as _)) }
    }
}

unsafe fn take_path(p: PWSTR) -> PathBuf {
    let path = PathBuf::from(OsString::from_wide(p.as_wide()));
    CoTaskMemFree(Some(p.0 as _));
    path
}

fn class(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

unsafe fn shell_windows() -> WResult<IShellWindows> {
    CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)
}

unsafe fn browser(d: &IDispatch) -> WResult<IShellBrowser> {
    d.cast::<IServiceProvider>()?.QueryService(&SID_STopLevelBrowser)
}

unsafe fn desktop(sw: &IShellWindows) -> WResult<IShellBrowser> {
    let mut hwnd = 0;
    let loc = VARIANT::from(CSIDL_DESKTOP as i32);
    browser(&sw.FindWindowSW(&loc, &VARIANT::default(), SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?)
}

unsafe fn foreground_browser() -> WResult<IShellBrowser> {
    let fg = GetForegroundWindow();
    match class(fg).as_str() {
        "CabinetWClass" => {
            // Windows 11 tabs: each tab is its own shell window; the active tab is the top ShellTabWindowClass child.
            // Pre-tab Explorer: IShellBrowser::GetWindow is the frame itself.
            let tab = FindWindowExW(fg, None, w!("ShellTabWindowClass"), None).unwrap_or(fg);
            let sw = shell_windows()?;
            for i in 0..sw.Count()? {
                if let Ok(b) = sw.Item(&VARIANT::from(i)).and_then(|d| browser(&d)) {
                    if b.GetWindow().is_ok_and(|h| h == tab || h == fg) {
                        return Ok(b);
                    }
                }
            }
            Err(E_FAIL.into())
        }
        "Progman" | "WorkerW" if FindWindowExW(fg, None, w!("SHELLDLL_DefView"), None).is_ok() => desktop(&shell_windows()?),
        _ => Err(E_FAIL.into()),
    }
}

unsafe fn selected(b: &IShellBrowser) -> WResult<Vec<PathBuf>> {
    let view: IFolderView2 = b.QueryActiveShellView()?.cast()?;
    let items = view.GetSelection(false)?;
    let mut out = Vec::new();
    for i in 0..items.GetCount()? {
        // Items without a filesystem path (This PC, Control Panel, ...) are skipped.
        if let Ok(p) = items.GetItemAt(i).and_then(|it| it.GetDisplayName(SIGDN_FILESYSPATH)) {
            out.push(take_path(p));
        }
    }
    Ok(out)
}

unsafe fn to_bitmap(hbm: HBITMAP, size: u32) -> Option<Bitmap> {
    let mut bm = BITMAP::default();
    if GetObjectW(hbm, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // ask for top-down regardless of the source orientation
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(None);
    let lines = GetDIBits(dc, hbm, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    if lines != h {
        return None;
    }
    fix_alpha(&mut px);
    Some(letterbox(&px, w, h, size))
}

/// All-zero alpha = opaque bitmap (24-bit source). Straight alpha (any channel > alpha) gets premultiplied.
fn fix_alpha(px: &mut [u8]) {
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    } else if px.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
        for p in px.chunks_exact_mut(4) {
            for c in 0..3 {
                p[c] = (p[c] as u32 * p[3] as u32 / 255) as u8;
            }
        }
    }
}

/// Centre a w x h image on a transparent size x size canvas (cropping the centre if it is larger).
fn letterbox(px: &[u8], w: i32, h: i32, size: u32) -> Bitmap {
    let s = size as i32;
    let (ox, oy) = ((s - w) / 2, (s - h) / 2);
    let mut out = vec![0u8; (s * s * 4) as usize];
    for y in 0.max(-oy)..h.min(s - oy) {
        let (x0, x1) = (0.max(-ox), w.min(s - ox));
        if x0 >= x1 {
            break;
        }
        let src = ((y * w + x0) * 4) as usize;
        let dst = (((y + oy) * s + x0 + ox) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        out[dst..dst + n].copy_from_slice(&px[src..src + n]);
    }
    Bitmap { width: size, height: size, bgra: out }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

unsafe fn folder_of(view: &IShellView) -> WResult<PathBuf> {
    let pf: IPersistFolder2 = view.cast::<IFolderView>()?.GetFolder()?;
    let pidl = Pidl(pf.GetCurFolder()?);
    Ok(take_path(SHGetNameFromIDList(pidl.0, SIGDN_FILESYSPATH)?))
}

unsafe fn reveal_in(dir: &Path, paths: &[PathBuf]) -> WResult<()> {
    let sw = shell_windows()?;
    let browsers = (0..sw.Count()?).filter_map(|i| sw.Item(&VARIANT::from(i)).and_then(|d| browser(&d)).ok()).chain(desktop(&sw).ok());
    let mut views: Vec<IShellView> = browsers
        .filter_map(|b| b.QueryActiveShellView().ok())
        .filter(|v| folder_of(v).is_ok_and(|f| same_dir(&f, dir)))
        .collect();
    if views.is_empty() {
        return Ok(());
    }
    let pidls: Vec<Pidl> = paths
        .iter()
        .filter(|p| p.parent().is_some_and(|d| same_dir(d, dir)))
        .filter_map(|p| {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(&HSTRING::from(p.as_path()), None, &mut pidl, 0, None).ok().map(|_| Pidl(pidl))
        })
        .collect();
    if pidls.is_empty() {
        return Err(Error::from(E_FAIL));
    }
    // Explorer learns about freshly written files asynchronously: retry briefly (~1 s max) until all are selected.
    for _ in 0..20 {
        views.retain(|v| !select(v, &pidls));
        if views.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

/// Select `pidls` in `view`; true once every one of them is actually selected.
unsafe fn select(view: &IShellView, pidls: &[Pidl]) -> bool {
    let Ok(fv) = view.cast::<IFolderView2>() else { return false };
    let first = (SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0 | SVSI_ENSUREVISIBLE.0 | SVSI_FOCUSED.0) as u32;
    for (i, p) in pidls.iter().enumerate() {
        let _ = view.SelectItem(ILFindLastID(p.0), if i == 0 { first } else { SVSI_SELECT.0 as u32 });
    }
    pidls.iter().all(|p| fv.GetSelectionState(ILFindLastID(p.0)).is_ok_and(|s| s & SVSI_SELECT.0 as u32 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixels() {
        let mut px = vec![0, 0, 0, 0, 10, 20, 30, 0]; // all-zero alpha => opaque
        fix_alpha(&mut px);
        assert_eq!((px[3], px[7]), (255, 255));
        let mut px = vec![200, 100, 50, 128]; // straight alpha => premultiplied
        fix_alpha(&mut px);
        assert_eq!(px, [100, 50, 25, 128]);
        let b = letterbox(&[1, 2, 3, 4, 5, 6, 7, 8], 2, 1, 4); // 2x1 centred in 4x4
        assert_eq!((b.width, b.bgra.len()), (4, 64));
        assert_eq!(&b.bgra[(4 + 1) * 4..(4 + 3) * 4], &[1, 2, 3, 4, 5, 6, 7, 8]);
        let b = letterbox(&[9; 4 * 9], 3, 3, 1); // larger than canvas => centre crop
        assert_eq!(b.bgra, [9; 4]);
        assert!(same_dir(Path::new("C:\\A\\b\\"), Path::new("c:\\a\\B")));
    }
}
