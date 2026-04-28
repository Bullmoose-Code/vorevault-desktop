//! Path helpers for filtering hidden files and directories from upload.

use std::path::Path;

/// Returns true if `path` is hidden by either the cross-platform dot-prefix
/// convention or (on Windows) the FILE_ATTRIBUTE_HIDDEN attribute.
pub fn is_hidden(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        if name.starts_with('.') {
            return true;
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
                return true;
            }
        }
    }

    false
}

/// Returns true if `path` itself or any directory between `root` (exclusive)
/// and `path` (exclusive) is hidden. The watch root itself is never considered
/// hidden — the user picked it knowingly.
///
/// If `path` is not under `root`, returns false.
pub fn has_hidden_ancestor_or_self(path: &Path, root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };

    let mut current = root.to_path_buf();
    for component in rel.components() {
        current.push(component);
        if is_hidden(&current) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn is_hidden_true_for_dot_prefix() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join(".dotfile");
        fs::write(&p, b"x").unwrap();
        assert!(is_hidden(&p));
    }

    #[test]
    fn is_hidden_false_for_normal_filename() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("clip.mp4");
        fs::write(&p, b"x").unwrap();
        assert!(!is_hidden(&p));
    }

    #[test]
    fn is_hidden_false_for_filename_with_dot_in_middle() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("foo.bar.baz");
        fs::write(&p, b"x").unwrap();
        assert!(!is_hidden(&p));
    }

    #[test]
    fn ancestor_check_skips_file_in_dot_dir() {
        let dir = TempDir::new().unwrap();
        let hidden_dir = dir.path().join(".cache");
        fs::create_dir(&hidden_dir).unwrap();
        let file = hidden_dir.join("clip.mp4");
        fs::write(&file, b"x").unwrap();
        assert!(has_hidden_ancestor_or_self(&file, dir.path()));
    }

    #[test]
    fn ancestor_check_skips_file_nested_under_dot_dir() {
        let dir = TempDir::new().unwrap();
        let hidden_dir = dir.path().join(".cache");
        let nested = hidden_dir.join("sub");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("clip.mp4");
        fs::write(&file, b"x").unwrap();
        assert!(has_hidden_ancestor_or_self(&file, dir.path()));
    }

    #[test]
    fn ancestor_check_passes_normal_file_in_normal_dir() {
        let dir = TempDir::new().unwrap();
        let normal = dir.path().join("VALORANT");
        fs::create_dir(&normal).unwrap();
        let file = normal.join("clip.mp4");
        fs::write(&file, b"x").unwrap();
        assert!(!has_hidden_ancestor_or_self(&file, dir.path()));
    }

    #[test]
    fn ancestor_check_skips_dot_file_directly_in_root() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(".DS_Store");
        fs::write(&file, b"x").unwrap();
        assert!(has_hidden_ancestor_or_self(&file, dir.path()));
    }

    #[test]
    fn ancestor_check_ignores_hidden_root_itself() {
        let dir = TempDir::new().unwrap();
        let dot_root = dir.path().join(".dotroot");
        fs::create_dir(&dot_root).unwrap();
        let file = dot_root.join("clip.mp4");
        fs::write(&file, b"x").unwrap();
        assert!(!has_hidden_ancestor_or_self(&file, &dot_root));
    }

    #[test]
    fn ancestor_check_returns_false_for_path_outside_root() {
        let dir = TempDir::new().unwrap();
        let other = dir.path().join("elsewhere");
        fs::create_dir(&other).unwrap();
        let file = other.join("clip.mp4");
        fs::write(&file, b"x").unwrap();
        let unrelated_root = dir.path().join("VALORANT");
        fs::create_dir(&unrelated_root).unwrap();
        assert!(!has_hidden_ancestor_or_self(&file, &unrelated_root));
    }

    #[cfg(windows)]
    #[test]
    fn is_hidden_true_for_windows_hidden_attribute() {
        use std::os::windows::ffi::OsStrExt;
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("Thumbs.db");
        fs::write(&p, b"x").unwrap();

        // Set FILE_ATTRIBUTE_HIDDEN via WinAPI.
        let wide: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            extern "system" {
                fn SetFileAttributesW(path: *const u16, attrs: u32) -> i32;
            }
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            let res = SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN);
            assert_ne!(res, 0, "SetFileAttributesW failed");
        }

        assert!(is_hidden(&p));
    }
}
