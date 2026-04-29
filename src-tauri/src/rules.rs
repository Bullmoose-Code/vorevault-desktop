//! Multi-watched-folder routing: rule definition, path-based lookup, overlap
//! detection, client-side tag normalization. Pure logic; no I/O.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One configured watched folder + routing target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchRule {
    /// Stable UUID v4 generated at rule creation. Survives edits.
    pub id: String,
    /// Absolute filesystem path to watch.
    pub path: String,
    /// VV folder UUID to upload into. `None` → server falls back to user home.
    pub vault_folder_id: Option<String>,
    /// Cached "Games / Apex" breadcrumb for UI rendering. Refreshed when the
    /// settings UI fetches the folder tree.
    pub vault_folder_label: Option<String>,
    /// Pre-normalized lowercase tag list. Empty vec = no tags.
    pub tags: Vec<String>,
}
