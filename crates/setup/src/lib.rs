//! Per-user install / uninstall / autostart / app icon. OWNER: Agent 6 (Packaging & Installer).

use orbit_core::Bitmap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs, thread, time::Duration};
use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_CLOSE};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const UNINSTALL_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Orbit");
const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// %LOCALAPPDATA%\Programs\Orbit\orbit.exe
pub fn installed_exe() -> PathBuf {
    PathBuf::from(env::var_os("LOCALAPPDATA").unwrap_or_default()).join(r"Programs\Orbit\orbit.exe")
}

/// True if the running exe is the installed copy.
pub fn is_installed_copy() -> bool {
    let canon = |p: PathBuf| fs::canonicalize(p).map(|p| p.to_string_lossy().to_lowercase());
    match (env::current_exe().and_then(canon), canon(installed_exe())) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Copy the running exe to `installed_exe()`, write orbit.ico next to it, Start-menu shortcut, autostart,
/// and an Apps & Features uninstall entry (UninstallString = `"<exe>" --uninstall`). No admin. Idempotent (upgrade in place:
/// close a running installed copy first). The host registers the Explorer context menu afterwards.
pub fn install() -> Result<PathBuf, String> {
    let exe = installed_exe();
    let dir = exe.parent().ok_or("LOCALAPPDATA is not set")?;
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    if !is_installed_copy() {
        close_running(&exe);
        let me = env::current_exe().map_err(|e| e.to_string())?;
        fs::copy(&me, &exe).map_err(|e| format!("Cannot copy Orbit to {}: {e}", exe.display()))?;
    }
    let ico = dir.join("orbit.ico");
    let ico_bytes = ico_file(&ICO_SIZES);
    fs::write(&ico, &ico_bytes).map_err(|e| format!("Cannot write {}: {e}", ico.display()))?;
    let lnk = shortcut_path();
    if let Some(p) = lnk.parent() {
        let _ = fs::create_dir_all(p);
    }
    write_shortcut(&lnk, &exe, &ico).map_err(|e| format!("Cannot create the Start menu shortcut: {e}"))?;
    write_run(&exe)?;

    let quoted = format!("\"{}\"", exe.display());
    let kb = (fs::metadata(&exe).map(|m| m.len()).unwrap_or(0) + ico_bytes.len() as u64).div_ceil(1024) as u32;
    for (name, value) in [
        (w!("DisplayName"), "Orbit"),
        (w!("DisplayVersion"), env!("CARGO_PKG_VERSION")),
        (w!("Publisher"), "Orbit"),
        (w!("DisplayIcon"), &ico.to_string_lossy()),
        (w!("InstallLocation"), &dir.to_string_lossy()),
        (w!("UninstallString"), &format!("{quoted} --uninstall")),
    ] {
        set_string(UNINSTALL_KEY, name, value)?;
    }
    for (name, value) in [(w!("EstimatedSize"), kb), (w!("NoModify"), 1), (w!("NoRepair"), 1)] {
        reg(unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, UNINSTALL_KEY, name, REG_DWORD.0, Some((&value as *const u32).cast()), 4) })?;
    }
    Ok(exe)
}

/// Reverse everything `install` did (the host removes the context menu itself), including deleting the install folder after this process exits.
pub fn uninstall() -> Result<(), String> {
    match fs::remove_file(shortcut_path()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("Cannot remove the Start menu shortcut: {e}")),
        _ => {}
    }
    gone(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Orbit")) })?;
    gone(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, UNINSTALL_KEY) })?;
    let exe = installed_exe();
    let Some(dir) = exe.parent().filter(|d| d.exists()) else { return Ok(()) };
    // The installed exe may be the caller (and may sit on a message box for a while): retry every ~1 s for 2 minutes.
    let d = dir.display();
    Command::new("cmd")
        .raw_arg(format!(
            "/d /s /c \"for /l %i in (1,1,120) do @(ping -n 2 127.0.0.1 >nul & rmdir /s /q \"{d}\" 2>nul & if not exist \"{d}\" exit)\""
        ))
        .current_dir(env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Cannot schedule removal of {d}: {e}"))
}

