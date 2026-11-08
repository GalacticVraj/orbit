//! The rename window: Direct2D + DirectWrite over a Mica backdrop. Text fields are drawn here too
//! (GDI EDIT controls render with alpha 0 on a frame-extended Mica surface, i.e. invisible text).

use crate::engine::{self, Case, Entry, Options, Sort, Status};
use std::collections::HashSet;
use std::path::PathBuf;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("Orbit.Rename");
const W: f32 = 720.0;
const H: f32 = 560.0;
const ROW: f32 = 30.0;
const PAD: f32 = 11.0;
const BLINK: usize = 1;
const CF_UNICODETEXT: u32 = 13;

/// Rect in DIPs: left, top, right, bottom.
#[derive(Clone, Copy)]
struct R(f32, f32, f32, f32);

impl R {
    fn has(self, x: f32, y: f32) -> bool {
        x >= self.0 && x < self.2 && y >= self.1 && y < self.3
    }
    fn d(self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.0, top: self.1, right: self.2, bottom: self.3 }
    }
    fn inset(self, dx: f32, dy: f32) -> R {
        R(self.0 + dx, self.1 + dy, self.2 - dx, self.3 - dy)
    }
    fn split(self, n: usize, i: usize) -> R {
        let w = (self.2 - self.0) / n as f32;
        R(self.0 + w * i as f32, self.1, self.0 + w * (i + 1) as f32, self.3)
    }
}

const FIELDS: [R; 4] = [R(24., 100., 576., 132.), R(592., 100., 696., 132.), R(24., 192., 232., 224.), R(248., 192., 456., 224.)];
const LABELS: [&str; 4] = ["Pattern", "Start at", "Find", "Replace with"];
const HINTS: [&str; 4] = ["", "1", "Text to find", "Nothing"];
const CASE_R: R = R(472., 192., 696., 224.);
const CASES: [(Case, &str); 4] = [(Case::Keep, "Keep"), (Case::Lower, "lower"), (Case::Upper, "UPPER"), (Case::Title, "Title")];
const CARD: R = R(24., 244., 696., 492.);
const SORT_R: R = R(500., 251., 686., 279.);
const SORTS: [(Sort, &str); 2] = [(Sort::Name, "Name"), (Sort::Date, "Date modified")];
const LIST: R = R(24., 287., 696., 488.);
const APPLY: R = R(496., 508., 592., 540.);
const CANCEL: R = R(600., 508., 696., 540.);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    Field(usize),
    Case(usize),
    Sort(usize),
    Apply,
    Cancel,
    Row(usize),
}

/// Single-line text field state (UTF-16, like DirectWrite and WM_CHAR).
#[derive(Default)]
struct Field {
    text: Vec<u16>,
    caret: usize,
    anchor: usize,
    /// Horizontal scroll of the text, updated while drawing.
    off: std::cell::Cell<f32>,
}

impl Field {
    fn set(&mut self, s: &str) {
        self.text = s.encode_utf16().collect();
        self.caret = self.text.len();
        self.anchor = self.caret;
    }
    fn string(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }
    fn sel(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
    fn insert(&mut self, s: &[u16]) {
        let (a, b) = self.sel();
        self.text.splice(a..b, s.iter().copied());
        self.caret = a + s.len();
        self.anchor = self.caret;
    }
    /// Next caret stop left (dir < 0) or right; whole words with Ctrl. Never splits a surrogate pair.
    fn step(&self, mut i: usize, dir: i32, word: bool) -> usize {
        let t = &self.text;
        let one = |i: usize| {
            if dir < 0 {
                let j = i.saturating_sub(1);
                if j > 0 && (0xDC00..0xE000).contains(&t[j]) { j - 1 } else { j }
            } else if i + 1 < t.len() && (0xD800..0xDC00).contains(&t[i]) {
                i + 2
            } else {
                (i + 1).min(t.len())
            }
        };
        if !word {
            return one(i);
        }
        let is_word = |c: u16| char::from_u32(c as u32).map_or(true, char::is_alphanumeric);
        if dir < 0 {
            while i > 0 && !is_word(t[i - 1]) {
                i = one(i);
            }
            while i > 0 && is_word(t[i - 1]) {
                i = one(i);
            }
        } else {
            while i < t.len() && is_word(t[i]) {
                i = one(i);
            }
            while i < t.len() && !is_word(t[i]) {
                i = one(i);
            }
        }
        i
    }
}

struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    title: IDWriteTextFormat,
    sub: IDWriteTextFormat,
    label: IDWriteTextFormat,
    body: IDWriteTextFormat,
    row: IDWriteTextFormat,
    note: IDWriteTextFormat,
    seg: IDWriteTextFormat,
    seg_on: IDWriteTextFormat,
    btn: IDWriteTextFormat,
    strong: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    icon_lg: IDWriteTextFormat,
}

