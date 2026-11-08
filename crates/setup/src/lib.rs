//! Per-user install / uninstall / autostart / app icon. OWNER: Agent 6 (Packaging & Installer).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::Bitmap;
use std::path::PathBuf;

/// %LOCALAPPDATA%\Programs\Orbit\orbit.exe
pub fn installed_exe() -> PathBuf {
    unimplemented!()
}

/// True if the running exe is the installed copy.
pub fn is_installed_copy() -> bool {
    unimplemented!()
}

/// Copy the running exe to `installed_exe()`, write orbit.ico next to it, Start-menu shortcut, autostart,
/// and an Apps & Features uninstall entry (UninstallString = `"<exe>" --uninstall`). No admin. Idempotent (upgrade in place:
/// close a running installed copy first). The host registers the Explorer context menu afterwards.
pub fn install() -> Result<PathBuf, String> {
    unimplemented!()
}

/// Reverse everything `install` did (the host removes the context menu itself), including deleting the install folder after this process exits.
pub fn uninstall() -> Result<(), String> {
    unimplemented!()
}

pub fn autostart() -> bool {
    unimplemented!()
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let _ = on;
    unimplemented!()
}

/// Orbit logo rendered procedurally at `size`x`size`, premultiplied BGRA (tray icon, .ico, windows).
pub fn icon_bitmap(size: u32) -> Bitmap {
    let _ = size;
    unimplemented!()
}
