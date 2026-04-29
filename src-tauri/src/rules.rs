//! Multi-watched-folder routing: rule definition, path-based lookup, overlap
//! detection, client-side tag normalization. Pure logic; no I/O.

use serde::{Deserialize, Serialize};
use std::path::Path;

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

/// Find the rule whose `path` is an ancestor (or equal) of `file_path`.
/// Returns the first match. Overlap is forbidden at save time, so there is
/// at most one match. Returns `None` when `file_path` is outside every
/// rule's root (e.g. event from a freshly-removed root that the OS hasn't
/// dropped yet).
pub fn find_rule_for_path<'a>(rules: &'a [WatchRule], file_path: &Path) -> Option<&'a WatchRule> {
    rules.iter().find(|r| {
        let root = Path::new(&r.path);
        file_path == root || file_path.starts_with(root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn rule(id: &str, path: &str) -> WatchRule {
        WatchRule {
            id: id.to_string(),
            path: path.to_string(),
            vault_folder_id: None,
            vault_folder_label: None,
            tags: vec![],
        }
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

    #[test]
    fn find_returns_matching_rule_for_descendant_file() {
        let rules = vec![rule("r1", "/a"), rule("r2", "/b")];
        let file = p("/a/sub/clip.mp4");
        let found = find_rule_for_path(&rules, &file);
        assert_eq!(found.map(|r| r.id.as_str()), Some("r1"));
    }

    #[test]
    fn find_returns_matching_rule_for_exact_root() {
        let rules = vec![rule("r1", "/a")];
        let file = p("/a");
        let found = find_rule_for_path(&rules, &file);
        assert_eq!(found.map(|r| r.id.as_str()), Some("r1"));
    }

    #[test]
    fn find_returns_none_when_no_rule_matches() {
        let rules = vec![rule("r1", "/a"), rule("r2", "/b")];
        let file = p("/c/clip.mp4");
        assert!(find_rule_for_path(&rules, &file).is_none());
    }

    #[test]
    fn find_returns_none_for_empty_rules() {
        let file = p("/a/clip.mp4");
        assert!(find_rule_for_path(&[], &file).is_none());
    }

    #[test]
    fn find_does_not_string_prefix_match() {
        // /apex_clips should not match a rule rooted at /apex.
        let rules = vec![rule("r1", "/apex")];
        let file = p("/apex_clips/clip.mp4");
        assert!(find_rule_for_path(&rules, &file).is_none());
    }
}
