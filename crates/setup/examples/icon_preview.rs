//! Renders the logo into target\icon-preview\ for visual inspection (transparent PNGs + a light/dark contact sheet).
use image::{imageops, Rgba, RgbaImage};

fn png(size: u32) -> RgbaImage {
    let b = orbit_setup::icon_bitmap(size);
    RgbaImage::from_fn(size, size, |x, y| {
        let p = &b.bgra[((y * size + x) * 4) as usize..][..4];
        let un = |c: u8| if p[3] == 0 { 0 } else { (c as u32 * 255 / p[3] as u32) as u8 };
        Rgba([un(p[2]), un(p[1]), un(p[0]), p[3]])
    })
}

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\target\icon-preview");
    std::fs::create_dir_all(&dir).unwrap();
    for s in [16, 32, 48, 256] {
        png(s).save(dir.join(format!("orbit-{s}.png"))).unwrap();
    }
    for s in [16, 32] {
        imageops::resize(&png(s), s * 8, s * 8, imageops::Nearest).save(dir.join(format!("orbit-{s}-x8.png"))).unwrap();
    }
    // Contact sheet: light taskbar row, dark taskbar row; small sizes at 1x then 16/32 at x8.
    let mut sheet = RgbaImage::new(980, 2 * 280);
    for (row, bg) in [[243u8, 243, 243, 255], [32, 32, 32, 255]].into_iter().enumerate() {
        let y0 = row as i64 * 280;
        for y in 0..280 {
            for x in 0..sheet.width() {
                sheet.put_pixel(x, y0 as u32 + y, Rgba(bg));
            }
        }
        let mut x = 672i64;
        for s in [16u32, 20, 24, 32, 40, 48, 64] {
            imageops::overlay(&mut sheet, &png(s), x, y0 + 120);
            x += s as i64 + 8;
        }
        imageops::overlay(&mut sheet, &imageops::resize(&png(16), 128, 128, imageops::Nearest), 8, y0 + 76);
        imageops::overlay(&mut sheet, &imageops::resize(&png(32), 256, 256, imageops::Nearest), 144, y0 + 12);
        imageops::overlay(&mut sheet, &png(256), 408, y0 + 12);
    }
    sheet.save(dir.join("sheet.png")).unwrap();
}
