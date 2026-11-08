//! Smart (rule-based) batch rename with live preview and undo. OWNER: Agent 8 (Smart Rename).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::{Action, Outcome};
use std::path::PathBuf;

/// "rename.batch" for any non-empty selection, plus "rename.undo" when an undo journal exists.
pub fn actions_for(paths: &[PathBuf]) -> Vec<Action> {
    let _ = paths;
    unimplemented!()
}

/// Open the modeless rename window (UI thread; returns immediately, the host's message loop drives it).
pub fn open(paths: Vec<PathBuf>) {
    let _ = paths;
    unimplemented!()
}

/// Revert the most recent rename batch recorded in the journal. Any thread.
pub fn undo() -> Result<Outcome, String> {
    unimplemented!()
}
