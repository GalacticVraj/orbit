//! File conversion engine: PDF, Office, image, video and folder actions. Everything runs locally.

mod files;
mod img;
mod office;
mod pdf;
mod video;

use orbit_core::{ext, human_size, kind_of, Action, Kind, Outcome, Progress};
use std::path::{Path, PathBuf};

/// Actions applicable to this selection (single file, folder, or many files — possibly mixed kinds).
/// Pure and fast: decide from extensions / is_dir only, never open file contents.
pub fn actions_for(paths: &[PathBuf]) -> Vec<Action> {
    let kinds: Vec<Kind> = paths.iter().map(|p| kind_of(p)).collect();
    let Some(&kind) = kinds.first() else { return Vec::new() };
    let exts: Vec<String> = paths.iter().map(|p| img::norm(&ext(p)).to_string()).collect();
    let all = |f: &dyn Fn(&str) -> bool| exts.iter().all(|e| f(e));
    let many = paths.len() > 1;
    let a = Action::new;
    let mut v = Vec::new();
    if kinds.iter().all(|&k| k == kind) {
        match kind {
            Kind::Pdf => {
                if many {
                    v.push(a("pdf.merge", "Merge PDFs", '\u{E8C8}'));
                }
                v.push(a("pdf.compress", "Compress PDF", '\u{E73F}'));
                if !many {
                    v.push(a("pdf.split", "Split Pages", '\u{E8C6}'));
                    v.push(a("pdf.images", "Extract Images", '\u{E91B}'));
                }
                v.push(a("pdf.to_docx", "Convert to Word", '\u{E8A5}'));
            }
            Kind::Word => {
                v.push(a("doc.to_pdf", "Convert to PDF", '\u{EA90}'));
                if all(&|e| e == "docx") {
                    v.push(a("doc.images", "Extract Images", '\u{E91B}'));
                }
            }
            Kind::Slides => {
                v.push(a("slides.to_pdf", "Convert to PDF", '\u{EA90}'));
                v.push(Action::group(
                    "Export Slides",
                    '\u{E786}',
                    vec![a("slides.png", "PNG", '\u{E91B}'), a("slides.jpg", "JPG", '\u{E91B}')],
                ));
            }
            Kind::Image => {
                let to: Vec<Action> = [("jpg", "JPG"), ("png", "PNG"), ("webp", "WEBP")]
                    .into_iter()
                    .filter(|(e, _)| !all(&|x| x == *e))
                    .map(|(e, l)| a(&format!("img.to_{e}"), &format!("To {l}"), '\u{E8AB}'))
                    .collect();
                v.push(Action::group("Convert", '\u{E8AB}', to));
                v.push(Action::group(
                    "Resize",
                    '\u{E740}',
                    vec![
                        a("img.resize50", "50%", '\u{E740}'),
                        a("img.resize1920", "1920 px", '\u{E740}'),
                        a("img.resize1280", "1280 px", '\u{E740}'),
                        a("img.resize800", "800 px", '\u{E740}'),
                    ],
                ));
                if all(&|e| matches!(e, "jpg" | "png" | "webp")) {
                    v.push(a("img.compress", "Compress", '\u{E73F}'));
                }
            }
            Kind::Video => {
                v.push(a("video.audio", "Extract Audio", '\u{E8D6}'));
                v.push(a("video.compress", "Compress Video", '\u{E73F}'));
                if !all(&|e| e == "mp4") {
                    v.push(a("video.mp4", "Convert to MP4", '\u{E714}'));
                }
            }
            Kind::Folder | Kind::Other => {}
        }
    }
    if many || kind == Kind::Folder {
        v.push(a("zip", "Zip", '\u{F012}'));
        v.push(a("size", "Calculate Size", '\u{EDA2}'));
    }
    v
}