impl Gfx {
    fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut fonts = None;
            dw.GetSystemFontCollection(&mut fonts, false)?;
            let pick = |want: PCWSTR, fallback: PCWSTR| {
                let (mut i, mut ok) = (0, BOOL(0));
                match &fonts {
                    Some(f) if f.FindFamilyName(want, &mut i, &mut ok).is_ok() && ok.as_bool() => want,
                    _ => fallback,
                }
            };
            let text = pick(w!("Segoe UI Variable Text"), w!("Segoe UI"));
            let display = pick(w!("Segoe UI Variable Display"), w!("Segoe UI"));
            let icons = pick(w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets"));
            let f = |family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
                let f = dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetTextAlignment(align)?;
                let sign = dw.CreateEllipsisTrimmingSign(&f)?;
                let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                f.SetTrimming(&trim, &sign)?;
                Ok(f)
            };
            let (l, c, r) = (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_TRAILING);
            let (reg, semi) = (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD);
            Ok(Gfx {
                title: f(display, 20., semi, l)?,
                sub: f(text, 12., reg, l)?,
                label: f(text, 12., reg, l)?,
                body: f(text, 14., reg, l)?,
                row: f(text, 13., reg, l)?,
                note: f(text, 12., reg, r)?,
                seg: f(text, 13., reg, c)?,
                seg_on: f(text, 13., semi, c)?,
                btn: f(text, 14., reg, c)?,
                strong: f(text, 14., semi, l)?,
                icon: f(icons, 12., reg, c)?,
                icon_lg: f(icons, 18., reg, c)?,
                d2d,
                dw,
            })
        }
    }
}

fn c(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn rgb(r: u8, g: u8, b: u8) -> D2D1_COLOR_F {
    c(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.)
}

fn alpha(mut col: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    col.a *= a;
    col
}

/// Windows 11 (Fluent) colour tokens.
struct Pal {
    text1: D2D1_COLOR_F,
    text2: D2D1_COLOR_F,
    text3: D2D1_COLOR_F,
    ctrl: D2D1_COLOR_F,
    ctrl_hover: D2D1_COLOR_F,
    input_on: D2D1_COLOR_F,
    stroke: D2D1_COLOR_F,
    strong: D2D1_COLOR_F,
    card: D2D1_COLOR_F,
    card_stroke: D2D1_COLOR_F,
    subtle: D2D1_COLOR_F,
    seg_on: D2D1_COLOR_F,
    err: D2D1_COLOR_F,
    bg: D2D1_COLOR_F,
    accent: D2D1_COLOR_F,
    on_accent: D2D1_COLOR_F,
    off_fill: D2D1_COLOR_F,
    off_text: D2D1_COLOR_F,
}

impl Pal {
    fn new(dark: bool, accent: D2D1_COLOR_F) -> Pal {
        let fg = |a| if dark { c(1., 1., 1., a) } else { c(0., 0., 0., a) };
        if dark {
            Pal {
                text1: fg(1.),
                text2: fg(0.786),
                text3: fg(0.545),
                ctrl: fg(0.061),
                ctrl_hover: fg(0.084),
                input_on: c(0.118, 0.118, 0.118, 0.7),
                stroke: fg(0.07),
                strong: fg(0.545),
                card: fg(0.051),
                card_stroke: c(0., 0., 0., 0.1),
                subtle: fg(0.06),
                seg_on: fg(0.1),
                err: rgb(0xFF, 0x99, 0xA4),
                bg: rgb(0x20, 0x20, 0x20),
                accent,
                on_accent: c(0., 0., 0., 1.),
                off_fill: fg(0.158),
                off_text: fg(0.53),
            }
        } else {
            Pal {
                text1: fg(0.894),
                text2: fg(0.62),
                text3: fg(0.447),
                ctrl: c(1., 1., 1., 0.7),
                ctrl_hover: c(0.976, 0.976, 0.976, 0.5),
                input_on: c(1., 1., 1., 1.),
                stroke: fg(0.0578),
                strong: fg(0.447),
                card: c(1., 1., 1., 0.7),
                card_stroke: fg(0.058),
                subtle: fg(0.037),
                seg_on: c(1., 1., 1., 1.),
                err: rgb(0xC4, 0x2B, 0x1C),
                bg: rgb(0xF3, 0xF3, 0xF3),
                accent,
                on_accent: c(1., 1., 1., 1.),
                off_fill: fg(0.217),
                off_text: c(1., 1., 1., 1.),
            }
        }
    }
}

/// (dark mode, accent colour) from the user's personalisation settings.
fn read_theme() -> (bool, D2D1_COLOR_F) {
    unsafe {
        let mut light = 1u32;
        let mut n = 4u32;
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut light as *mut u32 as _),
            Some(&mut n),
        );
        let dark = light == 0;
        let mut pal = [0u8; 32];
        let mut n = 32u32;
        let ok = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(pal.as_mut_ptr() as _),
            Some(&mut n),
        ) == ERROR_SUCCESS;
        // AccentPalette = Light3, Light2, Light1, Base, Dark1, ... (RGBA). Fluent: Light2 on dark, Dark1 on light.
        let i = if dark { 4 } else { 16 };
        let accent = match (ok, dark) {
            (true, _) => rgb(pal[i], pal[i + 1], pal[i + 2]),
            (false, true) => rgb(0x60, 0xCD, 0xFF),
            (false, false) => rgb(0x00, 0x5F, 0xB8),
        };
        (dark, accent)
    }
}

/// Drawing helpers bound to one render target + brush.
struct Pen<'a> {
    rt: &'a ID2D1RenderTarget,
    b: ID2D1SolidColorBrush,
}