pub fn autostart() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Orbit"), RRF_RT_REG_SZ, None, None, None) }.is_ok()
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    if on {
        write_run(&env::current_exe().map_err(|e| e.to_string())?)
    } else {
        gone(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Orbit")) })
    }
}

fn shortcut_path() -> PathBuf {
    PathBuf::from(env::var_os("APPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Start Menu\Programs\Orbit.lnk")
}

fn write_run(exe: &Path) -> Result<(), String> {
    set_string(RUN_KEY, w!("Orbit"), &format!("\"{}\"", exe.display()))
}

fn set_string(key: PCWSTR, name: PCWSTR, value: &str) -> Result<(), String> {
    let v: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    reg(unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_SZ.0, Some(v.as_ptr().cast()), (v.len() * 2) as u32) })
}

fn reg(e: WIN32_ERROR) -> Result<(), String> {
    e.ok().map_err(|e| format!("Registry write failed: {}", e.message()))
}

/// Deleting something that is already gone is fine.
fn gone(e: WIN32_ERROR) -> Result<(), String> {
    if e == ERROR_FILE_NOT_FOUND { Ok(()) } else { reg(e) }
}

/// Ask a running Orbit host to quit, then wait up to ~3 s for `exe` to become writable.
fn close_running(exe: &Path) {
    unsafe {
        if let Ok(h) = FindWindowW(w!("Orbit.Host"), PCWSTR::null()) {
            SendMessageTimeoutW(h, WM_CLOSE, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 3000, None);
        }
    }
    for _ in 0..30 {
        if !exe.exists() || fs::OpenOptions::new().write(true).open(exe).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn write_shortcut(lnk: &Path, exe: &Path, ico: &Path) -> windows::core::Result<()> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = (|| {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe))?;
            link.SetWorkingDirectory(&HSTRING::from(exe.parent().unwrap_or(exe)))?;
            link.SetIconLocation(&HSTRING::from(ico), 0)?;
            link.SetDescription(w!("The fastest way to act on files."))?;
            link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk), true)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        r
    }
}

/// .ico with one 32-bit BMP entry per size (straight alpha, empty AND mask — alpha does the masking).
fn ico_file(sizes: &[u32]) -> Vec<u8> {
    let mut dir = vec![0, 0, 1, 0];
    dir.extend((sizes.len() as u16).to_le_bytes());
    let mut data = Vec::new();
    for &s in sizes {
        let bmp = icon_bitmap(s);
        let mask_row = s.div_ceil(32) as usize * 4;
        let start = data.len();
        for v in [40, s, s * 2] {
            data.extend(v.to_le_bytes());
        }
        data.extend(1u16.to_le_bytes());
        data.extend(32u16.to_le_bytes());
        data.extend([0u8; 24]);
        for row in bmp.bgra.chunks_exact(s as usize * 4).rev() {
            for p in row.chunks_exact(4) {
                let a = p[3] as u32;
                let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a / 2) / a).min(255) as u8 };
                data.extend([un(p[0]), un(p[1]), un(p[2]), p[3]]);
            }
        }
        data.resize(data.len() + mask_row * s as usize, 0);
        let len = (data.len() - start) as u32;
        let offset = 6 + 16 * sizes.len() as u32 + start as u32;
        dir.extend([(s % 256) as u8, (s % 256) as u8, 0, 0]);
        dir.extend(1u16.to_le_bytes());
        dir.extend(32u16.to_le_bytes());
        dir.extend(len.to_le_bytes());
        dir.extend(offset.to_le_bytes());
    }
    dir.extend(data);
    dir
}

// ---------------------------------------------------------------- logo

type Rgba = [f32; 4]; // premultiplied

const TILT: f32 = -0.40; // radians; the orbit rises to the right
const RING_A: f32 = 0.97;
const RING_B: f32 = 0.30;
const SAT_T: f32 = 0.45; // satellite position on the ring (front, right)

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn paint(c: [f32; 3], a: f32) -> Rgba {
    [c[0] * a, c[1] * a, c[2] * a, a]
}

