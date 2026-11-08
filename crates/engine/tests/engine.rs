//! End-to-end tests of every pure-Rust action on generated fixtures.

use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use lopdf::{dictionary, Document, Object, Stream};
use orbit_engine::{actions_for, run};
use std::io::Write;
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("orbit-engine-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn go(id: &str, paths: &[PathBuf]) -> orbit_core::Outcome {
    run(id, paths, &|f| assert!((0.0..=1.0).contains(&f), "progress {f}")).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Noisy photo-like image so JPEG/PNG sizes behave like real photos.
fn photo(w: u32, h: u32) -> RgbImage {
    let mut seed = 1u32;
    RgbImage::from_fn(w, h, |x, y| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let n = (seed >> 24) as u8 / 8;
        Rgb([(x * 255 / w) as u8 ^ n, (y * 255 / h) as u8, ((x + y) % 256) as u8 / 2 + n])
    })
}

fn jpeg(img: &RgbImage, q: u8) -> Vec<u8> {
    let mut v = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut v, q).encode_image(img).unwrap();
    v
}

/// `pages` pages labelled "<tag> page N"; first page carries a big JPEG and a raw RGB image.
fn make_pdf(path: &Path, tag: &str, pages: usize, images: bool) {
    let mut doc = Document::with_version("1.5");
    let tree = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let mut kids = Vec::new();
    for n in 1..=pages {
        let mut xobjects = lopdf::Dictionary::new();
        let mut ops = format!("BT /F1 24 Tf 50 750 Td ({tag} page {n}) Tj ET\n");
        if images && n == 1 {
            let big = photo(3000, 2000);
            let dct = doc.add_object(Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 3000, "Height" => 2000,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode" },
                jpeg(&big, 95),
            ));
            let raw = doc.add_object(Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 300, "Height" => 200,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
                photo(300, 200).into_raw(),
            ));
            xobjects.set("Im1", dct);
            xobjects.set("Im2", raw);
            ops += "q 400 0 0 266 50 400 cm /Im1 Do Q q 150 0 0 100 50 200 cm /Im2 Do Q\n";
        }
        let content = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => tree, "Contents" => content,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font }, "XObject" => xobjects },
        });
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(
        tree,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages as i64,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()] }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => tree });
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

fn page_text(doc: &Document, n: u32) -> String {
    String::from_utf8_lossy(&doc.get_page_content(doc.get_pages()[&n]).unwrap()).into_owned()
}

fn ids(actions: &[orbit_core::Action]) -> Vec<String> {
    actions.iter().flat_map(|a| std::iter::once(a.id.clone()).chain(a.children.iter().map(|c| c.id.clone()))).collect()
}

#[test]
fn offers_the_right_actions() {
    let d = dir("actions");
    let pdf = ids(&actions_for(&[d.join("a.pdf")]));
    assert_eq!(pdf, ["pdf.compress", "pdf.split", "pdf.images", "pdf.to_docx"]);
    let pdfs = ids(&actions_for(&[d.join("a.pdf"), d.join("b.PDF")]));
    assert!(pdfs.contains(&"pdf.merge".into()) && pdfs.contains(&"zip".into()) && !pdfs.contains(&"pdf.split".into()));
    let png = ids(&actions_for(&[d.join("a.png")]));
    assert!(png.contains(&"img.to_jpg".into()) && !png.contains(&"img.to_png".into()) && png.contains(&"img.compress".into()));
    let jpegs = ids(&actions_for(&[d.join("a.jpeg"), d.join("b.JPG")]));
    assert!(!jpegs.contains(&"img.to_jpg".into()) && jpegs.contains(&"img.to_webp".into()));
    assert!(!ids(&actions_for(&[d.join("a.bmp")])).contains(&"img.compress".into()));
    assert!(!ids(&actions_for(&[d.join("a.mp4")])).contains(&"video.mp4".into()));
    assert!(ids(&actions_for(&[d.join("a.mkv")])).contains(&"video.mp4".into()));
    assert_eq!(ids(&actions_for(&[d.join("a.pdf"), d.join("b.png")])), ["zip", "size"]);
    assert_eq!(ids(&actions_for(&[d.clone()])), ["zip", "size"]);
    assert!(ids(&actions_for(&[d.join("x.doc")])) == ["doc.to_pdf"]);
    assert!(ids(&actions_for(&[d.join("x.pptx")])).contains(&"slides.jpg".into()));
    assert!(actions_for(&[d.join("x.txt")]).is_empty() && actions_for(&[]).is_empty());
    assert!(actions_for(&[d.join("a.png")]).len() <= 8);
}