impl Pen<'_> {
    unsafe fn rect(&self, r: R, col: D2D1_COLOR_F) {
        self.b.SetColor(&col);
        self.rt.FillRectangle(&r.d(), &self.b);
    }
    unsafe fn fill(&self, r: R, rad: f32, col: D2D1_COLOR_F) {
        self.b.SetColor(&col);
        self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r.d(), radiusX: rad, radiusY: rad }, &self.b);
    }
    unsafe fn line(&self, r: R, rad: f32, col: D2D1_COLOR_F) {
        self.b.SetColor(&col);
        let rr = D2D1_ROUNDED_RECT { rect: r.inset(0.5, 0.5).d(), radiusX: rad, radiusY: rad };
        self.rt.DrawRoundedRectangle(&rr, &self.b, 1.0, None);
    }
    unsafe fn text(&self, f: &IDWriteTextFormat, s: &str, r: R, col: D2D1_COLOR_F) {
        let s: Vec<u16> = s.encode_utf16().collect();
        self.b.SetColor(&col);
        self.rt.DrawText(&s, f, &r.d(), &self.b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }
}

struct Win {
    hwnd: HWND,
    journal: PathBuf,
    gfx: Gfx,
    rt: Option<ID2D1HwndRenderTarget>,
    entries: Vec<Entry>,
    existing: HashSet<String>,
    names: Vec<String>,
    status: Vec<Status>,
    fields: [Field; 4],
    focus: usize,
    case: Case,
    sort: Sort,
    scroll: f32,
    hover: Hit,
    pressed: Hit,
    drag: Option<usize>,
    tracking: bool,
    caret_on: bool,
    active: bool,
    error: Option<String>,
    title: String,
    folder: String,
    noun: &'static str,
    dark: bool,
    accent: D2D1_COLOR_F,
    mica: bool,
    dpi: u32,
}

impl Win {
    fn new(paths: Vec<PathBuf>, journal: PathBuf) -> Option<Win> {
        let mut entries = engine::load(&paths);
        if entries.is_empty() {
            return None;
        }
        engine::sort(&mut entries, Sort::Name);
        let dirs: HashSet<_> = entries.iter().filter_map(|e| e.path.parent()).collect();
        let folder = match dirs.iter().next() {
            Some(d) if dirs.len() == 1 => d.display().to_string(),
            _ => "Multiple folders".into(),
        };
        let folders = entries.iter().filter(|e| e.kind == orbit_core::Kind::Folder).count();
        let noun = if folders == entries.len() { "folder" } else if folders > 0 { "item" } else { "file" };
        let n = entries.len();
        let title = format!("Rename {n} {noun}{}", if n == 1 { "" } else { "s" });
        let mut fields: [Field; 4] = Default::default();
        fields[0].set(&engine::smart_pattern(&entries));
        fields[0].anchor = 0;
        fields[1].set("1");
        let (dark, accent) = read_theme();
        let mut w = Win {
            hwnd: HWND::default(),
            journal,
            gfx: Gfx::new().ok()?,
            rt: None,
            existing: engine::existing(&entries),
            entries,
            names: Vec::new(),
            status: Vec::new(),
            fields,
            focus: 0,
            case: Case::Keep,
            sort: Sort::Name,
            scroll: 0.,
            hover: Hit::None,
            pressed: Hit::None,
            drag: None,
            tracking: false,
            caret_on: true,
            active: true,
            error: None,
            title,
            folder,
            noun,
            dark,
            accent,
            mica: false,
            dpi: 96,
        };
        w.refresh();
        Some(w)
    }

    fn refresh(&mut self) {
        let o = Options {
            pattern: self.fields[0].string(),
            start: self.fields[1].string().parse().unwrap_or(1),
            find: self.fields[2].string(),
            replace: self.fields[3].string(),
            case: self.case,
        };
        self.names = engine::render(&self.entries, &o);
        self.status = engine::validate(&self.entries, &self.names, &self.existing);
        self.error = None;
        self.invalidate();
    }

