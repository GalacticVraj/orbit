//! Renders key ring states over a wallpaper into target\snapshots\*.png (offscreen; for visual review).
//!   cargo run -p orbit-ring --example snapshot

use orbit_core::{Action, Bitmap, Outcome};
use orbit_ring::{snapshot, Step};
use std::path::PathBuf;
use windows::core::{Result, HSTRING};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

fn main() -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let wic: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let screen = load(&wic, r"C:\Windows\Web\4K\Wallpaper\Windows\img0_1920x1200.jpg", 1600, 1000).unwrap_or_else(|_| gradient(1600, 1000));
    let photo = load(&wic, r"C:\Windows\Web\4K\Wallpaper\Windows\img19_1920x1200.jpg", 192, 120).ok().map(|b| crop(&b, 32, 0, 128, 120));
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"..\..\target\snapshots");
    std::fs::create_dir_all(&out).ok();

    let one = [PathBuf::from(r"C:\Users\me\Documents\Quarterly report 2026 - final.pdf")];
    let many: Vec<PathBuf> = (1..=12).map(|i| PathBuf::from(format!(r"C:\pics\IMG_{i:04}.jpg"))).collect();
    let doc = Some(doc_icon());
    let shots: Vec<(&str, (i32, i32), u32, bool, &[PathBuf], Option<Bitmap>, Vec<Step>)> = vec![
        ("01-dark-open", (520, 420), 144, true, &one, doc.clone(), vec![Step::Wait(1.0)]),
        ("02-light-open", (1150, 380), 144, false, &one, doc.clone(), vec![Step::Wait(1.0)]),
        ("03-dark-opening-60ms", (520, 420), 144, true, &one, doc.clone(), vec![Step::Wait(0.06)]),
        ("04-dark-hover", (820, 620), 144, true, &one, doc.clone(), vec![Step::Wait(1.0), Step::Hover(Some(1)), Step::Wait(0.6)]),
        ("05-light-hover", (420, 760), 144, false, &one, doc.clone(), vec![Step::Wait(1.0), Step::Hover(Some(5)), Step::Wait(0.6)]),
        ("06-sub-ring", (900, 300), 144, true, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(2), Step::Wait(1.0), Step::HoverCenter, Step::Wait(0.5)]),
        ("07-sub-ring-transition", (900, 300), 144, true, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(2), Step::Wait(0.09)]),
        ("08-spinner", (600, 500), 144, true, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(0), Step::Wait(0.8)]),
        ("09-progress", (600, 500), 144, false, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(0), Step::Wait(0.4), Step::Progress(0.62), Step::Wait(1.0)]),
        ("10-success", (600, 500), 144, true, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(0), Step::Progress(0.7), Step::Wait(0.5), Step::Finish(Ok(Outcome::new("Saved 4.2 MB", vec![]))), Step::Wait(1.2)]),
        ("11-success-pulse", (600, 500), 144, false, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(0), Step::Progress(0.7), Step::Wait(0.5), Step::Finish(Ok(Outcome::new("Saved 4.2 MB", vec![]))), Step::Wait(0.35)]),
        ("12-error", (600, 500), 144, true, &one, doc.clone(), vec![Step::Wait(1.0), Step::Click(0), Step::Wait(0.3), Step::Finish(Err("This PDF is password-protected, so it can't be compressed.".into())), Step::Wait(1.5)]),
        ("13-multi-badge", (1000, 700), 144, true, &many, photo.clone(), vec![Step::Wait(1.0)]),
        ("14-multi-badge-light-100", (1000, 700), 96, false, &many, photo.clone(), vec![Step::Wait(1.0), Step::Hover(Some(7)), Step::Wait(0.6)]),
        ("15-hint", (700, 450), 144, true, &[], None, vec![Step::Wait(1.0)]),
    ];
    for (name, at, dpi, dark, paths, icon, steps) in shots {
        let acts = if paths.is_empty() { vec![] } else if paths.len() > 1 { image_actions() } else { pdf_actions() };
        let img = snapshot(&screen, at, dpi, dark, paths, icon, acts, steps);
        save(&wic, &img, &out.join(format!("{name}.png")))?;
    }
    println!("wrote {}", out.display());
    Ok(())
}

