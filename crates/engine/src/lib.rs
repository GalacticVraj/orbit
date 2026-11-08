//! File conversion engine. OWNER: Agent 4 (File Conversion Engine).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::{Action, Outcome, Progress};
use std::path::PathBuf;

/// Actions applicable to this selection (single file, folder, or many files — possibly mixed kinds).
/// Pure and fast: decide from extensions / is_dir only, never open file contents.
pub fn actions_for(paths: &[PathBuf]) -> Vec<Action> {
    let _ = paths;
    unimplemented!()
}

/// Execute action `id` (one of the ids returned by `actions_for`) on `paths`.
/// Runs on a worker thread that is already COM STA-initialised. Never overwrite inputs:
/// write next to the source via `orbit_core::output_path`. Errors are short, human sentences.
pub fn run(id: &str, paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    let _ = (id, paths, progress);
    unimplemented!()
}
