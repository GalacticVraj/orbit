//! Office conversions: Word / PowerPoint automated over COM from a hidden PowerShell, LibreOffice as fallback.
//! Paths travel in environment variables, never inside the script text, so quotes and apostrophes are safe.

use crate::{name, parent, stem};
use orbit_core::{ext, free_path, output_path, Outcome, Progress};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Per input file; the Office process is killed if a file takes longer.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Exit code the scripts use for "application not installed".
const NO_APP: i32 = 3;

/// Fresh Word instance (doesn't touch the user's open documents). Prints "P <0..1>" progress, "E <msg>" per failure.
/// Opening a PDF needs DoNotPromptForConvert (else the hidden reflow prompt fails the open); the user's value is
/// restored before Quit since Word persists options.
const WORD: &str = r#"
$ErrorActionPreference = 'Stop'
$ins = $env:ORBIT_IN -split "`n"; $outs = $env:ORBIT_OUT -split "`n"; $fmt = [int]$env:ORBIT_FMT
try { $app = New-Object -ComObject Word.Application } catch { exit 3 }
$prompt = $app.Options.DoNotPromptForConvert
try {
  $app.Visible = $false; $app.DisplayAlerts = 0; $app.Options.DoNotPromptForConvert = $true
  for ($i = 0; $i -lt $ins.Count; $i++) {
    try {
      $d = $app.Documents.Open($ins[$i], $false, $true, $false, 'orbit-no-password')
      try { $d.SaveAs2($outs[$i], $fmt) } finally { $d.Close(0) }
    } catch { "E $($_.Exception.Message)" }
    "P $(($i + 1) / $ins.Count)"
  }
} finally { $app.Options.DoNotPromptForConvert = $prompt; $app.Quit(); [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) }
"#;

/// PowerPoint is single-instance: only Quit it if we were the ones who started it.
const POWERPOINT: &str = r#"
$ErrorActionPreference = 'Stop'
$ins = $env:ORBIT_IN -split "`n"; $outs = $env:ORBIT_OUT -split "`n"; $mode = $env:ORBIT_FMT
try { $app = New-Object -ComObject PowerPoint.Application } catch { exit 3 }
$own = $app.Presentations.Count -eq 0
try {
  for ($i = 0; $i -lt $ins.Count; $i++) {
    try {
      $p = $app.Presentations.Open($ins[$i], -1, 0, 0)
      try {
        if ($mode -eq 'pdf') { $p.SaveAs($outs[$i], 32) }
        else {
          $n = $p.Slides.Count; $w = 1920; $h = [int](1920 * $p.PageSetup.SlideHeight / $p.PageSetup.SlideWidth)
          for ($s = 1; $s -le $n; $s++) {
            $p.Slides.Item($s).Export((Join-Path $outs[$i] "Slide $s.$mode"), $mode.ToUpper(), $w, $h)
            "P $(($i + $s / $n) / $ins.Count)"
          }
        }
      } finally { $p.Close() }
    } catch { "E $($_.Exception.Message)" }
    "P $(($i + 1) / $ins.Count)"
  }
} finally { if ($own) { $app.Quit() }; [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) }
"#;

