//! Rendering: theme, frosted backdrop, and the Direct2D scene drawn into a 32bpp premultiplied DIB.

use crate::ring::{angle, Phase, Ring, CENTER_R, NODE_R, ORBIT, R_COMPACT, R_GLASS};
use orbit_core::Bitmap;
use std::f32::consts::{PI, TAU};
use windows::core::{w, Result, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;

type Rgb = [f32; 3];
const BLACK: Rgb = [0.0, 0.0, 0.0];
const WHITE: Rgb = [1.0, 1.0, 1.0];
const ICON: f32 = 50.0; // centre icon box, DIP

// ---------- theme ----------

pub struct Theme {
    pub dark: bool,
    pub glass: bool,
    pub accent: Rgb,
}

fn reg(key: PCWSTR, value: PCWSTR, buf: &mut [u8]) -> Option<usize> {
    let mut n = buf.len() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, value, RRF_RT_ANY, None, Some(buf.as_mut_ptr().cast()), Some(&mut n))
    };
    (r == ERROR_SUCCESS).then_some(n as usize)
}

fn reg_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut b = [0u8; 4];
    (reg(key, value, &mut b) == Some(4)).then(|| u32::from_le_bytes(b))
}

impl Theme {
    pub fn system() -> Theme {
        let p = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let dark = reg_dword(p, w!("AppsUseLightTheme")) == Some(0);
        let glass = reg_dword(p, w!("EnableTransparency")) != Some(0);
        Theme { dark, glass, accent: Self::accent(dark) }
    }

    /// Win11 uses a lighter accent shade on dark surfaces and a darker one on light surfaces.
    pub fn accent(dark: bool) -> Rgb {
        let to = |r: u8, g: u8, b: u8| [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
        let mut pal = [0u8; 32];
        let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");
        if reg(key, w!("AccentPalette"), &mut pal) == Some(32) {
            let i = if dark { 4 } else { 16 }; // Light2 / Dark1
            return to(pal[i], pal[i + 1], pal[i + 2]);
        }
        if let Some(c) = reg_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor")) {
            return to(c as u8, (c >> 8) as u8, (c >> 16) as u8);
        }
        if dark { to(0x60, 0xCD, 0xFF) } else { to(0x00, 0x5F, 0xB8) }
    }

    fn fg(&self) -> Rgb {
        if self.dark { WHITE } else { BLACK }
    }
    fn on_accent(&self) -> Rgb {
        let [r, g, b] = self.accent;
        if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5 { BLACK } else { WHITE }
    }
    fn result(&self, ok: bool) -> Rgb {
        match (ok, self.dark) {
            (true, true) => [0.42, 0.80, 0.37],
            (true, false) => [0.06, 0.48, 0.06],
            (false, true) => [1.0, 0.60, 0.64],
            (false, false) => [0.77, 0.17, 0.11],
        }
    }
}

// ---------- frosted backdrop ----------

/// Screen pixels (BGRA, `w`x`h`) -> 1/`f`-size, blurred, slightly saturated opaque bitmap.
pub fn frost(px: &[u8], w: usize, h: usize, f: usize) -> Bitmap {
    let (bw, bh) = ((w / f).max(1), (h / f).max(1));
    let mut buf = vec![[0f32; 3]; bw * bh];
    for (i, o) in buf.iter_mut().enumerate() {
        let (x0, y0) = ((i % bw) * f, (i / bw) * f);
        let mut n = 0.0f32;
        for y in y0..(y0 + f).min(h) {
            for x in x0..(x0 + f).min(w) {
                let p = &px[(y * w + x) * 4..];
                for c in 0..3 {
                    o[c] += p[c] as f32;
                }
                n += 1.0;
            }
        }
        o.iter_mut().for_each(|c| *c /= n.max(1.0));
    }
    let mut tmp = buf.clone();
    for _ in 0..3 {
        blur_pass(&buf, &mut tmp, bw, bh, 1, bw, 4);
        blur_pass(&tmp, &mut buf, bh, bw, bw, 1, 4);
    }
    let mut bgra = Vec::with_capacity(bw * bh * 4);
    for [b, g, r] in buf {
        let l = 0.0722 * b + 0.7152 * g + 0.2126 * r;
        for c in [b, g, r] {
            bgra.push((l + (c - l) * 1.4).clamp(0.0, 255.0) as u8);
        }
        bgra.push(255);
    }
    Bitmap { width: bw as u32, height: bh as u32, bgra }
}