#[test]
fn images_convert_resize_compress() {
    let d = dir("images");
    let png = d.join("alpha.png");
    RgbaImage::from_fn(400, 300, |x, _| if x < 10 { Rgba([0, 0, 0, 0]) } else { Rgba([200, 10, 10, 255]) })
        .save(&png)
        .unwrap();
    let jpg = d.join("photo.jpg");
    std::fs::write(&jpg, jpeg(&photo(2400, 1600), 98)).unwrap();

    let o = go("img.to_jpg", &[png.clone()]);
    let out = image::open(&o.reveal[0]).unwrap().to_rgb8();
    assert_eq!(o.reveal[0], d.join("alpha.jpg"));
    assert!(out.get_pixel(2, 2).0.iter().all(|&c| c > 240), "transparent -> white");

    let o = go("img.to_webp", &[jpg.clone()]);
    assert_eq!(image::ImageFormat::from_path(&o.reveal[0]).unwrap(), ImageFormat::WebP);
    assert_eq!(image::open(&o.reveal[0]).unwrap().width(), 2400);

    let o = go("img.resize1920", &[jpg.clone()]);
    let r = image::open(&o.reveal[0]).unwrap();
    assert_eq!((r.width(), r.height()), (1920, 1280));
    assert_eq!(o.reveal[0], d.join("photo_resized.jpg"));
    let o = go("img.resize50", &[png.clone()]);
    assert_eq!(image::open(&o.reveal[0]).unwrap().width(), 200);
    let o = go("img.resize1920", &[png.clone()]);
    assert!(o.reveal.is_empty(), "never upscales");

    let o = go("img.compress", &[jpg.clone()]);
    assert!(o.message.starts_with("Saved"), "{}", o.message);
    assert!(std::fs::metadata(&o.reveal[0]).unwrap().len() < std::fs::metadata(&jpg).unwrap().len());
    let again = go("img.compress", &[o.reveal[0].clone()]);
    assert_eq!(again.message, "Already optimized");

    // Batch: the one already-PNG file is skipped, outputs never overwrite.
    let o = go("img.to_png", &[jpg.clone(), png.clone(), d.join("alpha.jpg")]);
    assert_eq!(o.reveal, [d.join("photo.png"), d.join("alpha (2).png")]);
    assert!(run("img.to_png", &[d.join("missing.jpg")], &|_| {}).is_err());
}