pub fn run(id: &str, paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    // (script, script format arg, output extension; "" = a folder of slide images)
    let (script, fmt, out_ext) = match id {
        "pdf.to_docx" => (WORD, "16", "docx"),
        "doc.to_pdf" => (WORD, "17", "pdf"),
        "slides.to_pdf" => (POWERPOINT, "pdf", "pdf"),
        "slides.png" => (POWERPOINT, "png", ""),
        _ => (POWERPOINT, "jpg", ""),
    };
    // Reserve every output up front (empty placeholder / folder) so two inputs never pick the same name.
    let mut outs = Vec::new();
    for p in paths {
        let out = if out_ext.is_empty() {
            let d = free_path(parent(p), &format!("{} slides", stem(p)), "");
            std::fs::create_dir(&d).map(|_| d)
        } else {
            let f = output_path(p, "", out_ext);
            std::fs::File::create_new(&f).map(|_| f)
        };
        outs.push(out.map_err(|e| format!("Couldn't create the output ({e})."))?);
    }
    let result = powershell(script, fmt, paths, &outs, progress);
    let missing = matches!(result, Ok(NO_APP));
    let mut errors = result.err().into_iter().collect::<Vec<_>>();
    let mut done = Vec::new();
    for (src, out) in paths.iter().zip(outs) {
        let ok = if out_ext.is_empty() {
            std::fs::read_dir(&out).map(|mut d| d.next().is_some()).unwrap_or(false)
        } else {
            produced(&out) || (missing && libreoffice(src, &out, out_ext).is_ok())
        };
        if ok {
            done.push(out);
        } else {
            let _ = if out_ext.is_empty() { std::fs::remove_dir_all(&out) } else { std::fs::remove_file(&out) };
            if !missing {
                errors.push(format!("{} couldn't be converted.", name(src)));
            }
        }
    }
    if done.is_empty() {
        return Err(if missing {
            if out_ext.is_empty() { "Needs Microsoft PowerPoint." } else { "Needs Microsoft Office or LibreOffice." }.into()
        } else {
            errors.into_iter().next().unwrap_or_else(|| "The conversion failed.".into())
        });
    }
    let label = match out_ext {
        "" => "Slides exported".to_string(),
        e => format!("Saved as {}", e.to_uppercase()),
    };
    let msg = match paths.len() - done.len() {
        0 => label,
        n => format!("{label} · {n} failed"),
    };
    Ok(Outcome::new(msg, done))
}

fn produced(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
}

/// Runs a script in a hidden PowerShell; returns its exit code. Kills it on timeout.
fn powershell(script: &str, fmt: &str, ins: &[PathBuf], outs: &[PathBuf], progress: Progress) -> Result<i32, String> {
    // Office resolves relative paths against its own working folder, so always hand it absolute ones.
    let join = |v: &[PathBuf]| {
        let abs = |p: &PathBuf| std::path::absolute(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned();
        v.iter().map(abs).collect::<Vec<_>>().join("\n")
    };
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg("& ([scriptblock]::Create($env:ORBIT_PS))")
        .env("ORBIT_PS", script)
        .env("ORBIT_IN", join(ins))
        .env("ORBIT_OUT", join(outs))
        .env("ORBIT_FMT", fmt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "Couldn't start PowerShell.".to_string())?;
    let stdout = child.stdout.take().unwrap();
    let limit = TIMEOUT * ins.len() as u32;
    std::thread::scope(|s| {
        s.spawn(|| {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(f) = line.strip_prefix("P ").and_then(|f| f.trim().parse::<f32>().ok()) {
                    progress(f.clamp(0.0, 1.0));
                }
            }
        });
        wait(&mut child, limit).ok_or_else(|| "Office took too long and was stopped.".to_string())
    })
}

/// Waits up to `limit`; kills the process and returns None on timeout.
fn wait(child: &mut Child, limit: Duration) -> Option<i32> {
    let start = Instant::now();
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            return Some(s.code().unwrap_or(1));
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn soffice() -> Option<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|d| PathBuf::from(d).join(r"LibreOffice\program\soffice.exe"))
        .find(|p| p.exists())
}

/// `soffice --headless --convert-to` into a private temp folder, then moved onto the reserved `out`.
fn libreoffice(src: &Path, out: &Path, to: &str) -> Result<(), String> {
    let exe = soffice().ok_or("LibreOffice not found")?;
    let tmp = std::env::temp_dir().join(format!("orbit-lo-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    if ext(src) == "pdf" {
        cmd.arg("--infilter=writer_pdf_import");
    }
    let filter = if to == "docx" { "docx:MS Word 2007 XML" } else { to };
    let child = cmd
        .args(["--headless", "--norestore", "--convert-to", filter, "--outdir"])
        .arg(&tmp)
        .arg(src)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let made = tmp.join(format!("{}.{to}", stem(src)));
    let ok = child.ok().and_then(|mut c| wait(&mut c, TIMEOUT)).is_some()
        && std::fs::copy(&made, out).is_ok()
        && produced(out);
    let _ = std::fs::remove_dir_all(&tmp);
    if ok { Ok(()) } else { Err("LibreOffice couldn't convert the file".into()) }
}
