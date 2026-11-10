//! PDF actions on top of lopdf: compress, split, merge, extract images.

use crate::{files, img, name, parent, saved, stem};
use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use orbit_core::{free_path, output_path, Outcome, Progress};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::core::HSTRING;
use windows::Win32::UI::Shell::StrCmpLogicalW;

/// Images whose long edge exceeds this are downscaled when compressing.
const MAX_EDGE: u32 = 2000;
const JPEG_QUALITY: u8 = 70;

/// Loads a PDF. `write`: refuse encrypted files (we'd have to strip or re-apply their protection).
fn load(p: &Path, write: bool) -> Result<Document, String> {
    let doc = Document::load(p).map_err(|_| format!("{} couldn't be read as a PDF.", name(p)))?;
    if doc.is_encrypted() && (write || doc.encryption_state.is_none()) {
        return Err(format!("{} is password-protected.", name(p)));
    }
    Ok(doc)
}

fn save(doc: &mut Document) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    doc.save_modern(&mut buf).map_err(|e| format!("Couldn't write the PDF ({e})."))?;
    Ok(buf)
}

fn write(out: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let r = std::fs::File::create_new(out).and_then(|mut f| f.write_all(data).and_then(|_| f.sync_all()));
    r.map_err(|e| {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(out); // a partial file is worse than none
        }
        format!("Couldn't save {} ({e}).", name(out))
    })
}

fn int(d: &Dictionary, key: &[u8]) -> Option<i64> {
    d.get(key).and_then(Object::as_i64).ok()
}

