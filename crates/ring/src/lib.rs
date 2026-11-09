//! The Orbit ring: radial command UI + animation + glass effects. OWNER: Agent 3 (Ring UI + Animation/VFX).
//!
//! One layered, topmost popup (created on first use, then only hidden/shown) rendered with Direct2D
//! into a premultiplied DIB and pushed with UpdateLayeredWindow. Frames run only while springs move:
//! render -> DwmFlush -> PostMessage(WM_FRAME). Idle = no timers, no threads, per-show memory freed.

mod draw;
mod ring;

use draw::{Backdrop, Gfx, Surface, Theme};
use orbit_core::{Action, Bitmap, Outcome};
use ring::{Effect, Hit, Phase, Ring, HALF};
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_FRAME: u32 = WM_APP + 0x300;
const WM_PROGRESS: u32 = WM_APP + 0x301;
const WM_FINISH: u32 = WM_APP + 0x302;
const WM_DISMISS: u32 = WM_APP + 0x303;
const TIMER_LINGER: usize = 1;
/// Minimum frame spacing. Below 1/60 s so a 60 Hz panel never drops a vblank.
const FRAME: std::time::Duration = std::time::Duration::from_millis(13);
const WM_MOUSELEAVE: u32 = 0x02A3;

struct Shared {
    progress: AtomicU32,
    posted: AtomicBool,
}

impl Shared {
    fn new() -> Arc<Shared> {
        Arc::new(Shared { progress: AtomicU32::new(0), posted: AtomicBool::new(false) })
    }
}

/// Handle to the in-flight action shown on the ring. Cheap to clone, Send + Sync; every method
/// may be called from any thread (implementation posts window messages to the UI thread).
#[derive(Clone)]
pub struct Task {
    hwnd: isize,
    gen: u32,
    shared: Arc<Shared>,
}

impl Task {
    fn post(&self, msg: u32, lp: isize) -> bool {
        // A null HWND would post to the calling thread's queue instead; never do that.
        self.hwnd != 0
            && unsafe { PostMessageW(HWND(self.hwnd as _), msg, WPARAM(self.gen as usize), LPARAM(lp)) }.is_ok()
    }
    /// 0.0..=1.0. Ring shows a determinate progress arc; until the first call it shows an indeterminate spinner.
    pub fn progress(&self, fraction: f32) {
        self.shared.progress.store(fraction.to_bits(), Ordering::Relaxed);
        // Coalesce: at most one progress message in flight, so a chatty worker can't flood the queue.
        if !self.shared.posted.swap(true, Ordering::AcqRel) && !self.post(WM_PROGRESS, 0) {
            self.shared.posted.store(false, Ordering::Release);
        }
    }
    /// Ok => success micro-animation + `outcome.message` in the centre, then auto-dismiss.
    /// Err => error state with the message (wrapped / truncated nicely), then auto-dismiss.
    pub fn finish(&self, result: Result<Outcome, String>) {
        let p = Box::into_raw(Box::new(result));
        if !self.post(WM_FINISH, p as isize) {
            drop(unsafe { Box::from_raw(p) });
        }
    }
    /// Close the ring immediately (used when an action opens its own window).
    pub fn dismiss(&self) {
        self.post(WM_DISMISS, 0);
    }
}

/// Show the ring centred on the current cursor position (clamped to the monitor work area).
/// UI thread only (the thread running the message loop). `icon` is the selected item's
/// thumbnail/icon (128x128 px). `paths.len() > 1` => show a count badge.
/// `actions` empty => show a brief hint ("Select a file in Explorer") then fade out.
/// More nodes than fit nicely => the ring groups the overflow into a "More" sub-ring.
/// `on_pick` is called on the UI thread with the picked leaf action id and a Task for it.
/// If a Task is still running, `open` re-shows that task's ring at the cursor and ignores the new request.
pub fn open(paths: &[PathBuf], icon: Option<Bitmap>, actions: Vec<Action>, on_pick: Box<dyn FnMut(&str, Task)>) {
    ensure_window();
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    with(|ui| {
        if let Some(r) = ui.ring.as_mut().filter(|r| r.phase == Phase::Running) {
            r.reopen();
            ui.show_at(cursor, true);
            return;
        }
        unsafe {
            let _ = KillTimer(ui.hwnd, TIMER_LINGER);
        }
        ui.gen = ui.gen.wrapping_add(1);
        ui.shared = Shared::new();
        ui.theme = Theme::system();
        let ring = Ring::new(paths, icon, actions);
        let hint = ring.phase == Phase::Hint;
        ui.ring = Some(ring);
        ui.on_pick = Some(on_pick);
        ui.pressed = None;
        ui.show_at(cursor, true);
        if hint {
            ui.linger();
        }
    });
}

