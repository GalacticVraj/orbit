//! Windows Explorer integration. OWNER: Agent 2 (Explorer Integration).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::Bitmap;
use std::path::{Path, PathBuf};

/// Full paths of the items selected in the *foreground* Explorer window (active tab on
/// Windows 11 tabbed Explorer) or on the desktop. Empty if the foreground window is neither
/// or nothing is selected. Called on the UI thread, which is already COM STA-initialised.
/// Must be fast (<20 ms typical): no polling, no enumeration beyond what's needed.
pub fn selection() -> Vec<PathBuf> {
    unimplemented!()
}

/// Shell thumbnail (images/videos/PDF previews when Explorer has one) or file icon, `size`x`size`
/// physical pixels, premultiplied BGRA. Works for files and folders. Any thread (init COM itself if needed).
pub fn icon(path: &Path, size: u32) -> Option<Bitmap> {
    let _ = (path, size);
    unimplemented!()
}

/// Select `paths` in an Explorer window that is *already open* on their folder (or on the desktop
/// if they live there). Never opens a new window. Silently does nothing otherwise. Any thread.
pub fn reveal(paths: &[PathBuf]) {
    let _ = paths;
    unimplemented!()
}

/// Per-user (HKCU) "Orbit" entry in the Explorer right-click menu for files and folders,
/// running `"<exe>" --ring "%1"` with Orbit's icon. Must not require admin.
pub fn register_context_menu(exe: &Path) -> Result<(), String> {
    let _ = exe;
    unimplemented!()
}

pub fn unregister_context_menu() {
    unimplemented!()
}