#[test]
fn pdf_compress_split_merge_extract() {
    let d = dir("pdf");
    let big = d.join("report.pdf");
    make_pdf(&big, "report", 3, true);

    let o = go("pdf.compress", &[big.clone()]);
    assert!(o.message.starts_with("Saved"), "{}", o.message);
    let small = Document::load(&o.reveal[0]).unwrap();
    assert_eq!(small.get_pages().len(), 3);
    assert!(page_text(&small, 2).contains("report page 2"));
    let widths: Vec<i64> = small
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter_map(|s| s.dict.get(b"Width").and_then(Object::as_i64).ok())
        .collect();
    assert!(widths.contains(&2000), "big JPEG downscaled: {widths:?}");
    assert!(std::fs::metadata(&o.reveal[0]).unwrap().len() * 2 < std::fs::metadata(&big).unwrap().len());

    let plain = d.join("plain.pdf");
    make_pdf(&plain, "plain", 1, false);
    let o = go("pdf.compress", &[o.reveal[0].clone()]);
    assert_eq!(o.message, "Already optimized");
    assert!(run("pdf.split", &[plain.clone()], &|_| {}).is_err());

    let o = go("pdf.split", &[big.clone()]);
    assert_eq!(o.reveal, [d.join("report pages")]);
    let mut parts: Vec<_> = std::fs::read_dir(&o.reveal[0]).unwrap().map(|e| e.unwrap().path()).collect();
    parts.sort();
    assert_eq!(parts.len(), 3);
    for (i, p) in parts.iter().enumerate() {
        let one = Document::load(p).unwrap();
        assert_eq!(one.get_pages().len(), 1);
        assert!(page_text(&one, 1).contains(&format!("report page {}", i + 1)));
    }
    // Page 2 must not drag page 1's big image along.
    assert!(std::fs::metadata(&parts[1]).unwrap().len() < 10_000);

    let (two, ten) = (d.join("2.pdf"), d.join("10.pdf"));
    make_pdf(&two, "two", 2, false);
    make_pdf(&ten, "ten", 3, false);
    let o = go("pdf.merge", &[ten.clone(), two.clone()]);
    let merged = Document::load(&o.reveal[0]).unwrap();
    assert_eq!(merged.get_pages().len(), 5);
    assert!(page_text(&merged, 1).contains("two page 1"), "natural order: 2 before 10");
    assert!(page_text(&merged, 5).contains("ten page 3"));

    let o = go("pdf.images", &[big.clone()]);
    let mut names: Vec<_> = std::fs::read_dir(&o.reveal[0]).unwrap().map(|e| e.unwrap().file_name()).collect();
    names.sort();
    assert_eq!(names, ["Image 1.jpg", "Image 2.png"]);
    let jpg = image::open(o.reveal[0].join("Image 1.jpg")).unwrap();
    assert_eq!((jpg.width(), jpg.height()), (3000, 2000));
    assert!(run("pdf.images", &[plain], &|_| {}).is_err());
}

#[test]
fn folders_zip_size_docx_images() {
    let d = dir("folders");
    let f = d.join("Project");
    std::fs::create_dir_all(f.join("sub/empty")).unwrap();
    std::fs::write(f.join("a.txt"), vec![b'a'; 5000]).unwrap();
    std::fs::write(f.join("sub/b.bin"), vec![7u8; 3000]).unwrap();

    let o = go("size", &[f.clone()]);
    assert_eq!(o.message, "7.8 KB · 2 files");
    assert!(o.reveal.is_empty());

    let o = go("zip", &[f.clone()]);
    assert_eq!(o.reveal, [d.join("Project.zip")]);
    let mut z = zip::ZipArchive::new(std::fs::File::open(&o.reveal[0]).unwrap()).unwrap();
    let mut names: Vec<String> = z.file_names().map(String::from).collect();
    names.sort();
    assert_eq!(names, ["Project/", "Project/a.txt", "Project/sub/", "Project/sub/b.bin", "Project/sub/empty/"]);
    assert_eq!(z.by_name("Project/sub/b.bin").unwrap().size(), 3000);

    // Mixed selection => "<parent>.zip" with each item at the root.
    let o = go("zip", &[f.join("a.txt"), f.join("sub")]);
    assert_eq!(o.reveal, [f.join("Project.zip")]);
    assert_eq!(zip::ZipArchive::new(std::fs::File::open(&o.reveal[0]).unwrap()).unwrap().len(), 4);

    // Minimal .docx: a zip with word/media/*.
    let docx = d.join("letter.docx");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&docx).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file("word/document.xml", opts).unwrap();
    w.write_all(b"<w:document/>").unwrap();
    for n in ["image1.png", "image2.jpeg"] {
        w.start_file(format!("word/media/{n}"), opts).unwrap();
        w.write_all(b"data").unwrap();
    }
    w.finish().unwrap();
    let o = go("doc.images", &[docx]);
    assert_eq!(o.reveal, [d.join("letter images")]);
    assert!(d.join("letter images/image2.jpeg").exists());
}
