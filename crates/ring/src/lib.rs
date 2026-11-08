//! The Orbit ring: radial command UI + animation + glass effects. OWNER: Agent 3 (Ring UI + Animation/VFX).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::{Action, Bitmap, Outcome};
use std::path::PathBuf;

/// Handle to the in-flight action shown on the ring. Cheap to clone, Send + Sync; every method
/// may be called from any thread (implementation posts window messages to the UI thread).
#[derive(Clone)]
pub struct Task {
    _private: (),
}

impl Task {
    /// 0.0..=1.0. Ring shows a determinate progress arc; until the first call it shows an indeterminate spinner.
    pub fn progress(&self, fraction: f32) {
        let _ = fraction;
        unimplemented!()
    }
    /// Ok => success micro-animation + `outcome.message` in the centre, then auto-dismiss.
    /// Err => error state with the message (wrapped / truncated nicely), then auto-dismiss.
    pub fn finish(&self, result: Result<Outcome, String>) {
        let _ = result;
        unimplemented!()
    }
    /// Close the ring immediately (used when an action opens its own window).
    pub fn dismiss(&self) {
        unimplemented!()
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
    let _ = (paths, icon, actions, on_pick);
    unimplemented!()
}
