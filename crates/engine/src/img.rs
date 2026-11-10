//! Image conversion, resizing and compression (pure Rust via `image`).

use crate::{files, name, saved};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use orbit_core::{ext, output_path, Outcome};
use std::io::Cursor;
use std::path::Path;

/// Canonical spelling of an image extension ("jpeg" -> "jpg", "tiff" -> "tif").
pub fn norm(e: &str) -> &str {
    match e {
        "jpeg" => "jpg",
        "tiff" => "tif",
        e => e,
    }
}

/// Decodes and applies the EXIF orientation, so stripping metadata never rotates a photo.
pub fn load(p: &Path) -> Result<DynamicImage, String> {
    let bad = || format!("{} couldn't be read as an image.", name(p));
    let mut dec = ImageReader::open(p)
        .map_err(|_| bad())?
        .with_guessed_format()
        .map_err(|_| bad())?
        .into_decoder()
        .map_err(|_| bad())?;
    let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|_| bad())?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Encodes to `fmt` (normalised extension). JPEG flattens alpha onto white; WEBP is lossless.
pub fn encode(img: DynamicImage, fmt: &str, quality: u8) -> Result<Vec<u8>, String> {
    use DynamicImage::*;
    let img = match img {
        ImageLuma8(_) | ImageLumaA8(_) | ImageRgb8(_) | ImageRgba8(_) => img,
        i if i.color().has_alpha() => ImageRgba8(i.into_rgba8()),
        i => ImageRgb8(i.into_rgb8()),
    };
    let mut buf = Cursor::new(Vec::new());
    let r = match fmt {
        "jpg" => match img {
            ImageLuma8(g) => JpegEncoder::new_with_quality(&mut buf, quality).encode_image(&g),
            i => JpegEncoder::new_with_quality(&mut buf, quality).encode_image(&flatten(i)),
        },
        "png" => img.write_with_encoder(PngEncoder::new_with_quality(&mut buf, CompressionType::Best, PngFilter::Adaptive)),
        "webp" => img.write_with_encoder(WebPEncoder::new_lossless(&mut buf)),
        "gif" => ImageRgba8(img.into_rgba8()).write_to(&mut buf, ImageFormat::Gif),
        "bmp" if img.color().has_alpha() => img.write_to(&mut buf, ImageFormat::Bmp),
        "bmp" => ImageRgb8(img.into_rgb8()).write_to(&mut buf, ImageFormat::Bmp),
        "tif" => img.write_to(&mut buf, ImageFormat::Tiff),
        _ => return Err("This image format can't be written.".into()),
    };
    r.map_err(|e| format!("Couldn't encode the image ({e})."))?;
    Ok(buf.into_inner())
}

fn flatten(img: DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.into_rgb8();
    }
    let rgba = img.into_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        image::Rgb([0, 1, 2].map(|i| ((p[i] as u32 * a + 255 * (255 - a) + 127) / 255) as u8))
    })
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

pub fn convert(p: &Path, to: &str) -> Result<Outcome, String> {
    let data = encode(load(p)?, to, 92)?;
    let out = output_path(p, "", to);
    write(&out, &data)?;
    Ok(Outcome::new(format!("Saved as {}", to.to_uppercase()), vec![out]))
}

/// `long_edge` None = half size. Keeps aspect ratio and never upscales.
pub fn resize(p: &Path, long_edge: Option<u32>) -> Result<Outcome, String> {
    let img = load(p)?;
    let long = img.width().max(img.height());
    let target = long_edge.unwrap_or(long / 2).max(1);
    if target >= long {
        return Ok(Outcome::new("Already that small", Vec::new()));
    }
    let small = img.resize(target, target, FilterType::Lanczos3);
    let msg = format!("Resized to {}×{}", small.width(), small.height());
    let e = ext(p);
    let data = encode(small, norm(&e), 90)?;
    let out = output_path(p, "_resized", &e);
    write(&out, &data)?;
    Ok(Outcome::new(msg, vec![out]))
}

/// Same format, smaller file: JPEG q75 (metadata dropped), PNG max deflate, WEBP lossless.
pub fn compress(p: &Path) -> Result<Outcome, String> {
    let e = ext(p);
    let data = encode(load(p)?, norm(&e), 75)?;
    let before = files::len(p);
    if data.len() as u64 >= before {
        return Ok(Outcome::new("Already optimized", Vec::new()));
    }
    let out = output_path(p, "_compressed", &e);
    write(&out, &data)?;
    Ok(Outcome::new(saved(before, data.len() as u64), vec![out]))
}