/// One-time init (window, Direct2D/DirectWrite, text + icon fonts) so the first Alt+Q opens as fast as
/// later ones. UI thread; call once after startup. Renders one frame offscreen, shows nothing.
pub fn warm_up() {
    ensure_window();
    with(|ui| {
        let (Some(g), Some(mut sf)) = (ui.gfx.as_ref(), Surface::new(64, 64, 1.0)) else { return };
        let r = Ring::new(&[PathBuf::from("a.txt")], None, vec![Action::new("w", "Warm", '\u{E8A5}')]);
        sf.render(g, &r, &Theme::system(), None);
    });
}

// ---------- UI-thread state ----------

type PickCall = (Box<dyn FnMut(&str, Task)>, String, Task);

struct Ui {
    hwnd: HWND,
    gfx: Option<Gfx>,
    gen: u32,
    shared: Arc<Shared>,
    ring: Option<Ring>,
    on_pick: Option<Box<dyn FnMut(&str, Task)>>,
    theme: Theme,
    surface: Option<Surface>,
    backdrop: Option<Backdrop>,
    anchor: POINT,
    pos: POINT,
    visible: bool,
    animating: bool,
    last: Instant,
    pressed: Option<Hit>,
    tracking: bool,
    last_move: LPARAM,
}

thread_local! {
    // ManuallyDrop: no TLS destructor. Releasing D2D/DirectWrite objects during process teardown
    // crashes; the OS reclaims everything at exit anyway.
    static UI: ManuallyDrop<RefCell<Option<Ui>>> = const { ManuallyDrop::new(RefCell::new(None)) };
}

/// Run `f` on the UI state. Returns None when re-entered (a Win32 call we made is sending us a
/// message while the state is borrowed) — those messages fall through to DefWindowProc.
fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|c| c.try_borrow_mut().ok().and_then(|mut u| u.as_mut().map(f)))
}

fn ensure_window() {
    if UI.with(|c| c.borrow().is_some()) {
        return;
    }
    let hwnd = unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("Orbit.Ring"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("Orbit.Ring"),
            w!("Orbit"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        )
        .unwrap_or_default()
    };
    let ui = Ui {
        hwnd,
        gfx: Gfx::new().ok(),
        gen: 0,
        shared: Shared::new(),
        ring: None,
        on_pick: None,
        theme: Theme::system(),
        surface: None,
        backdrop: None,
        anchor: POINT::default(),
        pos: POINT::default(),
        visible: false,
        animating: false,
        last: Instant::now(),
        pressed: None,
        tracking: false,
        last_move: LPARAM(-1),
    };
    UI.with(|c| *c.borrow_mut() = Some(ui));
}