/// One box-blur pass along lines of `len` samples (`step` apart); `lines` lines `stride` apart. Edges clamp.
fn blur_pass(src: &[[f32; 3]], dst: &mut [[f32; 3]], len: usize, lines: usize, step: usize, stride: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for l in 0..lines {
        let at = |i: isize| src[l * stride + (i.clamp(0, len as isize - 1) as usize) * step];
        let mut acc = [0f32; 3];
        for i in -(r as isize)..=r as isize {
            let p = at(i);
            (0..3).for_each(|c| acc[c] += p[c]);
        }
        for i in 0..len {
            dst[l * stride + i * step] = acc.map(|a| a * norm);
            let (add, sub) = (at(i as isize + r as isize + 1), at(i as isize - r as isize));
            (0..3).for_each(|c| acc[c] += add[c] - sub[c]);
        }
    }
}

/// Area-average resize (premultiplied BGRA), used to pre-scale the icon to device pixels.
fn resize(b: &Bitmap, w: u32, h: u32) -> Bitmap {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let (fx, fy) = (b.width as f32 / w as f32, b.height as f32 / h as f32);
    for y in 0..h {
        let (y0, y1) = ((y as f32 * fy) as u32, (((y + 1) as f32 * fy).ceil() as u32).clamp(1, b.height));
        for x in 0..w {
            let (x0, x1) = ((x as f32 * fx) as u32, (((x + 1) as f32 * fx).ceil() as u32).clamp(1, b.width));
            let mut acc = [0u32; 4];
            let mut n = 0;
            for sy in y0.min(y1 - 1)..y1 {
                for sx in x0.min(x1 - 1)..x1 {
                    let i = ((sy * b.width + sx) * 4) as usize;
                    (0..4).for_each(|c| acc[c] += b.bgra[i + c] as u32);
                    n += 1;
                }
            }
            out.extend(acc.map(|a| (a / n.max(1)) as u8));
        }
    }
    Bitmap { width: w, height: h, bgra: out }
}

// ---------- device-independent resources (kept for the life of the thread) ----------

pub struct Gfx {
    d2d: ID2D1Factory,
    round: ID2D1StrokeStyle,
    name: IDWriteTextFormat,
    label: IDWriteTextFormat,
    msg: IDWriteTextFormat,
    badge: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    big: IDWriteTextFormat,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut fonts = None;
            dw.GetSystemFontCollection(&mut fonts, false)?;
            let has = |name: PCWSTR| {
                let (mut i, mut ok) = (0, BOOL(0));
                fonts.as_ref().is_some_and(|f| f.FindFamilyName(name, &mut i, &mut ok).is_ok() && ok.as_bool())
            };
            let icons = if has(w!("Segoe Fluent Icons")) { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            let text = if has(w!("Segoe UI Variable Text")) { w!("Segoe UI Variable Text") } else { w!("Segoe UI") };
            let fmt = |family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT, wrap: bool| -> Result<IDWriteTextFormat> {
                let f = dw.CreateTextFormat(
                    family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"),
                )?;
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
                let sign = dw.CreateEllipsisTrimmingSign(&f)?;
                let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                f.SetTrimming(&trim, &sign)?;
                Ok(f)
            };
            let round = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    ..Default::default()
                },
                None,
            )?;
            Ok(Gfx {
                name: fmt(text, 12.0, DWRITE_FONT_WEIGHT_NORMAL, false)?,
                label: fmt(text, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?,
                msg: fmt(text, 12.5, DWRITE_FONT_WEIGHT_NORMAL, true)?,
                badge: fmt(text, 10.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?,
                glyph: fmt(icons, 17.0, DWRITE_FONT_WEIGHT_NORMAL, false)?,
                big: fmt(icons, 26.0, DWRITE_FONT_WEIGHT_NORMAL, false)?,
                d2d,
                round,
            })
        }
    }
}

// ---------- per-show surface: memory DC + DIB + DC render target ----------

pub struct Surface {
    pub dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    res: Option<Res>,
}

struct Res {
    rt: ID2D1DCRenderTarget,
    icon: Option<ID2D1Bitmap>,
    backdrop: Option<ID2D1Bitmap>,
    noise: ID2D1Bitmap,
}