/// Execute action `id` (one of the ids returned by `actions_for`) on `paths`.
/// Runs on a worker thread that is already COM STA-initialised. Never overwrite inputs:
/// write next to the source via `orbit_core::output_path`. Errors are short, human sentences.
pub fn run(id: &str, paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    if paths.is_empty() {
        return Err("Nothing is selected.".into());
    }
    type Each = fn(&Path, Progress) -> Result<Outcome, String>;
    let f: Each = match id {
        "pdf.merge" => return pdf::merge(paths, progress),
        "pdf.to_docx" | "doc.to_pdf" | "slides.to_pdf" | "slides.png" | "slides.jpg" => {
            return office::run(id, paths, progress)
        }
        "zip" => return files::zip(paths, progress),
        "size" => return Ok(files::size(paths)),
        "pdf.compress" => pdf::compress,
        "pdf.split" => pdf::split,
        "pdf.images" => pdf::images,
        "doc.images" => |p, _| files::docx_images(p),
        "img.to_jpg" => |p, _| img::convert(p, "jpg"),
        "img.to_png" => |p, _| img::convert(p, "png"),
        "img.to_webp" => |p, _| img::convert(p, "webp"),
        "img.resize50" => |p, _| img::resize(p, None),
        "img.resize1920" => |p, _| img::resize(p, Some(1920)),
        "img.resize1280" => |p, _| img::resize(p, Some(1280)),
        "img.resize800" => |p, _| img::resize(p, Some(800)),
        "img.compress" => |p, _| img::compress(p),
        "video.audio" => |p, pr| video::run(p, video::Mode::Audio, pr),
        "video.compress" => |p, pr| video::run(p, video::Mode::Compress, pr),
        "video.mp4" => |p, pr| video::run(p, video::Mode::Mp4, pr),
        _ => return Err("This action isn't available.".into()),
    };
    // Conversions to a format a file already has are skipped in batches.
    let paths: Vec<&PathBuf> = match id.strip_prefix("img.to_") {
        Some(to) if paths.len() > 1 => paths.iter().filter(|p| img::norm(&ext(p)) != to).collect(),
        _ => paths.iter().collect(),
    };
    each(id, &paths, progress, f)
}

/// Runs `f` on every file, keeps going past failures, and sums the result up in one line.
fn each(id: &str, paths: &[&PathBuf], progress: Progress, f: fn(&Path, Progress) -> Result<Outcome, String>) -> Result<Outcome, String> {
    if let [p] = paths {
        return f(p, progress);
    }
    let n = paths.len().max(1) as f32;
    let (mut reveal, mut errors, mut first_msg) = (Vec::new(), Vec::new(), None);
    let (mut before, mut after) = (0u64, 0u64);
    for (i, p) in paths.iter().enumerate() {
        let sub = |x: f32| progress((i as f32 + x) / n);
        match f(p, &sub) {
            Ok(o) => {
                if id.ends_with("compress") && !o.reveal.is_empty() {
                    before += files::len(p);
                    after += o.reveal.iter().map(|r| files::len(r)).sum::<u64>();
                }
                first_msg.get_or_insert(o.message);
                reveal.extend(o.reveal);
            }
            Err(e) => errors.push(e),
        }
        progress((i + 1) as f32 / n);
    }
    if reveal.is_empty() {
        return match (errors.into_iter().next(), first_msg) {
            (Some(e), _) => Err(e),
            (None, m) => Ok(Outcome::new(m.unwrap_or_else(|| "Nothing to do".into()), Vec::new())),
        };
    }
    let mut msg = if before > 0 { saved(before, after) } else { format!("Done · {} files", reveal.len()) };
    if !errors.is_empty() {
        msg += &format!(" · {} failed", errors.len());
    }
    Ok(Outcome::new(msg, reveal))
}

/// "Saved 4.2 MB (63%)".
fn saved(before: u64, after: u64) -> String {
    let d = before.saturating_sub(after);
    format!("Saved {} ({}%)", human_size(d), d * 100 / before.max(1))
}

/// File name for error messages.
fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn parent(p: &Path) -> &Path {
    p.parent().unwrap_or(Path::new("."))
}