impl Ui {
    /// Place the window around `center` on its monitor, grab the backdrop, show.
    fn show_at(&mut self, center: POINT, activate: bool) {
        unsafe {
            if self.visible {
                let _ = ShowWindow(self.hwnd, SW_HIDE); // keep ourselves out of the backdrop capture
                let _ = DwmFlush();
            }
            let mon = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
            let (mut dx, mut dy) = (96, 96);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let s = dx.max(96) as f32 / 96.0;
            let size = (2.0 * HALF * s).ceil() as i32;
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let wa = mi.rcWork;
            self.anchor = center;
            self.pos = POINT {
                x: (center.x - size / 2).clamp(wa.left, (wa.right - size).max(wa.left)),
                y: (center.y - size / 2).clamp(wa.top, (wa.bottom - size).max(wa.top)),
            };
            self.surface = Surface::new(size, size, s);
            self.backdrop = match &self.surface {
                Some(sf) if self.theme.glass => Some(sf.capture(self.pos.x, self.pos.y)),
                _ => None,
            };
            if let Some(r) = &mut self.ring {
                r.tick(0.0);
            }
            self.paint();
            let _ = ShowWindow(self.hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
            if activate {
                let _ = SetForegroundWindow(self.hwnd);
            }
            self.visible = true;
        }
        self.kick();
    }

    fn paint(&mut self) {
        let (Some(sf), Some(r), Some(g)) = (&mut self.surface, &self.ring, &self.gfx) else { return };
        if !sf.render(g, r, &self.theme, self.backdrop.as_ref()) {
            return;
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&self.pos),
                Some(&SIZE { cx: sf.w, cy: sf.h }),
                sf.dc,
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    /// Start the vsync-paced frame loop if it isn't running.
    fn kick(&mut self) {
        if self.visible && !self.animating {
            self.animating = true;
            self.last = Instant::now() - FRAME; // first frame renders right away
            unsafe {
                let _ = PostMessageW(self.hwnd, WM_FRAME, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        // ~60 FPS on any display: on 120-165 Hz panels skip alternate vblanks instead of rendering
        // two to three times as often for motion the eye can barely tell apart.
        if self.visible && now - self.last < FRAME {
            unsafe {
                let _ = DwmFlush();
                let _ = PostMessageW(self.hwnd, WM_FRAME, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let dt = (now - self.last).as_secs_f32().min(0.05);
        self.last = now;
        let Some(r) = self.ring.as_mut().filter(|_| self.visible) else {
            self.animating = false;
            return;
        };
        let moving = r.tick(dt);
        if r.closed() {
            self.hide();
            return;
        }
        self.paint();
        if moving {
            unsafe {
                let _ = DwmFlush();
                let _ = PostMessageW(self.hwnd, WM_FRAME, WPARAM(0), LPARAM(0));
            }
        } else {
            self.animating = false;
        }
    }

    fn close(&mut self) {
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_LINGER);
        }
        if let Some(r) = &mut self.ring {
            r.close();
        }
        self.kick();
    }

    /// Fully hidden: free per-show memory. A running task keeps its ring state for re-showing.
    fn hide(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = KillTimer(self.hwnd, TIMER_LINGER);
        }
        self.visible = false;
        self.animating = false;
        self.surface = None;
        self.backdrop = None;
        self.pressed = None;
        if self.ring.as_ref().is_some_and(|r| r.phase != Phase::Running) {
            self.ring = None;
            self.on_pick = None;
        }
        unsafe {
            let _ = K32EmptyWorkingSet(GetCurrentProcess());
        }
    }

    fn linger(&mut self) {
        if let Some(r) = &self.ring {
            unsafe {
                SetTimer(self.hwnd, TIMER_LINGER, r.linger_ms(), None);
            }
        }
    }

    fn effect(&mut self, e: Effect) -> Option<PickCall> {
        match e {
            Effect::None => {}
            Effect::Close => self.close(),
            Effect::Pick(id) => {
                let task = Task { hwnd: self.hwnd.0 as isize, gen: self.gen, shared: self.shared.clone() };
                self.kick();
                return self.on_pick.take().map(|cb| (cb, id, task));
            }
        }
        self.kick();
        None
    }

    /// Cursor position (client px) -> DIPs relative to the ring centre.
    fn local(&self, lp: LPARAM) -> Option<(f32, f32)> {
        let sf = self.surface.as_ref()?;
        let x = (lp.0 & 0xFFFF) as i16 as f32 - sf.w as f32 / 2.0;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 - sf.h as f32 / 2.0;
        Some((x / sf.scale, y / sf.scale))
    }

    fn hit(&self, lp: LPARAM) -> Option<Hit> {
        let (x, y) = self.local(lp)?;
        self.ring.as_ref()?.hit(x, y)
    }

    /// Returns None if the message isn't ours, else an optional deferred on_pick call.
    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<Option<PickCall>> {
        let current = wp.0 as u32 == self.gen;
        match msg {
            WM_FRAME => self.frame(),
            WM_PROGRESS if current => {
                self.shared.posted.store(false, Ordering::Release);
                let f = f32::from_bits(self.shared.progress.load(Ordering::Relaxed));
                if let Some(r) = &mut self.ring {
                    r.set_progress(f);
                }
                self.kick();
            }
            WM_PROGRESS => {}
            WM_FINISH => {
                let res = *unsafe { Box::from_raw(lp.0 as *mut Result<Outcome, String>) };
                let Some(r) = self.ring.as_mut().filter(|r| current && r.phase == Phase::Running) else {
                    return Some(None);
                };
                let (ok, message) = match res {
                    Ok(o) => (true, o.message),
                    Err(e) => (false, e),
                };
                r.finish(ok, message);
                if !self.visible || r.closing {
                    r.reopen();
                    if !self.visible {
                        self.show_at(self.anchor, false); // play the result without stealing focus
                    }
                }
                self.linger();
                self.kick();
            }
            WM_DISMISS if current => {
                if let Some(r) = self.ring.as_mut().filter(|r| r.phase == Phase::Running) {
                    r.phase = Phase::Done(true);
                }
                if self.visible {
                    self.close();
                } else {
                    self.ring = None;
                }
            }
            WM_DISMISS => {}
            WM_TIMER if wp.0 == TIMER_LINGER => self.close(),
            WM_ACTIVATE => {
                let busy = self.ring.as_ref().is_none_or(|r| r.phase == Phase::Running || r.closing);
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE && self.visible && !busy {
                    self.close();
                }
                return None;
            }
            // Windows re-sends WM_MOUSEMOVE whenever the window changes under a still cursor;
            // only real movement may steal the keyboard highlight.
            WM_MOUSEMOVE if lp == self.last_move => {}
            WM_MOUSEMOVE => {
                self.last_move = lp;
                if !self.tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    self.tracking = unsafe { TrackMouseEvent(&mut t) }.is_ok();
                }
                let h = self.hit(lp);
                let (Some((x, y)), Some(r)) = (self.local(lp), self.ring.as_mut()) else { return Some(None) };
                if r.hover != h {
                    r.set_hover(h);
                }
                r.pointer(x, y);
                self.kick();
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                if let Some(r) = &mut self.ring {
                    r.set_hover(None);
                }
                self.kick();
            }
            WM_LBUTTONDOWN => {
                let h = self.hit(lp);
                let phase = self.ring.as_ref().map(|r| r.phase);
                match h {
                    None if phase != Some(Phase::Running) => self.close(), // the shadow counts as outside
                    None => {}
                    Some(h) => {
                        self.pressed = Some(h);
                        if let Some(r) = &mut self.ring {
                            r.set_press(Some(h));
                        }
                        unsafe { SetCapture(self.hwnd) };
                        self.kick();
                    }
                }
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let h = self.hit(lp);
                let pressed = self.pressed.take();
                let r = self.ring.as_mut()?;
                r.set_press(None);
                let e = match h {
                    Some(h) if pressed == Some(h) => r.activate(h),
                    _ => Effect::None,
                };
                return Some(self.effect(e));
            }
            // Right-click = Esc: up a level, else close.
            WM_RBUTTONUP => {
                let r = self.ring.as_mut()?;
                let e = if r.phase == Phase::Running || r.back() { Effect::None } else { Effect::Close };
                return Some(self.effect(e));
            }
            // The wheel turns the selection round the dial.
            WM_MOUSEWHEEL => {
                let r = self.ring.as_mut()?;
                r.rotate(if (wp.0 >> 16) as i16 > 0 { -1 } else { 1 });
                self.kick();
            }
            WM_KEYDOWN => {
                let r = self.ring.as_mut()?;
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                let e = match vk {
                    VK_ESCAPE | VK_BACK if r.back() => Effect::None,
                    VK_ESCAPE | VK_BACK => Effect::Close,
                    VK_LEFT | VK_UP => {
                        r.rotate(-1);
                        Effect::None
                    }
                    VK_RIGHT | VK_DOWN => {
                        r.rotate(1);
                        Effect::None
                    }
                    VK_TAB => {
                        r.rotate(if shift { -1 } else { 1 });
                        Effect::None
                    }
                    VK_RETURN | VK_SPACE => match r.hover {
                        Some(h) => r.activate(h),
                        None if r.phase == Phase::Menu => Effect::None,
                        None => r.activate(Hit::Center),
                    },
                    _ => {
                        let digit = match vk.0 {
                            k @ 0x31..=0x39 => k - 0x31,
                            k if (VK_NUMPAD1.0..=VK_NUMPAD9.0).contains(&k) => k - VK_NUMPAD1.0,
                            _ => return Some(None),
                        };
                        if r.phase == Phase::Menu && (digit as usize) < r.nodes.len() {
                            r.activate(Hit::Node(digit as usize))
                        } else {
                            Effect::None
                        }
                    }
                };
                return Some(self.effect(e));
            }
            _ => return None,
        }
        Some(None)
    }
}

/// Input, timers and paints that came due while we rendered. Posted messages outrank them in
/// GetMessage, so without this a continuous frame loop would starve the mouse, keyboard and the
/// host's timers. Bounded so a window that never validates its paint can't trap us here.
fn drain_pending() {
    let mut m = MSG::default();
    for (lo, hi, only) in [(0, 0, PM_QS_INPUT), (WM_TIMER, WM_TIMER, PM_REMOVE), (0, 0, PM_QS_PAINT)] {
        for _ in 0..64 {
            if !unsafe { PeekMessageW(&mut m, None, lo, hi, PM_REMOVE | only) }.as_bool() {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_FRAME {
        drain_pending();
    }
    match with(|ui| ui.handle(msg, wp, lp)) {
        Some(Some(call)) => {
            // Called with no borrow held: the host may pump messages, dismiss, or even reopen.
            if let Some((mut cb, id, task)) = call {
                cb(&id, task);
            }
            LRESULT(0)
        }
        _ => {
            if msg == WM_FINISH {
                drop(Box::from_raw(lp.0 as *mut Result<Outcome, String>)); // couldn't deliver: free it
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

// ---------- offscreen rendering for verification ----------

/// Scripted input for [`snapshot`].
#[doc(hidden)]
#[derive(Clone)]
pub enum Step {
    Wait(f32),
    Hover(Option<usize>),
    HoverCenter,
    Click(usize),
    Progress(f32),
    Finish(Result<Outcome, String>),
}

/// Render the ring (same state machine + renderer as the live window) over `screen` with the
/// cursor at `at`, after applying `steps`. Returns the window-sized crop, composited, opaque BGRA.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn snapshot(
    screen: &Bitmap,
    at: (i32, i32),
    dpi: u32,
    dark: bool,
    paths: &[PathBuf],
    icon: Option<Bitmap>,
    actions: Vec<Action>,
    steps: Vec<Step>,
) -> Bitmap {
    let theme = Theme { dark, glass: true, accent: Theme::accent(dark) };
    let mut r = Ring::new(paths, icon, actions);
    for step in steps {
        match step {
            Step::Wait(t) => {
                for _ in 0..(t * 60.0).round() as usize {
                    r.tick(1.0 / 60.0);
                }
            }
            Step::Hover(i) => r.set_hover(i.map(Hit::Node)),
            Step::HoverCenter => r.set_hover(Some(Hit::Center)),
            Step::Click(i) => {
                r.activate(Hit::Node(i));
            }
            Step::Progress(f) => r.set_progress(f),
            Step::Finish(res) => match res {
                Ok(o) => r.finish(true, o.message),
                Err(e) => r.finish(false, e),
            },
        }
    }
    let s = dpi as f32 / 96.0;
    let size = (2.0 * HALF * s).ceil() as i32;
    let (sw, sh) = (screen.width as i32, screen.height as i32);
    let (x0, y0) = ((at.0 - size / 2).clamp(0, (sw - size).max(0)), (at.1 - size / 2).clamp(0, (sh - size).max(0)));
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size.min(sh) {
        let src = (((y0 + y) * sw + x0) * 4) as usize;
        let n = (size.min(sw) * 4) as usize;
        out[(y * size * 4) as usize..][..n].copy_from_slice(&screen.bgra[src..src + n]);
    }
    let backdrop = Backdrop::new(&out, size as usize, size as usize);
    let (Ok(g), Some(mut sf)) = (Gfx::new(), Surface::new(size, size, s)) else { return Bitmap::default() };
    sf.render(&g, &r, &theme, Some(&backdrop));
    for (o, p) in out.chunks_exact_mut(4).zip(sf.pixels().chunks_exact(4)) {
        let inv = 255 - p[3] as u32;
        for c in 0..3 {
            o[c] = (p[c] as u32 + (o[c] as u32 * inv + 127) / 255).min(255) as u8;
        }
        o[3] = 255;
    }
    Bitmap { width: size as u32, height: size as u32, bgra: out }
}