pub fn pdf_actions() -> Vec<Action> {
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

fn image_actions() -> Vec<Action> {
    let mut v: Vec<Action> = [
        ("img.resize", "Resize images", '\u{E740}'),
        ("img.webp", "Convert to WebP", '\u{E8AB}'),
        ("img.pdf", "Combine into PDF", '\u{EA90}'),
        ("img.zip", "Compress to ZIP", '\u{F012}'),
        ("img.strip", "Remove metadata", '\u{E74D}'),
        ("img.rotate", "Rotate", '\u{E7AD}'),
        ("rename.batch", "Batch rename", '\u{E8AC}'),
    ]
    .iter()
    .map(|&(id, l, g)| Action::new(id, l, g))
    .collect();
    v.push(Action::new("img.share", "Share", '\u{E72D}'));
    v.push(Action::new("img.copy", "Copy paths", '\u{E8C8}')); // 9 nodes => "More" overflow
    v
}

fn load(wic: &IWICImagingFactory, path: &str, w: u32, h: u32) -> Result<Bitmap> {
    unsafe {
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&dec.GetFrame(0)?, w, h, WICBitmapInterpolationModeFant)?;
        let conv = wic.CreateFormatConverter()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut bgra)?;
        Ok(Bitmap { width: w, height: h, bgra })
    }
}

fn save(wic: &IWICImagingFactory, b: &Bitmap, path: &std::path::Path) -> Result<()> {
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), 0x4000_0000 /* GENERIC_WRITE */)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.SetSize(b.width, b.height)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(b.height, b.width * 4, &b.bgra)?;
        frame.Commit()?;
        enc.Commit()
    }
}

fn crop(b: &Bitmap, x: u32, y: u32, w: u32, h: u32) -> Bitmap {
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for r in y..y + h {
        let i = ((r * b.width + x) * 4) as usize;
        bgra.extend_from_slice(&b.bgra[i..i + (w * 4) as usize]);
    }
    Bitmap { width: w, height: h, bgra }
}

fn gradient(w: u32, h: u32) -> Bitmap {
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            bgra.extend([(255.0 * v) as u8, (120.0 + 100.0 * u) as u8, (255.0 * (1.0 - u)) as u8, 255]);
        }
    }
    Bitmap { width: w, height: h, bgra }
}

/// Stand-in for a shell file icon: a white page with a folded corner and a red PDF band.
fn doc_icon() -> Bitmap {
    let n = 128;
    let mut bgra = vec![0u8; n * n * 4];
    let cover = |x: f32, y: f32| {
        // Rounded page 24..104 x 8..120, folded corner cut at top-right.
        let (dx, dy) = ((24.0 - x).max(x - 104.0).max(0.0), (8.0 - y).max(y - 120.0).max(0.0));
        let inside = (6.0 - dx.hypot(dy)).clamp(0.0, 1.0) * ((24.0 - x).max(x - 104.0).max((8.0 - y).max(y - 120.0)) <= 0.0) as u8 as f32;
        let fold = ((x - 80.0) - (y - 8.0) - 24.0).clamp(-0.5, 0.5) + 0.5; // 1 beyond the diagonal
        (inside * (1.0 - fold)).clamp(0.0, 1.0)
    };
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let a = cover(fx, fy);
            let mut c = [250.0, 250.0, 250.0];
            if (62.0..86.0).contains(&fy) && (24.0..96.0).contains(&fx) {
                c = [48.0, 56.0, 214.0]; // BGR red band
            } else if fx > 80.0 && fy < 32.0 && (fx - 80.0) <= (fy - 8.0) + 24.0 && (fx - 80.0) > (fy - 8.0) {
                c = [210.0, 210.0, 210.0]; // folded flap shading
            }
            let i = (y * n + x) * 4;
            for k in 0..3 {
                bgra[i + k] = (c[k] * a) as u8;
            }
            bgra[i + 3] = (255.0 * a) as u8;
        }
    }
    Bitmap { width: n as u32, height: n as u32, bgra }
}
