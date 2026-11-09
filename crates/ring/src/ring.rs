//! Ring state machine: layout, springs, input semantics. No Win32 here, so it runs offscreen too.

use orbit_core::{Action, Bitmap};
use std::f32::consts::{PI, TAU};
use std::path::PathBuf;

// Geometry in DIPs, relative to the ring centre.
pub const R_GLASS: f32 = 136.0; // menu disc
pub const R_COMPACT: f32 = 84.0; // task / hint / result disc
pub const ORBIT: f32 = 97.0; // node centres
pub const NODE_R: f32 = 23.0;
pub const CENTER_R: f32 = 57.0;
pub const HALF: f32 = R_GLASS + 36.0; // half window size: room for the shadow and the success pulse

const MAX_NODES: usize = 8;
/// How far (DIP) the glass swells toward the hovered node.
const BULGE: f32 = 7.0;
/// Max magnetic pull (DIP) of a hovered node toward the cursor.
const MAGNET: f32 = 5.0;

/// Wrap an angle difference into -PI..PI.
pub fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Damped spring (unit mass). `k` stiffness, `z` damping ratio (<1 overshoots).
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
    pub to: f32,
    pub k: f32,
    pub z: f32,
    pub delay: f32,
}

impl Spring {
    pub fn new(x: f32, k: f32, z: f32) -> Self {
        Spring { x, v: 0.0, to: x, k, z, delay: 0.0 }
    }
    /// Advance by `dt` seconds; returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.delay > 0.0 {
            self.delay -= dt;
            return true;
        }
        if (self.x - self.to).abs() < 1e-3 && self.v.abs() < 1e-2 {
            self.x = self.to;
            self.v = 0.0;
            return false;
        }
        let w = self.k.sqrt();
        let mut t = dt;
        while t > 0.0 {
            let h = t.min(1.0 / 240.0);
            let a = -self.k * (self.x - self.to) - 2.0 * self.z * w * self.v;
            self.v += a * h;
            self.x += self.v * h;
            t -= h;
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Menu,
    Hint,
    Running,
    Done(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Center,
    Node(usize),
}

pub enum Effect {
    None,
    Pick(String),
    Close,
}

pub struct Node {
    pub glyph: char,
    pub label: String,
    pub group: bool,
    pub appear: Spring,
    pub hover: Spring,
    pub press: Spring,
}

pub struct Ring {
    tree: Vec<Action>,
    path: Vec<usize>,
    pub nodes: Vec<Node>,
    pub ghost: Vec<(char, bool)>,
    pub ghost_t: Spring,
    pub hover: Option<Hit>,
    pub name: String,
    pub count: usize,
    pub icon: Option<Bitmap>,
    pub phase: Phase,
    pub closing: bool,
    pub open: Spring,
    pub compact: Spring,
    pub label: String,
    pub label_a: Spring,
    pub center_hover: Spring,
    pub center_press: Spring,
    pub picked: (char, String),
    pub progress: Option<f32>,
    pub arc_start: Spring,
    pub arc_len: Spring,
    pub done: Spring,
    pub check: Spring,
    pub pulse: Spring,
    pub shake: Spring,
    /// Jelly: elliptical deformation amplitude (rest 0), along `wob_dir`.
    pub wobble: Spring,
    pub wob_dir: f32,
    /// Liquid swell of the glass toward the hovered node.
    pub bulge: Spring,
    pub bulge_dir: Spring,
    /// Selection indicator sliding along the rim.
    pub sel: Spring,
    pub sel_a: Spring,
    /// Magnetic offset of the hovered node toward the cursor.
    pub mag: (Spring, Spring),
    /// 0..1: result disc grows to fit a long message.
    pub grow: Spring,
    pub message: String,
    pub time: f32,
}

/// >8 nodes on a level => first 7 + a "More" group holding the rest (recursively).
pub fn fit(mut v: Vec<Action>) -> Vec<Action> {
    for a in &mut v {
        a.children = fit(std::mem::take(&mut a.children));
    }
    if v.len() > MAX_NODES {
        let rest = v.split_off(MAX_NODES - 1);
        v.push(Action::group("More", '\u{E712}', fit(rest)));
    }
    v
}

/// Node angle: start at 12 o'clock, clockwise (screen y grows downwards).
pub fn angle(i: usize, n: usize) -> f32 {
    -PI / 2.0 + i as f32 * TAU / n.max(1) as f32
}

impl Ring {
    pub fn new(paths: &[PathBuf], icon: Option<Bitmap>, actions: Vec<Action>) -> Ring {
        let name = paths
            .first()
            .map(|p| p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned())
            .unwrap_or_default();
        let hint = actions.is_empty();
        let mut r = Ring {
            tree: fit(actions),
            path: Vec::new(),
            nodes: Vec::new(),
            ghost: Vec::new(),
            ghost_t: Spring::new(0.0, 260.0, 1.0),
            hover: None,
            name,
            count: paths.len(),
            icon,
            phase: if hint { Phase::Hint } else { Phase::Menu },
            closing: false,
            open: Spring::new(0.0, 300.0, 0.78),
            compact: Spring::new(if hint { 1.0 } else { 0.0 }, 220.0, 0.86),
            label: String::new(),
            label_a: Spring::new(0.0, 500.0, 1.0),
            center_hover: Spring::new(0.0, 400.0, 1.0),
            center_press: Spring::new(0.0, 700.0, 0.6),
            picked: (' ', String::new()),
            progress: None,
            arc_start: Spring::new(-PI / 2.0, 120.0, 1.0),
            arc_len: Spring::new(0.0, 120.0, 1.0),
            done: Spring::new(0.0, 160.0, 1.0),
            check: Spring::new(0.0, 140.0, 1.0),
            pulse: Spring::new(0.0, 22.0, 1.0),
            shake: Spring::new(0.0, 900.0, 0.18),
            wobble: Spring { v: 1.0, ..Spring::new(0.0, 360.0, 0.3) },
            wob_dir: 0.0,
            bulge: Spring::new(0.0, 260.0, 0.45),
            bulge_dir: Spring::new(0.0, 320.0, 0.8),
            sel: Spring::new(0.0, 280.0, 0.72),
            sel_a: Spring::new(0.0, 400.0, 1.0),
            mag: (Spring::new(0.0, 300.0, 0.6), Spring::new(0.0, 300.0, 0.6)),
            grow: Spring::new(0.0, 200.0, 0.85),
            message: String::new(),
            time: 0.0,
        };
        r.open.to = 1.0;
        r.show_level(0.03);
        r
    }

    fn level(&self) -> &[Action] {
        let mut v = &self.tree[..];
        for &i in &self.path {
            v = &v[i].children;
        }
        v
    }

    /// Glass radius at angle `a` for base radius `rad`: jelly wobble + liquid swell toward the hover.
    pub fn rim(&self, rad: f32, a: f32) -> f32 {
        let w = self.wobble.x * (2.0 * (a - self.wob_dir)).cos();
        let d = wrap(a - self.bulge_dir.x) / 0.45;
        rad * (1.0 + w) + BULGE * self.bulge.x * (-d * d).exp()
    }

    /// Kick the jelly: positive stretches along `dir`.
    pub fn jiggle(&mut self, dir: f32, v: f32) {
        self.wob_dir = dir;
        self.wobble.v += v;
    }

    fn squish(&mut self, v: f32) {
        let dir = match self.hover {
            Some(Hit::Node(i)) => angle(i, self.nodes.len()),
            _ => 0.0,
        };
        self.jiggle(dir, -v);
    }

    /// Cursor moved (DIPs relative to the centre): the hovered node leans toward it.
    pub fn pointer(&mut self, x: f32, y: f32) {
        let (mut tx, mut ty) = (0.0, 0.0);
        if let Some(Hit::Node(i)) = self.hover.filter(|_| self.phase == Phase::Menu) {
            let a = angle(i, self.nodes.len());
            let (dx, dy) = (x - ORBIT * a.cos(), y - ORBIT * a.sin());
            let k = (dx.hypot(dy) * 0.2).min(MAGNET) / dx.hypot(dy).max(1e-3);
            (tx, ty) = (dx * k, dy * k);
        }
        self.mag.0.to = tx;
        self.mag.1.to = ty;
    }

    pub fn in_group(&self) -> bool {
        !self.path.is_empty()
    }

    /// The group whose children are on screen (glyph + label), if any.
    pub fn group(&self) -> Option<(char, &str)> {
        let (&last, parent) = self.path.split_last()?;
        let mut v = &self.tree[..];
        for &i in parent {
            v = &v[i].children;
        }
        Some((v[last].glyph, &v[last].label))
    }

    fn show_level(&mut self, delay: f32) {
        self.ghost = self.nodes.iter().map(|n| (n.glyph, n.group)).collect();
        self.ghost_t = Spring { x: 1.0, to: 0.0, ..self.ghost_t };
        self.nodes = self
            .level()
            .iter()
            .enumerate()
            .map(|(i, a)| Node {
                glyph: a.glyph,
                label: a.label.clone(),
                group: !a.children.is_empty(),
                appear: Spring { to: 1.0, delay: delay + i as f32 * 0.022, ..Spring::new(0.0, 230.0, 0.72) },
                hover: Spring::new(0.0, 420.0, 0.68),
                press: Spring::new(0.0, 900.0, 0.6),
            })
            .collect();
        self.hover = None;
        self.set_hover(None);
        if !self.ghost.is_empty() {
            self.jiggle(0.0, 0.45);
        }
    }

    pub fn set_hover(&mut self, h: Option<Hit>) {
        if self.phase != Phase::Menu {
            return;
        }
        self.hover = h;
        for (i, n) in self.nodes.iter_mut().enumerate() {
            n.hover.to = (h == Some(Hit::Node(i))) as u8 as f32;
        }
        self.center_hover.to = (h == Some(Hit::Center) && self.in_group()) as u8 as f32;
        self.mag.0.to = 0.0;
        self.mag.1.to = 0.0;
        // Rim indicator + liquid swell follow the hovered node along the shortest way round.
        match h {
            Some(Hit::Node(i)) => {
                let a = angle(i, self.nodes.len());
                for (s, shown) in [(&mut self.sel, self.sel_a.x), (&mut self.bulge_dir, self.bulge.x)] {
                    if shown < 0.05 {
                        s.x = a;
                        s.v = 0.0;
                    }
                    s.to = s.x + wrap(a - s.x);
                }
                self.sel_a.to = 1.0;
                self.bulge.to = 1.0;
            }
            _ => {
                self.sel_a.to = 0.0;
                self.bulge.to = 0.0;
            }
        }
        let text = match h {
            Some(Hit::Node(i)) => Some(self.nodes[i].label.clone()),
            Some(Hit::Center) if self.in_group() => Some("Back".to_string()),
            _ => None,
        };
        match text {
            Some(t) => {
                if t != self.label {
                    self.label = t;
                    self.label_a.x = self.label_a.x.min(0.3);
                }
                self.label_a.to = 1.0;
            }
            None => self.label_a.to = 0.0,
        }
    }

    pub fn set_press(&mut self, h: Option<Hit>) {
        for (i, n) in self.nodes.iter_mut().enumerate() {
            n.press.to = (h == Some(Hit::Node(i))) as u8 as f32;
        }
        if h.is_some() {
            self.squish(0.25);
        }
        self.center_press.to = (h == Some(Hit::Center)) as u8 as f32;
    }

    /// Hit test in DIPs relative to the centre.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let d = x.hypot(y);
        if self.phase != Phase::Menu {
            return (d <= R_COMPACT + 22.0 * self.grow.x).then_some(Hit::Center);
        }
        if d <= CENTER_R {
            return Some(Hit::Center);
        }
        // A little beyond the glass still counts: overshooting a node shouldn't drop it.
        if d > R_GLASS + 18.0 || self.nodes.is_empty() {
            return None;
        }
        let n = self.nodes.len();
        let step = TAU / n as f32;
        let a = (y.atan2(x) + PI / 2.0).rem_euclid(TAU);
        Some(Hit::Node(((a / step).round() as usize) % n))
    }

    /// Rotate the keyboard highlight by `d` nodes.
    pub fn rotate(&mut self, d: i32) {
        let n = self.nodes.len() as i32;
        if self.phase != Phase::Menu || n == 0 {
            return;
        }
        let i = match self.hover {
            Some(Hit::Node(i)) => (i as i32 + d).rem_euclid(n),
            _ if d > 0 => 0,
            _ => n - 1,
        };
        self.set_hover(Some(Hit::Node(i as usize)));
    }

    pub fn activate(&mut self, h: Hit) -> Effect {
        match (self.phase, h) {
            (Phase::Menu, Hit::Node(i)) if i < self.nodes.len() => {
                let a = &self.level()[i];
                if !a.children.is_empty() {
                    self.path.push(i);
                    self.show_level(0.05);
                    return Effect::None;
                }
                let id = a.id.clone();
                self.start_task(i);
                Effect::Pick(id)
            }
            (Phase::Menu, Hit::Center) => {
                if self.back() {
                    Effect::None
                } else {
                    Effect::Close
                }
            }
            (Phase::Running, _) | (Phase::Menu, _) => Effect::None,
            _ => Effect::Close,
        }
    }

    /// Up one level; false if already at the root.
    pub fn back(&mut self) -> bool {
        if self.phase != Phase::Menu || self.path.pop().is_none() {
            return false;
        }
        self.show_level(0.05);
        true
    }

    fn start_task(&mut self, i: usize) {
        let n = &self.nodes[i];
        self.picked = (n.glyph, n.label.clone());
        self.jiggle(angle(i, self.nodes.len()), 0.7);
        self.set_hover(None);
        self.phase = Phase::Running;
        for n in &mut self.nodes {
            n.appear.to = 0.0;
            n.appear.delay = 0.0;
            n.appear.z = 1.0;
        }
        self.ghost_t.x = 0.0;
        self.compact.to = 1.0;
        self.label_a.to = 0.0;
        self.center_hover.to = 0.0;
        self.arc_start.x = -PI / 2.0;
        self.arc_len.x = 0.0;
    }

    pub fn set_progress(&mut self, f: f32) {
        if self.phase != Phase::Running {
            return;
        }
        if self.progress.is_none() {
            // Hand over from the spinner: rotate forward to the nearest 12 o'clock.
            let top = -PI / 2.0;
            self.arc_start.to = top + ((self.arc_start.x - top) / TAU).ceil() * TAU;
        }
        self.progress = Some(f.clamp(0.0, 1.0));
        self.arc_len.to = f.clamp(0.0, 1.0) * TAU;
    }

    pub fn finish(&mut self, ok: bool, message: String) {
        self.phase = Phase::Done(ok);
        self.grow.to = (message.chars().count() > 30) as u8 as f32;
        self.message = message;
        if self.progress.is_none() {
            let top = -PI / 2.0;
            self.arc_start.to = top + ((self.arc_start.x - top) / TAU).ceil() * TAU;
        }
        self.progress = Some(1.0);
        self.arc_len.to = TAU;
        self.arc_len.k = 260.0;
        self.done.to = 1.0;
        self.check.to = 1.0;
        self.check.delay = 0.14;
        if ok {
            self.pulse = Spring { x: 0.0, to: 1.0, delay: 0.12, ..self.pulse };
            self.jiggle(PI / 2.0, 0.6);
        } else {
            self.shake.v = 260.0;
        }
        self.compact.to = 1.0;
    }

    pub fn close(&mut self) {
        self.closing = true;
        self.open.to = 0.0;
        self.open.k = 900.0;
        self.open.z = 1.0;
    }

    /// Re-show (task ring brought back): replay the open spring.
    pub fn reopen(&mut self) {
        self.closing = false;
        self.open = Spring { to: 1.0, ..Spring::new(self.open.x.min(0.2), 300.0, 0.78) };
    }

    /// How long a result / hint stays up before auto-dismissing.
    pub fn linger_ms(&self) -> u32 {
        let extra = self.message.chars().count().saturating_sub(24) as u32 * 35;
        match self.phase {
            Phase::Done(true) => 1500 + extra,
            Phase::Done(false) => 3000 + extra,
            _ => 1700,
        }
    }

    pub fn spinning(&self) -> bool {
        self.phase == Phase::Running && self.progress.is_none()
    }

    /// Advance all animation; returns true while anything still moves.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.time += dt;
        if self.spinning() {
            // Indeterminate: arc chases its tail while the whole thing rotates.
            let t = self.time;
            self.arc_start.x = -PI / 2.0 + t * 4.2 + (t * 2.1).sin() * 0.9;
            self.arc_len.x = 1.2 + (0.5 + 0.5 * (t * 2.1 - 1.2).sin()) * 2.2;
            self.arc_start.v = 0.0;
            self.arc_len.v = 0.0;
        }
        let mut moving = self.spinning();
        for s in [
            &mut self.open,
            &mut self.compact,
            &mut self.label_a,
            &mut self.center_hover,
            &mut self.center_press,
            &mut self.ghost_t,
            &mut self.arc_start,
            &mut self.arc_len,
            &mut self.done,
            &mut self.check,
            &mut self.pulse,
            &mut self.shake,
            &mut self.grow,
            &mut self.wobble,
            &mut self.bulge,
            &mut self.bulge_dir,
            &mut self.sel,
            &mut self.sel_a,
            &mut self.mag.0,
            &mut self.mag.1,
        ] {
            moving |= s.step(dt);
        }
        for n in &mut self.nodes {
            moving |= n.appear.step(dt) | n.hover.step(dt) | n.press.step(dt);
        }
        moving
    }

    /// Fully faded out after `close()`.
    pub fn closed(&self) -> bool {
        self.closing && self.open.x <= 0.02
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_motion() {
        let acts: Vec<Action> = (0..12).map(|i| Action::new(&format!("a{i}"), "A", 'x')).collect();
        let tree = fit(acts);
        assert_eq!(tree.len(), 8);
        assert_eq!(tree[7].children.len(), 5);

        let mut r = Ring::new(&[PathBuf::from("x.pdf")], None, tree);
        assert_eq!(r.hit(0.0, -ORBIT), Some(Hit::Node(0))); // top
        assert_eq!(r.hit(ORBIT, 0.0), Some(Hit::Node(2))); // right = 2/8 turn clockwise
        assert_eq!(r.hit(0.0, 0.0), Some(Hit::Center));
        assert_eq!(r.hit(R_GLASS + 30.0, 0.0), None);
        assert!((wrap(TAU - 0.1) + 0.1).abs() < 1e-5);

        r.rotate(-1);
        assert_eq!(r.hover, Some(Hit::Node(7)));
        assert!(matches!(r.activate(Hit::Node(7)), Effect::None)); // "More" opens a sub-ring
        assert!(r.in_group() && r.nodes.len() == 5);
        assert!(r.back() && !r.in_group());
        assert!(matches!(r.activate(Hit::Node(1)), Effect::Pick(id) if id == "a1"));

        let mut n = 0;
        while r.tick(1.0 / 60.0) && n < 600 {
            if r.spinning() {
                r.set_progress(0.5);
            }
            n += 1;
        }
        assert!(n < 600, "springs must settle");
        assert!((r.arc_len.x - PI).abs() < 1e-2);
    }

    /// The frame loop stops only when every spring rests: an open, hovered menu must go idle.
    #[test]
    fn menu_goes_idle() {
        let acts: Vec<Action> = (0..6).map(|i| Action::new(&format!("a{i}"), "A", 'x')).collect();
        let mut r = Ring::new(&[PathBuf::from("x.pdf")], None, acts);
        let settle = |r: &mut Ring| (0..600).take_while(|_| r.tick(1.0 / 60.0)).count();
        assert!(settle(&mut r) < 600, "open must settle");
        r.set_hover(Some(Hit::Node(2)));
        r.pointer(40.0, 30.0);
        assert!(settle(&mut r) < 600, "hover must settle");
        r.rotate(3);
        assert!(settle(&mut r) < 600, "keyboard move must settle");
    }
}
