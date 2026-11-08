//! Types shared by every Orbit crate. Kept dependency-free on purpose.

use std::path::{Path, PathBuf};

/// One node on the ring. `children` non-empty => clicking opens a sub-ring instead of executing.
#[derive(Clone, Debug, Default)]
pub struct Action {
    /// Stable id passed back on pick. Engine ids are plain ("pdf.compress"),
    /// plugin ids are "plugin:<plugin>:<action>", rename ids are "rename.*".
    pub id: String,
    /// Short label, shown when the node is hovered (e.g. "Compress PDF").
    pub label: String,
    /// Segoe Fluent Icons / Segoe MDL2 Assets codepoint, e.g. '\u{E8A5}'.
    pub glyph: char,
    pub children: Vec<Action>,
}

impl Action {
    pub fn new(id: &str, label: &str, glyph: char) -> Self {
        Action { id: id.into(), label: label.into(), glyph, children: Vec::new() }
    }
    pub fn group(label: &str, glyph: char, children: Vec<Action>) -> Self {
        Action { id: String::new(), label: label.into(), glyph, children }
    }
}

/// Result of a finished action.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// One short line shown in the ring centre, e.g. "Saved 4.2 MB" or "1.24 GB".
    pub message: String,
    /// Files/folders produced; Orbit selects them in Explorer afterwards.
    pub reveal: Vec<PathBuf>,
}

impl Outcome {
    pub fn new(message: impl Into<String>, reveal: Vec<PathBuf>) -> Self {
        Outcome { message: message.into(), reveal }
    }
}

/// Reports progress in 0.0..=1.0. Called from worker threads.
pub type Progress<'a> = &'a (dyn Fn(f32) + Sync);

/// 32-bit premultiplied BGRA pixels, row-major, stride = width * 4.
#[derive(Clone, Debug, Default)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Word,
    Slides,
    Image,
    Video,
    Folder,
    Other,
}

pub fn kind_of(path: &Path) -> Kind {
    if path.is_dir() {
        return Kind::Folder;
    }
    match ext(path).as_str() {
        "pdf" => Kind::Pdf,
        "docx" | "doc" | "rtf" | "odt" => Kind::Word,
        "pptx" | "ppt" | "odp" => Kind::Slides,
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff" => Kind::Image,
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "webm" | "m4v" | "3gp" => Kind::Video,
        _ => Kind::Other,
    }
}

/// Lower-case extension without the dot ("" if none).
pub fn ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// `dir/stem<suffix>.<new_ext>`, adding " (2)", " (3)"... until the name is free.
/// Every action writes through this so nothing is ever overwritten.
pub fn output_path(src: &Path, suffix: &str, new_ext: &str) -> PathBuf {
    let dir = src.parent().unwrap_or(Path::new("."));
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    free_path(dir, &format!("{stem}{suffix}"), new_ext)
}

/// First non-existing `dir/base.ext`, `dir/base (2).ext`, ... (`ext` empty => no extension, e.g. folders).
pub fn free_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let name = |n: u32| {
        let b = if n == 1 { base.to_string() } else { format!("{base} ({n})") };
        if ext.is_empty() { b } else { format!("{b}.{ext}") }
    };
    (1..).map(|n| dir.join(name(n))).find(|p| !p.exists()).unwrap()
}

/// "1.2 MB" style formatting.
pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", U[i]) }
}

/// %APPDATA%\Orbit (created on demand).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join("Orbit");
    let _ = std::fs::create_dir_all(&d);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(kind_of(Path::new("a/B.JPG")), Kind::Image);
        let d = std::env::temp_dir().join("orbit-core-test");
        let _ = std::fs::create_dir_all(&d);
        let f = d.join("x.txt");
        std::fs::write(&f, "1").unwrap();
        assert_eq!(free_path(&d, "x", "txt"), d.join("x (2).txt"));
        assert_eq!(output_path(&f, "_c", "pdf"), d.join("x_c.pdf"));
    }
}