impl Surface {
    pub fn new(w: i32, h: i32, scale: f32) -> Option<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(dib) = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, dib);
            Some(Surface { dc, dib, old, bits: bits.cast(), w, h, scale, res: None })
        }
    }

    pub fn pixels(&self) -> &[u8] {
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize)
        }
    }

    /// Copy the screen under the window (at `x`,`y`) and frost it.
    pub fn capture(&self, x: i32, y: i32) -> Bitmap {
        unsafe {
            let screen = GetDC(None);
            let _ = BitBlt(self.dc, 0, 0, self.w, self.h, screen, x, y, ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0));
            ReleaseDC(None, screen);
        }
        frost(self.pixels(), self.w as usize, self.h as usize, 4)
    }

    /// Draw `ring` into the DIB. False on failure (resources are rebuilt next time).
    pub fn render(&mut self, g: &Gfx, r: &Ring, t: &Theme, backdrop: Option<&Bitmap>) -> bool {
        if self.res.is_none() {
            self.res = unsafe { self.create(g, r.icon.as_ref(), backdrop) }.ok();
        }
        let Some(res) = &self.res else { return false };
        let ok = unsafe { scene(g, res, r, t, self.scale, self.w as f32 / self.scale) }.is_ok();
        if !ok {
            self.res = None;
        }
        ok
    }

    unsafe fn create(&self, g: &Gfx, icon: Option<&Bitmap>, backdrop: Option<&Bitmap>) -> Result<Res> {
        let pf = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
        // Software: no GPU driver loaded => tiny RAM and fast first frame; the ring is small.
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: pf,
            ..Default::default()
        };
        let rt = g.d2d.CreateDCRenderTarget(&props)?;
        rt.BindDC(self.dc, &RECT { left: 0, top: 0, right: self.w, bottom: self.h })?;
        let s = self.scale;
        rt.SetDpi(96.0 * s, 96.0 * s);
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE); // ClearType breaks per-pixel alpha
        // `dpi` says how many bitmap pixels make one inch, i.e. how big the bitmap is in DIPs.
        let bmp = |b: &Bitmap, dpi: f32| {
            let props = D2D1_BITMAP_PROPERTIES { pixelFormat: pf, dpiX: dpi, dpiY: dpi };
            let size = D2D_SIZE_U { width: b.width, height: b.height };
            rt.CreateBitmap(size, Some(b.bgra.as_ptr().cast()), b.width * 4, &props)
        };
        let icon = match icon.filter(|b| b.width > 0 && b.height > 0 && b.bgra.len() >= (b.width * b.height * 4) as usize) {
            Some(b) => {
                let k = (ICON * s).round() / b.width.max(b.height) as f32;
                let (w, h) = (((b.width as f32 * k).round() as u32).max(1), ((b.height as f32 * k).round() as u32).max(1));
                Some(bmp(&resize(b, w, h), 96.0 * s)?)
            }
            None => None,
        };
        let backdrop = match backdrop {
            Some(b) => Some(bmp(b, 96.0 * s * b.width as f32 / self.w as f32)?),
            None => None,
        };
        // Faint film grain, 1 bitmap px = 1 device px.
        let mut seed = 0x9E37_79B9u32;
        let noise: Vec<u8> = (0..64 * 64)
            .flat_map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let a = (seed & 7) as u8;
                if seed & 0x100 != 0 { [a, a, a, a] } else { [0, 0, 0, a] }
            })
            .collect();
        let noise = bmp(&Bitmap { width: 64, height: 64, bgra: noise }, 96.0 * s)?;
        Ok(Res { rt, icon, backdrop, noise })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.res = None;
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.dib);
            let _ = DeleteDC(self.dc);
        }
    }
}

// ---------- the scene ----------

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}
fn ell(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE {
    D2D1_ELLIPSE { point: pt(x, y), radiusX: r, radiusY: r }
}
fn scale(k: f32) -> Matrix3x2 {
    Matrix3x2 { M11: k, M22: k, ..Matrix3x2::identity() }
}
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn color(c: Rgb, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c[0], g: c[1], b: c[2], a: a.clamp(0.0, 1.0) }
}

