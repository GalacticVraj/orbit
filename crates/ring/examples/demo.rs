//! Live ring with sample actions. Alt+Shift+Q reopens it at the cursor; close the console to quit.
//!   cargo run -p orbit-ring --example demo            (interactive)
//!   cargo run -p orbit-ring --example demo -- --auto  (scripted smoke test, exits by itself)
//! Picks: "Split pages" fails, "Rename" dismisses (opens "its own window"), anything else succeeds.

use orbit_core::{Action, Outcome};
use orbit_ring::Task;
use std::path::PathBuf;
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn actions() -> Vec<Action> {
    vec![
        Action::new("pdf.compress", "Compress PDF", '\u{E73F}'),
        Action::new("pdf.word", "Convert to Word", '\u{E8A5}'),
        Action::group("Convert", '\u{E8AB}', vec![
            Action::new("pdf.png", "Pages to PNG", '\u{EB9F}'),
            Action::new("pdf.jpg", "Pages to JPG", '\u{E91B}'),
            Action::new("pdf.txt", "Extract text", '\u{E8D2}'),
        ]),
        Action::new("pdf.split", "Split pages", '\u{E8C6}'),
        Action::new("pdf.rotate", "Rotate", '\u{E7AD}'),
        Action::new("rename.batch", "Rename", '\u{E8AC}'),
        Action::new("copy.path", "Copy path", '\u{E8C8}'),
    ]
}

fn show(empty: bool) {
    let paths = [PathBuf::from(r"C:\Users\me\Documents\Quarterly report.pdf")];
    let acts = if empty { Vec::new() } else { actions() };
    orbit_ring::open(&paths, None, acts, Box::new(|id: &str, task: Task| pick(id, task)));
}

fn pick(id: &str, task: Task) {
    println!("picked {id}");
    if id == "rename.batch" {
        task.dismiss();
        return;
    }
    let fail = id == "pdf.split";
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700)); // spinner first
        for i in 0..=20 {
            task.progress(i as f32 / 20.0);
            std::thread::sleep(Duration::from_millis(70));
        }
        task.finish(if fail {
            Err("This PDF is password-protected, so its pages can't be split.".into())
        } else {
            Ok(Outcome::new("Saved 4.2 MB", vec![]))
        });
    });
}

fn key(vk: VIRTUAL_KEY) {
    unsafe {
        if let Ok(h) = FindWindowW(w!("Orbit.Ring"), None) {
            let _ = PostMessageW(h, WM_KEYDOWN, WPARAM(vk.0 as usize), LPARAM(0));
        }
    }
}

thread_local!(static STEP: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });

unsafe extern "system" fn auto_step(_: HWND, _: u32, _: usize, _: u32) {
    let step = STEP.get() + 1;
    STEP.set(step);
    println!("auto step {step}");
    match step {
        1 | 2 => key(VK_RIGHT),
        3 => key(VK_RETURN),   // "Convert to Word": spinner, progress, success
        10 => show(false),     // new ring replaces the finished one
        11 => key(VK_3),       // "Convert" group -> sub-ring
        12 => key(VK_ESCAPE),  // back to the root
        13 => key(VK_4),       // "Split pages" -> error
        14 => key(VK_ESCAPE),  // hide while running; the error re-shows it
        22 => show(true),      // hint
        26 => {
            let mut m = PROCESS_MEMORY_COUNTERS::default();
            let _ = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
            println!("idle working set after hide: {:.1} MB", m.WorkingSetSize as f64 / 1048576.0);
        }
        27 => PostQuitMessage(0),
        _ => {}
    }
}

fn main() {
    let auto = std::env::args().any(|a| a == "--auto");
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = RegisterHotKey(None, 1, MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'Q'));
        if auto {
            SetTimer(None, 0, 500, Some(auto_step));
        }
    }
    println!("Orbit ring demo: Alt+Shift+Q opens the ring at the cursor.");
    let t = std::time::Instant::now();
    show(false);
    println!("open() returned after {:?} (first frame already pushed)", t.elapsed());
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == WM_HOTKEY {
                show(false);
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