fn over(dst: Rgba, src: Rgba) -> Rgba {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

/// One sample of the logo at (x, y) in [-1, 1]² (y down); `px` = one pixel in those units.
fn sample(x: f32, y: f32, px: f32) -> Rgba {
    let small = px > 0.07; // ≤ ~28 px: fewer, bolder details
    let tiny = px > 0.1; // ≤ 20 px (tray): bolder still
    // Orbit ellipse in its own (untilted) frame; ly > 0 is the near half, drawn in front of the orb.
    let (s, c) = TILT.sin_cos();
    let (lx, ly) = (x * c + y * s, -x * s + y * c);
    let f = (lx / RING_A).powi(2) + (ly / RING_B).powi(2) - 1.0;
    let g = ((2.0 * lx / (RING_A * RING_A)).powi(2) + (2.0 * ly / (RING_B * RING_B)).powi(2)).sqrt().max(1e-4);
    let ring_d = (f / g).abs();
    let hw = if tiny { 0.9 * px } else if small { 0.75 * px } else { 0.024f32.max(0.6 * px) };
    let gap = if small { 0.55 * px } else { 0.035f32.max(0.7 * px) };
    let front = ly > 0.0;

    // Satellite.
    let (sx, sy) = (RING_A * SAT_T.cos(), RING_B * SAT_T.sin());
    let (sx, sy) = (sx * c - sy * s, sx * s + sy * c);
    let sd = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt();
    let sr = if small { 1.35 * px } else { 0.085f32.max(1.2 * px) };

    let r_orb = if small { 0.66 } else { 0.62 };
    let (qx, qy) = (x / r_orb, y / r_orb);
    let r = (qx * qx + qy * qy).sqrt();

    let mut out = [0.0; 4];
    // Ring (hidden next to the satellite so the dot reads on its own).
    let ring = if ring_d < hw && sd > sr + gap {
        let t = (lx / RING_A * 0.5 + 0.5).clamp(0.0, 1.0);
        let col = mix([0.58, 0.40, 1.0], [0.12, 0.78, 0.98], t);
        // At tray sizes the near side is lightened so it separates from the orb behind it.
        let col = if tiny && front && r < 1.0 { mix(col, [1.0; 3], 0.4) } else { col };
        paint(col, if front || tiny { 1.0 } else { 0.8 })
    } else {
        [0.0; 4]
    };
    if !front {
        out = over(out, ring);
    }

    // Glass orb.
    let cut = front && ring_d < hw + gap;
    if r < 1.0 && !cut {
        let diag = (qx * 0.62 + qy * 0.78).clamp(-1.0, 1.0); // -1 top-left (lit) .. 1 bottom-right
        let t = diag * 0.5 + 0.5;
        let t = if tiny { 0.25 + 0.75 * t } else { t }; // tiny: skip the palest cyan, keep the orb saturated
        let (cyan, violet, indigo) = ([0.32, 0.86, 1.0], [0.50, 0.30, 0.96], [0.13, 0.07, 0.42]);
        let mut col = if t < 0.45 { mix(cyan, violet, t / 0.45) } else { mix(violet, indigo, (t - 0.45) / 0.55) };
        let limb = 1.0 - 0.30 * r.powi(4);
        col = col.map(|v| v * limb);
        // Inner rim glow on the shadow side, like light bouncing inside glass.
        let rim = smooth(0.78, 1.0, r) * ((qx * 0.62 + qy * 0.78) / r.max(1e-3)).max(0.0) * 0.55;
        col = mix(col, [0.45, 0.55, 1.0], rim);
        let mut px_col = paint(col, 1.0);
        // Thin bright edge on the lit side.
        let edge = smooth(0.86, 0.98, r) * (-(qx * 0.62 + qy * 0.78) / r.max(1e-3)).max(0.0) * if small { 0.0 } else { 0.5 };
        px_col = over(px_col, paint([1.0, 1.0, 1.0], edge));
        // Soft specular highlight, upper left.
        let (hx, hy) = (qx + 0.30, qy + 0.42);
        let (hs, hc) = (-0.6f32).sin_cos();
        let (ex, ey) = ((hx * hc + hy * hs) / 0.42, (-hx * hs + hy * hc) / 0.24);
        let e = (ex * ex + ey * ey).sqrt();
        let spec = (1.0 - smooth(0.0, 1.0, e)).powf(1.4) * if tiny { 0.3 } else if small { 0.55 } else { 0.75 };
        px_col = over(px_col, paint([1.0, 1.0, 1.0], spec));
        out = over(out, px_col);
    }
    if front {
        out = over(out, ring);
    }

    // Satellite: halo + bright core.
    if !small {
        out = over(out, paint([0.30, 0.85, 1.0], 0.35 * (-(sd / (sr * 1.6)).powi(2)).exp() * (sd > sr) as u8 as f32));
    }
    if sd < sr {
        out = over(out, paint(mix([1.0, 1.0, 1.0], [0.10, 0.74, 0.98], (sd / sr).powi(2)), 1.0));
    }
    out
}

/// Orbit logo rendered procedurally at `size`x`size`, premultiplied BGRA (tray icon, .ico, windows).
pub fn icon_bitmap(size: u32) -> Bitmap {
    const SS: u32 = 4;
    let px = 2.0 / size as f32;
    let mut bgra = Vec::with_capacity((size * size * 4) as usize);
    for j in 0..size {
        for i in 0..size {
            let mut acc = [0.0f32; 4];
            for sj in 0..SS {
                for si in 0..SS {
                    let x = (i as f32 + (si as f32 + 0.5) / SS as f32) * px - 1.0;
                    let y = (j as f32 + (sj as f32 + 0.5) / SS as f32) * px - 1.0;
                    let v = sample(x, y, px);
                    acc = [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2], acc[3] + v[3]];
                }
            }
            let n = (SS * SS) as f32;
            let a = (acc[3] / n).min(1.0);
            let ch = |v: f32| ((v / n).min(a) * 255.0 + 0.5) as u8;
            bgra.extend([ch(acc[2]), ch(acc[1]), ch(acc[0]), (a * 255.0 + 0.5) as u8]);
        }
    }
    Bitmap { width: size, height: size, bgra }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_premultiplied_with_transparent_corners() {
        for s in [16, 24, 32, 48, 256] {
            let b = icon_bitmap(s);
            assert_eq!(b.bgra.len(), (s * s * 4) as usize);
            assert!(b.bgra.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
            for (x, y) in [(0, 0), (s - 1, 0), (0, s - 1), (s - 1, s - 1)] {
                assert_eq!(b.bgra[((y * s + x) * 4 + 3) as usize], 0, "corner {x},{y} at {s}px");
            }
            assert!(b.bgra.chunks_exact(4).filter(|p| p[3] == 255).count() as u32 > s * s / 5, "solid body at {s}px");
        }
    }

    #[test]
    fn ico_parses_back() {
        let ico = ico_file(&ICO_SIZES);
        let u16_at = |o: usize| u16::from_le_bytes([ico[o], ico[o + 1]]) as u32;
        let u32_at = |o: usize| u32::from_le_bytes(ico[o..o + 4].try_into().unwrap());
        assert_eq!((u16_at(0), u16_at(2), u16_at(4)), (0, 1, ICO_SIZES.len() as u32));
        let mut end = 6 + 16 * ICO_SIZES.len();
        for (k, &s) in ICO_SIZES.iter().enumerate() {
            let e = 6 + 16 * k;
            let w = if ico[e] == 0 { 256 } else { ico[e] as u32 };
            assert_eq!((w, ico[e + 1] as u32 % 256, u16_at(e + 4), u16_at(e + 6)), (s, s % 256, 1, 32));
            let (len, off) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
            assert_eq!(off, end, "entries are contiguous");
            assert_eq!(len, 40 + (s * s * 4) as usize + s.div_ceil(32) as usize * 4 * s as usize);
            assert_eq!((u32_at(off), u32_at(off + 4), u32_at(off + 8), u16_at(off + 14)), (40, s, 2 * s, 32));
            // Windows itself must accept the entry.
            use windows::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, DestroyIcon, LR_DEFAULTCOLOR};
            let icon = unsafe { CreateIconFromResourceEx(&ico[off..off + len], true, 0x0003_0000, s as i32, s as i32, LR_DEFAULTCOLOR) };
            unsafe { DestroyIcon(icon.expect("CreateIconFromResourceEx")).unwrap() };
            end = off + len;
        }
        assert_eq!(end, ico.len());
    }
}