/// Drawing helpers sharing one solid brush and a global opacity.
struct Pen<'a> {
    g: &'a Gfx,
    rt: &'a ID2D1RenderTarget,
    br: ID2D1SolidColorBrush,
    a: f32,
}

impl Pen<'_> {
    unsafe fn brush(&self, c: Rgb, a: f32) -> &ID2D1SolidColorBrush {
        self.br.SetColor(&color(c, a * self.a));
        &self.br
    }
    unsafe fn disc(&self, x: f32, y: f32, r: f32, c: Rgb, a: f32) {
        self.rt.FillEllipse(&ell(x, y, r), self.brush(c, a));
    }
    unsafe fn circle(&self, x: f32, y: f32, r: f32, w: f32, c: Rgb, a: f32) {
        self.rt.DrawEllipse(&ell(x, y, r), self.brush(c, a), w, None);
    }
    unsafe fn text(&self, s: &str, f: &IDWriteTextFormat, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32) {
        if a <= 0.004 || s.is_empty() {
            return;
        }
        let u: Vec<u16> = s.encode_utf16().collect();
        let rc = D2D_RECT_F { left: x - w / 2.0, top: y - h / 2.0, right: x + w / 2.0, bottom: y + h / 2.0 };
        self.rt.DrawText(&u, f, &rc, self.brush(c, a), D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }
    unsafe fn stops(&self, stops: &[(f32, Rgb, f32)]) -> Result<ID2D1GradientStopCollection> {
        let s: Vec<D2D1_GRADIENT_STOP> =
            stops.iter().map(|&(p, c, a)| D2D1_GRADIENT_STOP { position: p, color: color(c, a * self.a) }).collect();
        self.rt.CreateGradientStopCollection(&s, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
    }
    /// Disc filled with a radial falloff (shadows, glows).
    unsafe fn radial(&self, x: f32, y: f32, r: f32, stops: &[(f32, Rgb, f32)]) -> Result<()> {
        let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES { center: pt(x, y), radiusX: r, radiusY: r, ..Default::default() };
        let b = self.rt.CreateRadialGradientBrush(&props, None, &self.stops(stops)?)?;
        self.rt.FillEllipse(&ell(x, y, r), &b);
        Ok(())
    }
    /// Top-to-bottom gradient brush spanning a disc of radius `r`.
    unsafe fn vertical(&self, r: f32, stops: &[(f32, Rgb, f32)]) -> Result<ID2D1LinearGradientBrush> {
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(0.0, -r), endPoint: pt(0.0, r) };
        self.rt.CreateLinearGradientBrush(&props, None, &self.stops(stops)?)
    }
    unsafe fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, sweep: f32, w: f32, c: Rgb, a: f32) -> Result<()> {
        if sweep <= 0.002 {
            return Ok(());
        }
        if sweep >= TAU - 0.002 {
            self.circle(cx, cy, r, w, c, a);
            return Ok(());
        }
        let geo = self.g.d2d.CreatePathGeometry()?;
        let sink = geo.Open()?;
        let a1 = a0 + sweep;
        sink.BeginFigure(pt(cx + r * a0.cos(), cy + r * a0.sin()), D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: pt(cx + r * a1.cos(), cy + r * a1.sin()),
            size: D2D_SIZE_F { width: r, height: r },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if sweep > PI { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        self.rt.DrawGeometry(&geo, self.brush(c, a), w, &self.g.round);
        Ok(())
    }
    /// Polyline drawn on up to fraction `t` of its length (stroke-trim).
    unsafe fn trace(&self, pts: &[(f32, f32)], t: f32, w: f32, c: Rgb, a: f32) -> Result<()> {
        let segs: Vec<f32> = pts.windows(2).map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1)).collect();
        let mut left = segs.iter().sum::<f32>() * t.clamp(0.0, 1.0);
        if left <= 0.01 {
            return Ok(());
        }
        let geo = self.g.d2d.CreatePathGeometry()?;
        let sink = geo.Open()?;
        sink.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_HOLLOW);
        for (p, &len) in pts.windows(2).zip(&segs) {
            let k = (left / len).min(1.0);
            sink.AddLine(pt(p[0].0 + (p[1].0 - p[0].0) * k, p[0].1 + (p[1].1 - p[0].1) * k));
            left -= len;
            if left <= 0.0 {
                break;
            }
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        self.rt.DrawGeometry(&geo, self.brush(c, a), w, &self.g.round);
        Ok(())
    }
}

