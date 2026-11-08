//! Manual driver: `cargo run -p orbit-engine --example run -- <action-id> <files...>`
//! With only files, prints the actions offered for them.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (id, files) = match args.split_first() {
        Some((id, files)) if !files.is_empty() && !std::path::Path::new(id).exists() => (id.clone(), files.to_vec()),
        _ if !args.is_empty() => (String::new(), args),
        _ => return eprintln!("usage: run <action-id> <files...>   |   run <files...>"),
    };
    let paths: Vec<std::path::PathBuf> = files.iter().map(Into::into).collect();
    if id.is_empty() {
        for a in orbit_engine::actions_for(&paths) {
            println!("{:<16} {}", a.id, a.label);
            for c in a.children {
                println!("  {:<14} {}", c.id, c.label);
            }
        }
        return;
    }
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let start = std::time::Instant::now();
    let report = |f: f32| eprint!("\r{:5.1}%", f * 100.0);
    match orbit_engine::run(&id, &paths, &report) {
        Ok(o) => {
            println!("\nOK in {:.1}s: {}", start.elapsed().as_secs_f32(), o.message);
            for r in o.reveal {
                println!("  -> {}", r.display());
            }
        }
        Err(e) => {
            println!("\nERROR: {e}");
            std::process::exit(1);
        }
    }
}
