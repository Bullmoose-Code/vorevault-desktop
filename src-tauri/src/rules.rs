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

/// True iff `candidate` is equal to, an ancestor of, or a descendant of any
/// path in `others`. Caller is responsible for excluding the rule being
/// edited from `others` (so a rule can be re-saved without tripping the
/// check). Comparison is path-prefix based; both sides should be canonical
/// absolute paths.
pub fn is_path_overlap(others: &[&Path], candidate: &Path) -> bool {
    others.iter().any(|o| paths_overlap(o, candidate))
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn overlap_detects_equal_paths() {
        let a = p("/a/b");
        let candidate = p("/a/b");
        let others = vec![a.as_path()];
        assert!(is_path_overlap(&others, &candidate));
    }

    #[test]
    fn overlap_detects_descendant_candidate() {
        let a = p("/a");
        let candidate = p("/a/b/c");
        let others = vec![a.as_path()];
        assert!(is_path_overlap(&others, &candidate));
    }

    #[test]
    fn overlap_detects_ancestor_candidate() {
        let a = p("/a/b/c");
        let candidate = p("/a");
        let others = vec![a.as_path()];
        assert!(is_path_overlap(&others, &candidate));
    }

    #[test]
    fn overlap_returns_false_for_siblings() {
        let a = p("/a/b");
        let b = p("/a/c");
        let candidate = p("/a/d");
        let others = vec![a.as_path(), b.as_path()];
        assert!(!is_path_overlap(&others, &candidate));
    }

    #[test]
    fn overlap_returns_false_for_unrelated_paths() {
        let a = p("/x/y");
        let candidate = p("/totally/elsewhere");
        let others = vec![a.as_path()];
        assert!(!is_path_overlap(&others, &candidate));
    }

    #[test]
    fn overlap_returns_false_for_empty_others() {
        let candidate = p("/a/b");
        assert!(!is_path_overlap(&[], &candidate));
    }

    #[test]
    fn overlap_does_not_match_path_prefix_string_only() {
        // /a/bcd should NOT overlap with /a/bc, even though "/a/bc" is a
        // string prefix of "/a/bcd". Path::starts_with is component-wise.
        let a = p("/a/bc");
        let candidate = p("/a/bcd");
        let others = vec![a.as_path()];
        assert!(!is_path_overlap(&others, &candidate));
    }
}
