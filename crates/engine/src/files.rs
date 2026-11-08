//! Folder and archive actions: Zip, Calculate Size, DOCX image extraction.

use crate::{name, parent, stem};
use orbit_core::{free_path, human_size, Outcome, Progress};
use std::fs::{self, File, Metadata};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub fn len(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Depth-first walk that never follows symlinks or junctions.
fn walk(p: &Path, f: &mut dyn FnMut(&Path, &Metadata)) {
    let Ok(m) = fs::symlink_metadata(p) else { return };
    if m.file_type().is_symlink() {
        return;
    }
    f(p, &m);
    if m.is_dir() {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                walk(&e.path(), f);
            }
        }
    }
}

pub fn size(paths: &[PathBuf]) -> Outcome {
    let (mut bytes, mut files) = (0u64, 0u64);
    for p in paths {
        walk(p, &mut |_, m| {
            if m.is_file() {
                bytes += m.len();
                files += 1;
            }
        });
    }
    let unit = if files == 1 { "file" } else { "files" };
    Outcome::new(format!("{} · {} {unit}", human_size(bytes), thousands(files)), Vec::new())
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One item => "<name>.zip"; many => "<parent folder>.zip" (or "Archive.zip"). Entries keep their top-level names.
pub fn zip(paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    let dir = parent(&paths[0]);
    let base = match paths {
        [p] if p.is_dir() => name(p),
        [p] => stem(p),
        _ => dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Archive".into()),
    };
    let out = free_path(dir, &base, "zip");
    let mut items: Vec<(PathBuf, String, Option<u64>)> = Vec::new(); // (path, entry name, size if file)
    for p in paths {
        let root = parent(p);
        walk(p, &mut |q, m| {
            let rel = q.strip_prefix(root).unwrap_or(q).to_string_lossy().replace('\\', "/");
            items.push((q.to_path_buf(), rel, m.is_file().then(|| m.len())));
        });
    }
    let result = write_zip(&out, &items, progress);
    if result.is_err() {
        let _ = fs::remove_file(&out);
    }
    result.map_err(|e| format!("Couldn't create the zip: {e}"))?;
    Ok(Outcome::new(format!("Zipped · {}", human_size(len(&out))), vec![out]))
}

fn write_zip(out: &Path, items: &[(PathBuf, String, Option<u64>)], progress: Progress) -> Result<(), String> {
    let total: u64 = items.iter().filter_map(|i| i.2).sum::<u64>().max(1);
    let mut zw = ZipWriter::new(File::create_new(out).map_err(|e| e.to_string())?);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    for (path, rel, size) in items {
        let Some(size) = size else {
            zw.add_directory(rel.as_str(), opts).map_err(|e| e.to_string())?;
            continue;
        };
        let mut f = File::open(path).map_err(|e| format!("{} can't be read ({e})", name(path)))?;
        zw.start_file(rel.as_str(), opts.large_file(*size >= u32::MAX as u64)).map_err(|e| e.to_string())?;
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            zw.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(done as f32 / total as f32);
        }
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// A .docx is a zip; its pictures live in word/media/.
pub fn docx_images(p: &Path) -> Result<Outcome, String> {
    let bad = |_| format!("{} isn't a valid .docx file.", name(p));
    let mut zip = ZipArchive::new(File::open(p).map_err(|e| e.to_string())?).map_err(bad)?;
    let dir = free_path(parent(p), &format!("{} images", stem(p)), "");
    let mut n = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(bad)?;
        let entry_name = entry.name().to_string();
        let Some(file) = entry_name.strip_prefix("word/media/").filter(|f| !f.contains(['/', '\\', ':']) && !matches!(*f, "" | "." | "..")) else {
            continue;
        };
        if n == 0 {
            fs::create_dir(&dir).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        fs::write(dir.join(file), data).map_err(|e| e.to_string())?;
        n += 1;
    }
    if n == 0 {
        return Err("This document has no images.".into());
    }
    Ok(Outcome::new(format!("{n} images extracted"), vec![dir]))
}