unsafe fn scene(g: &Gfx, res: &Res, r: &Ring, t: &Theme, s: f32, size: f32) -> Result<()> {
    let rt: &ID2D1RenderTarget = &res.rt;
    rt.BeginDraw();
    rt.Clear(Some(&D2D1_COLOR_F::default()));
    let open = r.open.x.max(0.0);
    let k = 0.86 + 0.14 * open;
    let c = size / 2.0;
    let world = scale(k) * Matrix3x2::translation(c + r.shake.x, c);
    rt.SetTransform(&world);
    let br = rt.CreateSolidColorBrush(&D2D1_COLOR_F::default(), None)?;
    let p = Pen { g, rt, br, a: open.min(1.0) };
    let px = 1.0 / (s * k); // one device pixel in pre-transform DIPs
    let fg = t.fg();
    let cp = r.compact.x.clamp(0.0, 1.0);
    let grow = r.grow.x;
    let rad = R_GLASS + (R_COMPACT + 22.0 * grow - R_GLASS) * cp;

    // Depth: wide ambient shadow + tight contact shadow.
    let sa = if t.dark { 0.42 } else { 0.20 };
    let e = rad + 34.0;
    p.radial(0.0, 12.0, e, &[(0.0, BLACK, sa), ((rad - 18.0) / e, BLACK, sa), ((rad + 6.0) / e, BLACK, sa * 0.35), (1.0, BLACK, 0.0)])?;
    let e = rad + 9.0;
    p.radial(0.0, 2.5, e, &[(0.0, BLACK, sa * 0.6), ((rad - 3.0) / e, BLACK, sa * 0.6), (1.0, BLACK, 0.0)])?;

    // Glass: frosted copy of what is behind, aligned to the screen regardless of the open-scale.
    let glass = t.glass && res.backdrop.is_some();
    if let (true, Some(bd)) = (glass, &res.backdrop) {
        let props = D2D1_BITMAP_BRUSH_PROPERTIES {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        };
        let b = rt.CreateBitmapBrush(bd, Some(&props), Some(&D2D1_BRUSH_PROPERTIES { opacity: (p.a * 1.8).min(1.0), transform: Matrix3x2::identity() }))?;
        b.SetTransform(&(Matrix3x2::translation(-(c + r.shake.x), -c) * scale(1.0 / k)));
        rt.FillEllipse(&ell(0.0, 0.0, rad), &b);
    }
    let (tint, ta) = if t.dark { ([0.10, 0.10, 0.12], 0.65) } else { ([0.975, 0.975, 0.985], 0.64) };
    p.disc(0.0, 0.0, rad, tint, if glass { ta } else { 1.0 });
    let sheen = p.vertical(rad, &[(0.0, WHITE, if t.dark { 0.07 } else { 0.30 }), (0.55, WHITE, 0.0)])?;
    rt.FillEllipse(&ell(0.0, 0.0, rad), &sheen);
    if glass {
        let props = D2D1_BITMAP_BRUSH_PROPERTIES {
            extendModeX: D2D1_EXTEND_MODE_WRAP,
            extendModeY: D2D1_EXTEND_MODE_WRAP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
        };
        let b = rt.CreateBitmapBrush(&res.noise, Some(&props), Some(&D2D1_BRUSH_PROPERTIES { opacity: p.a, transform: Matrix3x2::identity() }))?;
        rt.FillEllipse(&ell(0.0, 0.0, rad), &b);
    }
    // Hairlines: dark outer edge for separation, light inner edge brighter at the top.
    p.circle(0.0, 0.0, rad + px * 0.5, px, BLACK, if t.dark { 0.55 } else { 0.10 });
    let (hi, lo) = if t.dark { (0.20, 0.04) } else { (0.95, 0.40) };
    let edge = p.vertical(rad, &[(0.0, WHITE, hi), (1.0, WHITE, lo)])?;
    rt.DrawEllipse(&ell(0.0, 0.0, rad - px * 0.5), &edge, px, None);

    // ----- menu: centre disc + nodes -----
    let m = 1.0 - cp;
    if m > 0.004 {
        let ch = r.center_hover.x;
        let cr = CENTER_R * (1.0 - 0.04 * r.center_press.x);
        p.radial(0.0, 3.0, cr + 10.0, &[(0.0, BLACK, 0.16 * m), (cr / (cr + 10.0), BLACK, 0.12 * m), (1.0, BLACK, 0.0)])?;
        let fill = if t.dark { 0.05 + 0.04 * ch } else { 0.55 + 0.15 * ch };
        p.disc(0.0, 0.0, cr, WHITE, fill * m);
        let (hi, lo) = if t.dark { (0.16, 0.03) } else { (1.0, 0.5) };
        let edge = p.vertical(cr, &[(0.0, WHITE, hi * m), (1.0, WHITE, lo * m)])?;
        rt.DrawEllipse(&ell(0.0, 0.0, cr - px * 0.5), &edge, px, None);

        let n = r.ghost.len();
        for (i, &(glyph, group)) in r.ghost.iter().enumerate() {
            node(&p, t, &world, px, angle(i, n), r.ghost_t.x, 0.0, 0.0, glyph, group, m)?;
        }
        let n = r.nodes.len();
        for (i, nd) in r.nodes.iter().enumerate() {
            node(&p, t, &world, px, angle(i, n), nd.appear.x, nd.hover.x, nd.press.x, nd.glyph, nd.group, m)?;
        }
        rt.SetTransform(&world);

        let la = r.label_a.x.clamp(0.0, 1.0);
        let base = match r.group() {
            Some((glyph, label)) => {
                p.text(&glyph.to_string(), &g.big, 0.0, -12.0, 40.0, 40.0, fg, 0.88 * (1.0 - ch) * m);
                p.text("\u{E72B}", &g.big, 0.0, -12.0, 40.0, 40.0, fg, 0.88 * ch * m);
                label
            }
            None => {
                icon(&p, res, -13.0, m)?;
                if r.count > 1 {
                    badge(&p, t, r.count, tint, m)?;
                }
                &r.name
            }
        };
        p.text(base, &g.name, 0.0, 27.0, 98.0, 18.0, fg, 0.62 * (1.0 - la) * m);
        p.text(&r.label, &g.label, 0.0, 27.0, 100.0, 18.0, fg, 0.95 * la * m);
    }

    // ----- compact: hint / progress / result -----
    if cp > 0.004 {
        rt.SetTransform(&world);
        match r.phase {
            Phase::Hint => {
                p.text("\u{EC50}", &g.big, 0.0, -18.0, 40.0, 40.0, t.accent, cp);
                p.text("Select a file in Explorer", &g.msg, 0.0, 18.0, 112.0, 40.0, fg, 0.9 * cp);
            }
            Phase::Running | Phase::Done(_) => {
                let ok = r.phase != Phase::Done(false);
                let d = r.done.x.clamp(0.0, 1.0);
                let res_c = t.result(ok);
                let ring_r = rad - 13.0;
                p.circle(0.0, 0.0, ring_r, 3.0, fg, 0.10 * cp);
                p.arc(0.0, 0.0, ring_r, r.arc_start.x, r.arc_len.x.min(TAU), 3.0, mix(t.accent, res_c, d), cp)?;
                let pu = r.pulse.x;
                if pu > 0.001 && pu < 0.999 {
                    p.circle(0.0, 0.0, ring_r + 40.0 * pu, 2.5 * (1.0 - pu), res_c, 0.6 * (1.0 - pu) * cp);
                }
                let busy = cp * (1.0 - d);
                p.text(&r.picked.0.to_string(), &g.big, 0.0, -12.0, 40.0, 40.0, fg, 0.9 * busy);
                p.text(&r.picked.1, &g.label, 0.0, 22.0, 104.0, 18.0, fg, 0.8 * busy);
                if d > 0.004 {
                    let y = if r.message.is_empty() { 0.0 } else { -20.0 - 10.0 * grow };
                    if ok {
                        let pts = [(-11.0, y + 0.5), (-3.5, y + 8.0), (12.0, y - 8.5)];
                        p.trace(&pts, r.check.x, 3.2, res_c, cp)?;
                    } else {
                        p.text("\u{E7BA}", &g.big, 0.0, y, 40.0, 40.0, res_c, d * cp);
                    }
                    p.text(&r.message, &g.msg, 0.0, 17.0 + 4.0 * grow, 118.0 + 40.0 * grow, 50.0 + 18.0 * grow, fg, 0.92 * d * cp);
                }
            }
            Phase::Menu => {}
        }
    }
    rt.EndDraw(None, None)
}