    fn invalidate(&self) {
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = InvalidateRect(self.hwnd, None, false);
            }
        }
    }

    fn blink(&mut self) {
        self.caret_on = true;
        unsafe {
            let t = GetCaretBlinkTime();
            if t != u32::MAX && !self.hwnd.0.is_null() {
                SetTimer(self.hwnd, BLINK, t, None);
            }
        }
    }

    fn max_scroll(&self) -> f32 {
        (self.entries.len() as f32 * ROW - (LIST.3 - LIST.1)).max(0.)
    }

    fn scroll_by(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy).clamp(0., self.max_scroll());
        self.invalidate();
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        if let Some(i) = FIELDS.iter().position(|r| r.has(x, y)) {
            return Hit::Field(i);
        }
        if let Some(i) = (0..CASES.len()).find(|&i| CASE_R.split(CASES.len(), i).has(x, y)) {
            return Hit::Case(i);
        }
        if let Some(i) = (0..SORTS.len()).find(|&i| SORT_R.split(SORTS.len(), i).has(x, y)) {
            return Hit::Sort(i);
        }
        if APPLY.has(x, y) {
            return Hit::Apply;
        }
        if CANCEL.has(x, y) {
            return Hit::Cancel;
        }
        if LIST.has(x, y) {
            let i = ((y - LIST.1 + self.scroll) / ROW) as usize;
            if i < self.entries.len() {
                return Hit::Row(i);
            }
        }
        Hit::None
    }

    fn layout(&self, t: &[u16]) -> Result<IDWriteTextLayout> {
        unsafe { self.gfx.dw.CreateTextLayout(t, &self.gfx.body, 4096., FIELDS[0].3 - FIELDS[0].1) }
    }

    fn caret_x(&self, l: &IDWriteTextLayout, pos: usize) -> f32 {
        let (mut x, mut y, mut m) = (0., 0., DWRITE_HIT_TEST_METRICS::default());
        unsafe {
            let _ = l.HitTestTextPosition(pos as u32, false, &mut x, &mut y, &mut m);
        }
        x
    }

    fn caret_at(&self, i: usize, x: f32) -> usize {
        let f = &self.fields[i];
        let Ok(l) = self.layout(&f.text) else { return f.caret };
        let (mut trailing, mut inside, mut m) = (BOOL(0), BOOL(0), DWRITE_HIT_TEST_METRICS::default());
        unsafe {
            let _ = l.HitTestPoint(x - FIELDS[i].0 - PAD + f.off.get(), 16., &mut trailing, &mut inside, &mut m);
        }
        (m.textPosition + if trailing.as_bool() { m.length } else { 0 }) as usize
    }

    /// Typed or pasted text into the focused field; true if it changed.
    fn type_text(&mut self, s: &[u16]) -> bool {
        let f = &mut self.fields[self.focus];
        let s: Vec<u16> = if self.focus == 1 {
            let (a, b) = f.sel();
            let room = 9usize.saturating_sub(f.text.len() - (b - a));
            s.iter().copied().filter(|c| (b'0' as u16..=b'9' as u16).contains(c)).take(room).collect()
        } else {
            s.iter().copied().filter(|&c| c >= 0x20 && c != 0x7F).collect()
        };
        let change = !s.is_empty() || f.caret != f.anchor;
        if change {
            f.insert(&s);
        }
        change
    }

    unsafe fn key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
        let hwnd = self.hwnd;
        let page = LIST.3 - LIST.1 - ROW;
        let i = self.focus;
        let f = &mut self.fields[i];
        let before = f.text.clone();
        let len = f.text.len();
        match vk {
            VK_TAB => {
                self.focus = (self.focus + if shift { 3 } else { 1 }) % 4;
                let f = &mut self.fields[self.focus];
                f.anchor = 0;
                f.caret = f.text.len();
            }
            VK_RETURN => self.apply(),
            VK_ESCAPE => {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            VK_LEFT | VK_RIGHT => {
                let d = if vk == VK_LEFT { -1 } else { 1 };
                let (a, b) = f.sel();
                f.caret = if !shift && a != b && !ctrl { if d < 0 { a } else { b } } else { f.step(f.caret, d, ctrl) };
                if !shift {
                    f.anchor = f.caret;
                }
            }
            VK_HOME | VK_END => {
                f.caret = if vk == VK_HOME { 0 } else { len };
                if !shift {
                    f.anchor = f.caret;
                }
            }
            VK_BACK | VK_DELETE => {
                if f.anchor == f.caret {
                    f.caret = f.step(f.caret, if vk == VK_BACK { -1 } else { 1 }, ctrl);
                }
                f.insert(&[]);
            }
            VK_A if ctrl => {
                f.anchor = 0;
                f.caret = len;
            }
            VK_C | VK_X if ctrl => {
                let (a, b) = f.sel();
                if a != b {
                    clip_set(hwnd, &f.text[a..b]);
                    if vk == VK_X {
                        f.insert(&[]);
                    }
                }
            }
            VK_V if ctrl => {
                if let Some(s) = clip_get(hwnd) {
                    let s: Vec<u16> = s.into_iter().map(|c| if c == b'\r' as u16 || c == b'\n' as u16 { b' ' as u16 } else { c }).collect();
                    self.type_text(&s);
                }
            }
            VK_UP | VK_DOWN => self.scroll_by(if vk == VK_UP { -ROW } else { ROW }),
            VK_PRIOR | VK_NEXT => self.scroll_by(if vk == VK_PRIOR { -page } else { page }),
            _ => return false,
        }
        if self.fields[i].text != before {
            self.refresh();
        }
        self.blink();
        self.invalidate();
        true
    }

    fn apply(&mut self) {
        if !engine::summary(&self.status, self.noun).1 {
            return;
        }
        let pairs: Vec<(PathBuf, PathBuf)> = self
            .entries
            .iter()
            .zip(&self.names)
            .zip(&self.status)
            .filter(|(_, s)| **s == Status::Ok)
            .map(|((e, n), _)| (e.path.clone(), e.path.with_file_name(n)))
            .collect();
        match engine::apply(&pairs, &self.journal) {
            Ok(()) => unsafe {
                let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Err(e) => {
                self.error = Some(e);
                self.invalidate();
            }
        }
    }

    unsafe fn style_window(&mut self) {
        let hwnd = self.hwnd;
        let set = |attr: DWMWINDOWATTRIBUTE, v: i32| DwmSetWindowAttribute(hwnd, attr, &v as *const i32 as _, 4).is_ok();
        set(DWMWA_USE_IMMERSIVE_DARK_MODE, self.dark as i32);
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0);
        self.mica = set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW.0);
        let m = if self.mica { -1 } else { 0 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: m, cxRightWidth: m, cyTopHeight: m, cyBottomHeight: m });
        let o = WTA_OPTIONS { dwFlags: WTNCA_NODRAWCAPTION | WTNCA_NODRAWICON, dwMask: WTNCA_NODRAWCAPTION | WTNCA_NODRAWICON };
        let _ = SetWindowThemeAttribute(hwnd, WTA_NONCLIENT, &o as *const WTA_OPTIONS as _, std::mem::size_of::<WTA_OPTIONS>() as u32);
    }

    unsafe fn paint(&mut self) {
        if self.rt.is_none() {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let props = rt_props(self.dpi as f32);
            let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            self.rt = self.gfx.d2d.CreateHwndRenderTarget(&props, &hp).ok();
        }
        let Some(rt) = self.rt.clone() else { return };
        rt.BeginDraw();
        self.draw(&rt);
        if rt.EndDraw(None, None).is_err() {
            self.rt = None; // device lost: recreate on next paint
        }
    }

    unsafe fn draw(&self, rt: &ID2D1RenderTarget) {
        let p = Pal::new(self.dark, self.accent);
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        rt.Clear(Some(&if self.mica { c(0., 0., 0., 0.) } else { p.bg }));
        let Ok(b) = rt.CreateSolidColorBrush(&p.text1, None) else { return };
        let d = Pen { rt, b };
        let g = &self.gfx;

        // Header
        let tile = R(24., 22., 60., 58.);
        d.fill(tile, 8., alpha(p.accent, 0.16));
        d.text(&g.icon_lg, "\u{E8AC}", tile, p.accent);
        d.text(&g.title, &self.title, R(74., 19., 696., 42.), p.text1);
        d.text(&g.sub, &self.folder, R(74., 41., 696., 58.), p.text2);

        // Inputs
        for (i, r) in FIELDS.iter().enumerate() {
            d.text(&g.label, LABELS[i], R(r.0, r.1 - 24., r.2, r.1 - 4.), p.text2);
            self.draw_field(&d, &p, i);
        }
        if let Ok(l) = self.hint_layout() {
            d.b.SetColor(&p.text3);
            rt.DrawTextLayout(D2D_POINT_2F { x: FIELDS[0].0 + 2., y: FIELDS[0].3 + 6. }, &l, &d.b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        d.text(&g.label, "Case", R(CASE_R.0, CASE_R.1 - 24., CASE_R.2, CASE_R.1 - 4.), p.text2);
        let case = CASES.iter().position(|c| c.0 == self.case).unwrap_or(0);
        let hover = |h: fn(usize) -> Hit, n: usize| (0..n).find(|&i| self.hover == h(i));
        self.segments(&d, &p, CASE_R, &CASES.map(|c| c.1), case, hover(Hit::Case, CASES.len()));

        // Preview card
        d.fill(CARD, 8., p.card);
        d.line(CARD, 8., p.card_stroke);
        d.text(&g.strong, "Preview", R(CARD.0 + 16., SORT_R.1, 300., SORT_R.3), p.text1);
        let sort = SORTS.iter().position(|s| s.0 == self.sort).unwrap_or(0);
        self.segments(&d, &p, SORT_R, &SORTS.map(|s| s.1), sort, hover(Hit::Sort, SORTS.len()));
        d.rect(R(CARD.0 + 1., LIST.1 - 1., CARD.2 - 1., LIST.1), p.stroke);
        rt.PushAxisAlignedClip(&LIST.d(), D2D1_ANTIALIAS_MODE_ALIASED);
        let first = (self.scroll / ROW) as usize;
        let visible = ((LIST.3 - LIST.1) / ROW) as usize + 2;
        for i in first..self.entries.len().min(first + visible) {
            let y = LIST.1 + 4. + i as f32 * ROW - self.scroll;
            if self.hover == Hit::Row(i) {
                d.fill(R(CARD.0 + 6., y, CARD.2 - 6., y + ROW), 4., p.subtle);
            }
            let old = self.entries[i].path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            d.text(&g.row, &old, R(CARD.0 + 16., y, 306., y + ROW), p.text2);
            d.text(&g.icon, "\u{E72A}", R(310., y, 334., y + ROW), p.text3);
            let (col, note) = match self.status[i] {
                Status::Ok => (p.text1, None),
                Status::Same => (p.text3, Some(("Unchanged", p.text3))),
                Status::Bad(pr) => (p.err, Some((pr.label(), p.err))),
            };
            let right = if note.is_some() { 548. } else { CARD.2 - 16. };
            d.text(&g.row, &self.names[i], R(342., y, right, y + ROW), col);
            if let Some((s, col)) = note {
                d.text(&g.note, s, R(552., y, CARD.2 - 16., y + ROW), col);
            }
        }
        rt.PopAxisAlignedClip();
        if self.max_scroll() > 0. {
            let vh = LIST.3 - LIST.1;
            let th = (vh * vh / (vh + self.max_scroll())).max(24.);
            let ty = LIST.1 + (vh - th) * self.scroll / self.max_scroll();
            d.fill(R(CARD.2 - 6., ty + 3., CARD.2 - 3., ty + th - 3.), 1.5, alpha(p.text3, 0.7));
        }

        // Footer
        let (msg, ok) = engine::summary(&self.status, self.noun);
        let (msg, bad) = match &self.error {
            Some(e) => (e.clone(), true),
            None => (msg, !ok && self.status.iter().any(|s| matches!(s, Status::Bad(_)))),
        };
        let mut x = 26.;
        if bad {
            d.text(&g.icon_lg, "\u{E783}", R(24., APPLY.1, 44., APPLY.3), p.err);
            x = 52.;
        }
        d.text(&g.row, &msg, R(x, APPLY.1, APPLY.0 - 12., APPLY.3), if bad { p.err } else { p.text2 });
        let ok = ok && self.error.is_none();
        let (fill, text) = match (ok, self.pressed == Hit::Apply, self.hover == Hit::Apply) {
            (false, ..) => (p.off_fill, p.off_text),
            (true, true, _) => (alpha(p.accent, 0.8), alpha(p.on_accent, 0.8)),
            (true, false, true) => (alpha(p.accent, 0.9), p.on_accent),
            _ => (p.accent, p.on_accent),
        };
        d.fill(APPLY, 4., fill);
        d.text(&g.btn, "Rename", APPLY, text);
        let fill = match (self.pressed == Hit::Cancel, self.hover == Hit::Cancel) {
            (true, _) => p.subtle,
            (_, true) => p.ctrl_hover,
            _ => p.ctrl,
        };
        d.fill(CANCEL, 4., fill);
        d.line(CANCEL, 4., p.stroke);
        d.text(&g.btn, "Cancel", CANCEL, p.text1);
    }

    fn hint_layout(&self) -> Result<IDWriteTextLayout> {
        let parts = [("###", "  number"), ("{name}", "  original name"), ("{date}", "  date modified"), ("{ext}", "  extension")];
        let mut s = String::new();
        let mut bold = Vec::new();
        for (i, (tok, what)) in parts.iter().enumerate() {
            if i > 0 {
                s += "      ";
            }
            bold.push((s.encode_utf16().count() as u32, tok.len() as u32));
            s += tok;
            s += what;
        }
        let s: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let l = self.gfx.dw.CreateTextLayout(&s, &self.gfx.label, 600., 16.)?;
            for (start, length) in bold {
                l.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_RANGE { startPosition: start, length })?;
            }
            Ok(l)
        }
    }

    unsafe fn segments(&self, d: &Pen, p: &Pal, r: R, labels: &[&str], sel: usize, hover: Option<usize>) {
        d.fill(r, 5., p.ctrl);
        d.line(r, 5., p.stroke);
        for (i, s) in labels.iter().enumerate() {
            let sr = r.split(labels.len(), i).inset(3., 3.);
            if i == sel {
                d.fill(sr, 3., p.seg_on);
                d.line(sr, 3., p.stroke);
                d.text(&self.gfx.seg_on, s, sr, p.text1);
            } else {
                if hover == Some(i) {
                    d.fill(sr, 3., p.subtle);
                }
                d.text(&self.gfx.seg, s, sr, p.text2);
            }
        }
    }

    unsafe fn draw_field(&self, d: &Pen, p: &Pal, i: usize) {
        let r = FIELDS[i];
        let on = self.focus == i;
        d.fill(r, 4., if on { p.input_on } else if self.hover == Hit::Field(i) { p.ctrl_hover } else { p.ctrl });
        d.line(r, 4., p.stroke);
        // Bottom edge follows the rounded corners: clip a filled rounded rect to its last 1-2 px.
        let edge = if on { 2. } else { 1. };
        d.rt.PushAxisAlignedClip(&R(r.0, r.3 - edge, r.2, r.3).d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        d.fill(r, 4., if on { p.accent } else { p.strong });
        d.rt.PopAxisAlignedClip();

        let inner = R(r.0 + PAD, r.1, r.2 - PAD, r.3);
        let Ok(l) = self.layout(&self.fields[i].text) else { return };
        let f = &self.fields[i];
        let cx = self.caret_x(&l, f.caret);
        let width = inner.2 - inner.0 - 2.;
        let mut off = f.off.get().min(cx).max(cx - width).max(0.);
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = l.GetMetrics(&mut m);
        let total = m.widthIncludingTrailingWhitespace;
        off = off.min((total - width).max(0.));
        d.rt.PushAxisAlignedClip(&inner.d(), D2D1_ANTIALIAS_MODE_ALIASED);
        let (a, b) = f.sel();
        if on && a != b {
            let (x1, x2) = (self.caret_x(&l, a), self.caret_x(&l, b));
            d.rect(R(inner.0 + x1 - off, r.1 + 6., inner.0 + x2 - off, r.3 - 6.), alpha(p.accent, 0.4));
        }
        if f.text.is_empty() {
            d.text(&self.gfx.body, HINTS[i], inner, p.text3);
        }
        d.b.SetColor(&p.text1);
        d.rt.DrawTextLayout(D2D_POINT_2F { x: inner.0 - off, y: r.1 }, &l, &d.b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        if on && self.active && self.caret_on {
            let x = (inner.0 + cx - off).round();
            d.rect(R(x, r.1 + 8., x + 1., r.3 - 8.), p.text1);
        }
        d.rt.PopAxisAlignedClip();
        f.off.set(off);
    }
}

fn rt_props(dpi: f32) -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: dpi,
        dpiY: dpi,
        ..Default::default()
    }
}

