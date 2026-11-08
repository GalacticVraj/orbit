//! Opens the rename window on sample files: `cargo run -p orbit-rename --example window`.
//! (PNG renders for review: `cargo test -p orbit-rename -- --ignored --test-threads=1` -> target\rename-shots.)

use windows::core::w;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let dir = std::env::temp_dir().join("orbit-rename-demo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut paths = Vec::new();
    for i in [2, 10, 1, 24, 3, 101, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21] {
        let p = dir.join(format!("IMG_{}.jpg", 1000 + i));
        std::fs::write(&p, "demo").unwrap();
        paths.push(p);
    }
    println!("Sample files in {}", dir.display());
    orbit_rename::open(paths);
    unsafe {
        let mut msg = MSG::default();
        while FindWindowW(w!("Orbit.Rename"), None).is_ok() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        println!("Window closed; files now: {:?}", std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>());
    }
}