#[allow(clippy::too_many_arguments)]
unsafe fn node(p: &Pen, t: &Theme, world: &Matrix3x2, px: f32, base: f32, a: f32, h: f32, pr: f32, glyph: char, group: bool, m: f32) -> Result<()> {
    if a <= 0.01 {
        return Ok(());
    }
    let ang = base - (1.0 - a) * 0.45; // slight angular sweep while blooming
    let (x, y) = (ORBIT * a * ang.cos(), ORBIT * a * ang.sin());
    let sc = a.clamp(0.0, 1.15) * (1.0 + 0.14 * h - 0.10 * pr);
    let al = (a * 1.6).clamp(0.0, 1.0) * m;
    p.rt.SetTransform(&(scale(sc) * Matrix3x2::translation(x, y) * *world));
    if h > 0.01 {
        let acc = t.accent;
        let e = NODE_R * 2.1;
        p.radial(0.0, 0.0, e, &[(0.0, acc, 0.6 * h * al), (0.5, acc, 0.45 * h * al), (0.72, acc, 0.14 * h * al), (1.0, acc, 0.0)])?;
    }
    let (fill, line) = if t.dark { (0.075, 0.11) } else { (0.62, 0.95) };
    p.disc(0.0, 0.0, NODE_R, WHITE, fill * al * (1.0 - h));
    p.disc(0.0, 0.0, NODE_R, t.accent, h * al);
    let px = px / sc;
    p.circle(0.0, 0.0, NODE_R - px * 0.5, px, WHITE, (line + (0.35 - line) * h) * al);
    if !t.dark {
        p.circle(0.0, 0.0, NODE_R + px * 0.5, px, BLACK, 0.06 * al);
    }
    let gc = mix(t.fg(), t.on_accent(), h);
    p.text(&glyph.to_string(), &p.g.glyph, 0.0, 0.0, NODE_R * 2.0, NODE_R * 2.0, gc, 0.9 * al);
    if group {
        p.arc(0.0, 0.0, NODE_R + 4.0, ang - 0.4, 0.8, 1.5, t.fg(), 0.5 * al)?;
    }
    Ok(())
}