unsafe fn clip_get(hwnd: HWND) -> Option<Vec<u16>> {
    OpenClipboard(hwnd).ok()?;
    let mut out = None;
    if let Ok(h) = GetClipboardData(CF_UNICODETEXT) {
        let g = HGLOBAL(h.0);
        let p = GlobalLock(g) as *const u16;
        if !p.is_null() {
            let n = (0..).take_while(|&i| *p.add(i) != 0).count();
            out = Some(std::slice::from_raw_parts(p, n).to_vec());
            let _ = GlobalUnlock(g);
        }
    }
    let _ = CloseClipboard();
    out
}

unsafe fn clip_set(hwnd: HWND, s: &[u16]) {
    if OpenClipboard(hwnd).is_err() {
        return;
    }
    let _ = EmptyClipboard();
    if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, (s.len() + 1) * 2) {
        let p = GlobalLock(g) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
            *p.add(s.len()) = 0;
            let _ = GlobalUnlock(g);
            if SetClipboardData(CF_UNICODETEXT, HANDLE(g.0)).is_err() {
                let _ = GlobalFree(g);
            }
        }
    }
    let _ = CloseClipboard();
}

pub(crate) fn open(paths: Vec<PathBuf>, journal: PathBuf) {
    let Some(win) = Win::new(paths, journal) else { return };
    unsafe {
        let inst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: inst.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly once registered
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dpi, mut _y) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let px = |v: f32| (v * dpi as f32 / 96.).round() as i32;
        let mut rc = RECT { left: 0, top: 0, right: px(W), bottom: px(H) };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let wa = mi.rcWork;
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = (wa.top + (wa.bottom - wa.top - h) / 2).max(wa.top);
        // WM_NCCREATE takes the box out of this slot; if creation fails early it is dropped here.
        let mut slot = Some(Box::new(win));
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Rename"),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            HINSTANCE(inst.0),
            Some(&mut slot as *mut Option<Box<Win>> as _),
        );
        if let Ok(hwnd) = created {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
            if let Some(w) = p.as_mut() {
                w.dpi = GetDpiForWindow(hwnd);
                w.style_window();
                w.blink();
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn lo(v: isize) -> i32 {
    (v & 0xFFFF) as u16 as i16 as i32
}
fn hi(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as u16 as i16 as i32
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        if let Some(mut w) = (*(cs.lpCreateParams as *mut Option<Box<Win>>)).take() {
            w.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(w) as isize);
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(p)); // releases every D2D/DWrite object of this window
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    // Only touch the state for messages we handle, so re-entrant default processing never aliases it.
    const MINE: [u32; 18] = [
        WM_PAINT, WM_ERASEBKGND, WM_SIZE, WM_DPICHANGED, WM_ACTIVATE, WM_TIMER, WM_SETCURSOR, WM_MOUSEMOVE,
        WM_MOUSELEAVE, WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MOUSEWHEEL, WM_KEYDOWN, WM_CHAR,
        WM_SETTINGCHANGE, WM_DWMCOLORIZATIONCOLORCHANGED, WM_DESTROY,
    ];
    if !MINE.contains(&msg) {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let w = &mut *p;
    let s = 96. / w.dpi as f32;
    let (x, y) = (lo(lp.0) as f32 * s, hi(lp.0) as f32 * s);
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            w.paint();
            let _ = EndPaint(hwnd, &ps);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_SIZE => {
            if let Some(rt) = &w.rt {
                let _ = rt.Resize(&D2D_SIZE_U { width: lo(lp.0) as u32 & 0xFFFF, height: hi(lp.0) as u32 & 0xFFFF });
            }
        }
        WM_DPICHANGED => {
            w.dpi = (wp.0 & 0xFFFF) as u32;
            if let Some(rt) = &w.rt {
                rt.SetDpi(w.dpi as f32, w.dpi as f32);
            }
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        WM_ACTIVATE => {
            w.active = (wp.0 & 0xFFFF) as u32 != WA_INACTIVE;
            w.blink();
            w.invalidate();
        }
        WM_TIMER => {
            w.caret_on = !w.caret_on;
            if w.active {
                w.invalidate();
            }
        }
        WM_SETCURSOR => {
            if (lp.0 & 0xFFFF) as u32 != HTCLIENT {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = ScreenToClient(hwnd, &mut pt);
            let text = matches!(w.hit(pt.x as f32 * s, pt.y as f32 * s), Hit::Field(_));
            SetCursor(LoadCursorW(None, if text { IDC_IBEAM } else { IDC_ARROW }).unwrap_or_default());
            return LRESULT(1);
        }
        WM_MOUSEMOVE => {
            if !w.tracking {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                w.tracking = TrackMouseEvent(&mut t).is_ok();
            }
            if let Some(i) = w.drag {
                w.fields[i].caret = w.caret_at(i, x);
                w.blink();
                w.invalidate();
            }
            let h = w.hit(x, y);
            if h != w.hover {
                w.hover = h;
                w.invalidate();
            }
        }
        WM_MOUSELEAVE => {
            w.tracking = false;
            w.hover = Hit::None;
            w.invalidate();
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let h = w.hit(x, y);
            w.pressed = h;
            match h {
                Hit::Field(i) => {
                    w.focus = i;
                    if msg == WM_LBUTTONDBLCLK {
                        let f = &mut w.fields[i];
                        f.anchor = 0;
                        f.caret = f.text.len();
                    } else {
                        let c = w.caret_at(i, x);
                        let f = &mut w.fields[i];
                        f.caret = c;
                        if GetKeyState(VK_SHIFT.0 as i32) >= 0 {
                            f.anchor = c;
                        }
                        w.drag = Some(i);
                    }
                }
                Hit::Case(i) if w.case != CASES[i].0 => {
                    w.case = CASES[i].0;
                    w.refresh();
                }
                Hit::Sort(i) if w.sort != SORTS[i].0 => {
                    w.sort = SORTS[i].0;
                    engine::sort(&mut w.entries, w.sort);
                    w.refresh();
                }
                _ => {}
            }
            w.blink();
            w.invalidate();
            SetCapture(hwnd);
        }
        WM_LBUTTONUP => {
            let h = w.hit(x, y);
            let pressed = std::mem::replace(&mut w.pressed, Hit::None);
            w.drag = None;
            w.invalidate();
            if h == pressed && h == Hit::Apply {
                w.apply();
            } else if h == pressed && h == Hit::Cancel {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            let _ = ReleaseCapture();
        }
        WM_MOUSEWHEEL => w.scroll_by(-(hi(wp.0 as isize) as f32) / 120. * ROW * 3.),
        WM_KEYDOWN => {
            if !w.key(VIRTUAL_KEY(wp.0 as u16)) {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
        }
        WM_CHAR => {
            let ch = wp.0 as u16;
            if w.type_text(&[ch]) {
                w.refresh();
            }
            w.blink();
        }
        WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
            (w.dark, w.accent) = read_theme();
            w.style_window();
            w.invalidate();
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, BLINK);
        }
        _ => {}
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;

    fn sample_dir(name: &str) -> (PathBuf, Vec<PathBuf>) {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut v = Vec::new();
        for n in [1024, 1025, 1031, 1032, 1040, 1102, 1103, 1104, 1210, 1211, 1300, 1301] {
            let p = d.join(format!("IMG_{n}.jpg"));
            std::fs::write(&p, "x").unwrap();
            v.push(p);
        }
        std::fs::write(d.join("Holiday_002.jpg"), "x").unwrap();
        (d, v)
    }

    unsafe fn save_png(wic: &IWICImagingFactory, src: &IWICBitmapSource, out: &Path) -> Result<()> {
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&windows::core::HSTRING::from(out.as_os_str()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.WriteSource(src, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()
    }

    fn shots_dir() -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/rename-shots");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Renders the window scene (as drawn on screen, minus the Mica blur) to PNGs for review.
    /// `cargo test -p orbit-rename shots -- --ignored`
    #[test]
    #[ignore]
    fn shots() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
            let (dir, paths) = sample_dir("orbit-rename-shots");
            for (name, dark, pattern) in
                [("dark", true, None), ("light", false, None), ("dark-error", true, Some("Holiday_###")), ("light-error", false, Some("Holiday"))]
            {
                let mut w = Win::new(paths.clone(), dir.join("journal.json")).unwrap();
                w.dark = dark;
                w.accent = if dark { rgb(0x60, 0xCD, 0xFF) } else { rgb(0x00, 0x5F, 0xB8) };
                if let Some(pat) = pattern {
                    w.fields[0].set(pat);
                    w.fields[2].set("IMG");
                    w.focus = 2;
                    w.case = Case::Upper;
                    w.hover = Hit::Row(3);
                } else {
                    w.hover = Hit::Apply;
                }
                w.refresh();
                let scale = 1.5;
                let (pw, ph) = ((W * scale) as u32, (H * scale) as u32);
                let bmp = wic.CreateBitmap(pw, ph, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand).unwrap();
                let rt = w.gfx.d2d.CreateWicBitmapRenderTarget(&bmp, &rt_props(96. * scale)).unwrap();
                rt.BeginDraw();
                w.draw(&rt);
                rt.EndDraw(None, None).unwrap();
                save_png(&wic, &bmp.cast().unwrap(), &shots_dir().join(format!("{name}.png"))).unwrap();
            }
        }
    }

    /// Opens the real window (Mica and all), screen-grabs it, closes it.
    /// `cargo test -p orbit-rename live -- --ignored`
    #[test]
    #[ignore]
    fn live() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (dir, paths) = sample_dir("orbit-rename-live");
            open(paths, dir.join("journal.json"));
            let hwnd = FindWindowW(CLASS, None).unwrap();
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            let end = std::time::Instant::now() + std::time::Duration::from_millis(1500);
            let mut msg = MSG::default();
            while std::time::Instant::now() < end {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let mut r = RECT::default();
            DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as _, 16).unwrap();
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let sdc = GetDC(None);
            let mdc = CreateCompatibleDC(sdc);
            let bm = CreateCompatibleBitmap(sdc, w, h);
            let old = SelectObject(mdc, bm);
            BitBlt(mdc, 0, 0, w, h, sdc, r.left, r.top, SRCCOPY).unwrap();
            SelectObject(mdc, old);
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
            let src = wic.CreateBitmapFromHBITMAP(bm, None, WICBitmapIgnoreAlpha).unwrap();
            save_png(&wic, &src.cast().unwrap(), &shots_dir().join("live.png")).unwrap();
            let _ = DeleteObject(bm);
            let _ = DeleteDC(mdc);
            ReleaseDC(None, sdc);
            // Type over the selected pattern, press Enter: renames, journals, closes.
            for ch in "Trip_##".encode_utf16() {
                SendMessageW(hwnd, WM_CHAR, WPARAM(ch as usize), LPARAM(0));
            }
            SendMessageW(hwnd, WM_KEYDOWN, WPARAM(VK_RETURN.0 as usize), LPARAM(0));
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            assert!(FindWindowW(CLASS, None).is_err(), "window closed after apply");
            assert!(dir.join("Trip_01.jpg").exists() && dir.join("Trip_12.jpg").exists());
            let o = engine::undo_at(&dir.join("journal.json")).unwrap();
            assert_eq!(o.message, "Restored 12 names");
            assert!(dir.join("IMG_1024.jpg").exists());
        }
    }
}
