//! Smart (rule-based) batch rename with live preview and undo. OWNER: Agent 8 (Smart Rename).

mod engine;
mod ui;

use orbit_core::{Action, Outcome};
use std::path::PathBuf;

fn journal() -> PathBuf {
    orbit_core::data_dir().join("rename-undo.json")
}

/// "rename.batch" for any non-empty selection, plus "rename.undo" when an undo journal exists.
pub fn actions_for(paths: &[PathBuf]) -> Vec<Action> {
    let mut v = Vec::new();
    if !paths.is_empty() {
        let label = if paths.len() > 1 { "Batch Rename" } else { "Smart Rename" };
        v.push(Action::new("rename.batch", label, '\u{E8AC}'));
    }
    if engine::can_undo(&journal()) {
        v.push(Action::new("rename.undo", "Undo Rename", '\u{E7A7}'));
    }
    v
}

/// Open the modeless rename window (UI thread; returns immediately, the host's message loop drives it).
pub fn open(paths: Vec<PathBuf>) {
    ui::open(paths, journal());
}

/// Revert the most recent rename batch recorded in the journal. Any thread.
pub fn undo() -> Result<Outcome, String> {
    engine::undo_at(&journal())
}
