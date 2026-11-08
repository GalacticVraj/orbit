//! Out-of-process plugin system. OWNER: Agent 7 (Plugin System).
//! CONTRACT — keep these exact public signatures; replace bodies with real implementations.

use orbit_core::{Action, Outcome, Progress};
use std::path::{Path, PathBuf};

/// Folder that holds installed plugins (one sub-folder per plugin).
pub fn dir() -> PathBuf {
    unimplemented!()
}

/// Plugin actions matching this selection. Ids are "plugin:<plugin-id>:<action-id>".
/// Reads manifests on each call (cheap, no watchers, no background work).
pub fn actions_for(paths: &[PathBuf]) -> Vec<Action> {
    let _ = paths;
    unimplemented!()
}

/// Run a plugin action. Worker thread.
pub fn run(id: &str, paths: &[PathBuf], progress: Progress) -> Result<Outcome, String> {
    let _ = (id, paths, progress);
    unimplemented!()
}

/// Install a plugin from a folder or a .zip containing a manifest. Returns the plugin's display name.
pub fn install(source: &Path) -> Result<String, String> {
    let _ = source;
    unimplemented!()
}