fn is_image(o: &Object) -> bool {
    matches!(o, Object::Stream(s) if s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image"))
}

/// DeviceRGB/Gray-like colour spaces whose samples map 1:1 onto RGB/L pixels.
fn plain_color(d: &Dictionary) -> bool {
    match d.get(b"ColorSpace") {
        Ok(Object::Name(n)) => matches!(n.as_slice(), b"DeviceRGB" | b"DeviceGray" | b"CalRGB" | b"CalGray"),
        Ok(Object::Array(a)) => matches!(a.first().and_then(|o| o.as_name().ok()), Some(b"ICCBased" | b"CalRGB" | b"CalGray")),
        _ => false,
    }
}

/// Decodes 8-bit RGB/Gray images stored as JPEG, Flate or raw. Anything exotic => None (left untouched).
fn decode(s: &Stream) -> Option<DynamicImage> {
    let d = &s.dict;
    if !plain_color(d) || d.has(b"Decode") || int(d, b"BitsPerComponent") != Some(8) {
        return None;
    }
    let (w, h) = (int(d, b"Width")? as u32, int(d, b"Height")? as u32);
    match s.filters().unwrap_or_default().as_slice() {
        [b"DCTDecode"] => {
            let i = image::load_from_memory_with_format(&s.content, ImageFormat::Jpeg).ok()?;
            matches!(i, DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_)).then_some(i)
        }
        [] | [b"FlateDecode"] => {
            let mut raw = s.get_plain_content().ok()?;
            let px = w as usize * h as usize;
            if px == 0 {
                None
            } else if raw.len() >= px * 3 && raw.len() < px * 4 {
                raw.truncate(px * 3);
                RgbImage::from_raw(w, h, raw).map(DynamicImage::ImageRgb8)
            } else if raw.len() >= px && raw.len() < px * 2 {
                raw.truncate(px);
                GrayImage::from_raw(w, h, raw).map(DynamicImage::ImageLuma8)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Re-encodes one image XObject as a (possibly downscaled) JPEG if that makes it smaller.
fn shrink(s: &mut Stream) {
    if s.content.len() < 16 * 1024 {
        return;
    }
    let Some(mut i) = decode(s) else { return };
    if i.width().max(i.height()) > MAX_EDGE {
        i = i.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::CatmullRom);
    }
    let (w, h) = (i.width(), i.height());
    let Ok(jpeg) = img::encode(i, "jpg", JPEG_QUALITY) else { return };
    if jpeg.len() < s.content.len() {
        s.dict.set("Width", w as i64);
        s.dict.set("Height", h as i64);
        s.dict.set("BitsPerComponent", 8);
        s.dict.set("Filter", "DCTDecode");
        s.dict.remove(b"DecodeParms");
        s.set_content(jpeg);
    }
}

/// Ids reachable from `start`. `one_page`: don't climb `/Parent` or wander into other pages.
fn reachable(doc: &Document, start: &Object, one_page: bool) -> HashSet<ObjectId> {
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    while let Some(o) = stack.pop() {
        let d = match o {
            Object::Reference(id) => {
                if let Ok(t) = doc.get_object(*id) {
                    let page = matches!(t.type_name(), Ok(b"Page" | b"Pages"));
                    if !(one_page && page) && seen.insert(*id) {
                        stack.push(t);
                    }
                }
                continue;
            }
            Object::Array(a) => {
                stack.extend(a);
                continue;
            }
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        stack.extend(d.iter().filter(|(k, _)| !(one_page && k.as_slice() == b"Parent")).map(|(_, v)| v));
    }
    seen
}

fn prune(doc: &mut Document) {
    let keep = reachable(doc, &Object::Dictionary(doc.trailer.clone()), false);
    doc.objects.retain(|id, _| keep.contains(id));
}

/// The page dictionary with inheritable attributes copied down from its ancestors.
fn flat_page(doc: &Document, id: ObjectId) -> Dictionary {
    let mut page = doc.get_dictionary(id).cloned().unwrap_or_default();
    let mut up = page.get(b"Parent").and_then(Object::as_reference).ok();
    while let Some(pid) = up {
        let Ok(node) = doc.get_dictionary(pid) else { break };
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            if !page.has(key) {
                if let Ok(v) = node.get(key) {
                    page.set(key.to_vec(), v.clone());
                }
            }
        }
        up = node.get(b"Parent").and_then(Object::as_reference).ok().filter(|&n| n != pid);
    }
    page
}

/// Gives `doc` a fresh page tree + catalog holding exactly `pages`.
fn set_pages(doc: &mut Document, pages: Vec<(ObjectId, Dictionary)>) {
    let tree = doc.new_object_id();
    let kids: Vec<Object> = pages.iter().map(|(id, _)| Object::Reference(*id)).collect();
    let count = kids.len() as i64;
    for (id, mut d) in pages {
        d.set("Parent", tree);
        doc.objects.insert(id, Object::Dictionary(d));
    }
    doc.objects.insert(tree, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => tree });
    doc.trailer = dictionary! { "Root" => catalog };
}

pub fn compress(p: &Path, progress: Progress) -> Result<Outcome, String> {
    let before = files::len(p);
    let mut doc = load(p, true)?;
    prune(&mut doc);
    let total = doc.objects.values().filter(|o| is_image(o)).count().max(1);
    let done = AtomicUsize::new(0);
    doc.objects.par_iter_mut().filter(|(_, o)| is_image(o)).for_each(|(_, o)| {
        if let Object::Stream(s) = o {
            shrink(s);
        }
        progress(0.9 * (done.fetch_add(1, Ordering::Relaxed) + 1) as f32 / total as f32);
    });
    doc.compress();
    let data = save(&mut doc)?;
    if data.len() as u64 * 100 > before * 99 {
        // Under 1% isn't worth a second file.
        return Ok(Outcome::new("Already optimized", Vec::new()));
    }
    let out = output_path(p, "_compressed", "pdf");
    write(&out, &data)?;
    Ok(Outcome::new(saved(before, data.len() as u64), vec![out]))
}

/// One PDF per page in "<name> pages".
pub fn split(p: &Path, progress: Progress) -> Result<Outcome, String> {
    let doc = load(p, true)?;
    let pages = doc.get_pages();
    if pages.len() < 2 {
        return Err("This PDF has only one page.".into());
    }
    let dir = free_path(parent(p), &format!("{} pages", stem(p)), "");
    fs::create_dir(&dir).map_err(|e| format!("Couldn't create the folder ({e})."))?;
    let digits = pages.len().to_string().len();
    for (n, &id) in &pages {
        let page = flat_page(&doc, id);
        let mut one = Document::with_version(doc.version.clone());
        for oid in reachable(&doc, &Object::Dictionary(page.clone()), true) {
            if oid != id {
                one.objects.insert(oid, doc.get_object(oid).cloned().unwrap_or(Object::Null));
            }
        }
        one.max_id = doc.max_id;
        set_pages(&mut one, vec![(id, page)]);
        let data = save(&mut one)?;
        write(&dir.join(format!("{} - page {:0digits$}.pdf", stem(p), n)), &data)?;
        progress(*n as f32 / pages.len() as f32);
    }
    Ok(Outcome::new(format!("Split into {} pages", pages.len()), vec![dir]))
}

/// Explorer's ordering ("2.pdf" before "10.pdf").
fn natural(a: &Path, b: &Path) -> std::cmp::Ordering {
    let r = unsafe { StrCmpLogicalW(&HSTRING::from(name(a)), &HSTRING::from(name(b))) };
    r.cmp(&0)
}

pub fn merge(paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    let mut paths = paths.to_vec();
    paths.sort_by(|a, b| natural(a, b));
    let mut out = Document::with_version("1.7");
    let mut pages = Vec::new();
    let mut next = 1;
    for (i, p) in paths.iter().enumerate() {
        let mut doc = load(p, true)?;
        doc.renumber_objects_with(next);
        next = doc.objects.keys().map(|k| k.0).max().unwrap_or(next) + 1;
        for (_, id) in doc.get_pages() {
            pages.push((id, flat_page(&doc, id)));
        }
        out.objects.extend(doc.objects);
        progress(0.8 * (i + 1) as f32 / paths.len() as f32);
    }
    out.max_id = next;
    let count = pages.len();
    set_pages(&mut out, pages);
    prune(&mut out); // drops the old catalogs, page trees and outlines
    let data = save(&mut out)?;
    let target = free_path(parent(&paths[0]), "Merged", "pdf");
    write(&target, &data)?;
    Ok(Outcome::new(format!("Merged {} files · {count} pages", paths.len()), vec![target]))
}

/// JPEGs (and JPEG 2000) are copied byte-for-byte; Flate/raw RGB or Gray become PNG. Soft masks are skipped.
pub fn images(p: &Path, progress: Progress) -> Result<Outcome, String> {
    let doc = load(p, false)?;
    let masks: HashSet<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok()?.dict.get(b"SMask").and_then(Object::as_reference).ok())
        .collect();
    let dir = free_path(parent(p), &format!("{} images", stem(p)), "");
    let all: Vec<(&ObjectId, &Stream)> = doc
        .objects
        .iter()
        .filter(|(id, o)| is_image(o) && !masks.contains(id))
        .filter_map(|(id, o)| Some((id, o.as_stream().ok()?)))
        .collect();
    let mut n = 0;
    for (i, (_, s)) in all.iter().enumerate() {
        let (data, ext) = match s.filters().unwrap_or_default().as_slice() {
            [b"DCTDecode"] => (s.content.clone(), "jpg"),
            [b"JPXDecode"] => (s.content.clone(), "jp2"),
            _ => match decode(s).map(|i| img::encode(i, "png", 0)) {
                Some(Ok(d)) => (d, "png"),
                _ => continue,
            },
        };
        if n == 0 {
            fs::create_dir(&dir).map_err(|e| format!("Couldn't create the folder ({e})."))?;
        }
        n += 1;
        write(&dir.join(format!("Image {n}.{ext}")), &data)?;
        progress((i + 1) as f32 / all.len() as f32);
    }
    if n == 0 {
        return Err("No extractable images in this PDF.".into());
    }
    Ok(Outcome::new(format!("{n} images extracted"), vec![dir]))
}