unsafe fn icon(p: &Pen, res: &Res, cy: f32, a: f32) -> Result<()> {
    let Some(bmp) = &res.icon else {
        p.text("\u{E8A5}", &p.g.big, 0.0, cy, 48.0, 48.0, [0.5; 3], a);
        return Ok(());
    };
    let size = bmp.GetSize();
    let (l, t) = (-size.width / 2.0, cy - size.height / 2.0);
    let props = D2D1_BITMAP_BRUSH_PROPERTIES {
        extendModeX: D2D1_EXTEND_MODE_CLAMP,
        extendModeY: D2D1_EXTEND_MODE_CLAMP,
        interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    };
    let b = p.rt.CreateBitmapBrush(bmp, Some(&props), Some(&D2D1_BRUSH_PROPERTIES { opacity: a * p.a, transform: Matrix3x2::identity() }))?;
    b.SetTransform(&Matrix3x2::translation(l, t));
    let rect = D2D_RECT_F { left: l, top: t, right: l + size.width, bottom: t + size.height };
    p.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: 7.0, radiusY: 7.0 }, &b);
    Ok(())
}

unsafe fn badge(p: &Pen, t: &Theme, count: usize, tint: Rgb, a: f32) -> Result<()> {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    let w = 18.0 + 6.5 * (text.len() as f32 - 1.0);
    let (x, y) = (ICON / 2.0 - 1.0, -13.0 - ICON / 2.0 + 3.0);
    let pill = |grow: f32| D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: x - w / 2.0 - grow, top: y - 9.0 - grow, right: x + w / 2.0 + grow, bottom: y + 9.0 + grow },
        radiusX: 9.0 + grow,
        radiusY: 9.0 + grow,
    };
    p.rt.FillRoundedRectangle(&pill(2.0), p.brush(tint, a));
    p.rt.FillRoundedRectangle(&pill(0.0), p.brush(t.accent, a));
    p.text(&text, &p.g.badge, x, y - 0.5, w, 18.0, t.on_accent(), a);
    Ok(())
}
