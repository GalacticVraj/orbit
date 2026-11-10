//! Microsoft Store logos, drawn from the same procedural logo as the app icon.
//!   cargo run -p orbit-setup --example store_assets -- <dir>
use std::path::Path;

/// The logo, `fill` of the smaller side, centred on a transparent w x h canvas.
fn save(dir: &Path, file: &str, w: u32, h: u32, fill: f32) {
    let n = ((w.min(h) as f32) * fill).round() as u32;
    let icon = orbit_setup::icon_bitmap(n);
    let mut img = image::RgbaImage::new(w, h);
    let (ox, oy) = ((w - n) / 2, (h - n) / 2);
    for y in 0..n {
        for x in 0..n {
            let i = ((y * n + x) * 4) as usize;
            let [b, g, r, a] = [icon.bgra[i], icon.bgra[i + 1], icon.bgra[i + 2], icon.bgra[i + 3]];
            let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 }; // PNG wants straight alpha
            img.put_pixel(ox + x, oy + y, image::Rgba([un(r), un(g), un(b), a]));
        }
    }
    img.save(dir.join(file)).expect("save logo");
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: store_assets <dir>");
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    for sc in [100u32, 200, 400] {
        let k = sc / 100;
        save(dir, &format!("StoreLogo.scale-{sc}.png"), 50 * k, 50 * k, 1.0);
        save(dir, &format!("Square44x44Logo.scale-{sc}.png"), 44 * k, 44 * k, 1.0);
        save(dir, &format!("Square150x150Logo.scale-{sc}.png"), 150 * k, 150 * k, 0.66);
        if sc <= 200 {
            save(dir, &format!("Wide310x150Logo.scale-{sc}.png"), 310 * k, 150 * k, 0.66);
        }
    }
    for t in [16u32, 24, 32, 48, 256] {
        for suffix in ["", "_altform-unplated", "_altform-lightunplated"] {
            save(dir, &format!("Square44x44Logo.targetsize-{t}{suffix}.png"), t, t, 1.0);
        }
    }
    println!("wrote {}", dir.display());
}
