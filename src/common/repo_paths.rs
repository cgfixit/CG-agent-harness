//! Repo-relative path safety shared by the server (browser-staged `read_files`)
//! and the agentic jail. Port of `utils/repo_paths.py`.

/// Return `target`'s canonical repo-relative form, or `None` if unsafe.
///
/// Rejected outright (never "cleaned"): empty, NUL, absolute (POSIX or Windows
/// drive-qualified), leading `-` (flag injection), a `..` segment, `:` in a
/// segment, or a segment with a trailing space/dot (Windows silently strips
/// those, so `README.md.` and `README.md` would open the same file).
pub fn canonical_repo_relative_path(target: &str) -> Option<String> {
    if target.is_empty() || target.contains('\0') {
        return None;
    }
    let normalized = target.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with('-') || is_windows_absolute(target) {
        return None;
    }
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() {
        return None;
    }
    for part in &parts {
        if *part == ".." || part.contains(':') || part.trim_end_matches([' ', '.']) != *part {
            return None;
        }
    }
    Some(parts.join("/"))
}

/// `C:\...`, `C:/...` or a UNC `\\server\share` form.
pub fn is_windows_absolute(target: &str) -> bool {
    let bytes = target.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    target.starts_with("\\\\") || target.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_relative_paths_and_drops_leading_dot_segments() {
        assert_eq!(
            canonical_repo_relative_path("src/main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            canonical_repo_relative_path("./src/./main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            canonical_repo_relative_path("a\\b\\c").as_deref(),
            Some("a/b/c"),
            "backslashes normalize"
        );
    }

    #[test]
    fn rejects_every_escape_shape() {
        for bad in [
            "",
            "/etc/passwd",
            "-rf",
            "../secret",
            "a/../../secret",
            "a/b:c",
            "trailing space ",
            "trailing.dot.",
            "C:\\Windows",
            "C:/Windows",
            "\\\\server\\share",
            "//server/share",
            "a\0b",
        ] {
            assert!(
                canonical_repo_relative_path(bad).is_none(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn a_dot_only_path_has_no_relative_form() {
        assert!(canonical_repo_relative_path(".").is_none());
        assert!(canonical_repo_relative_path("./.").is_none());
    }

    use proptest::prelude::*;

    proptest::proptest! {
        /// Whatever this function accepts must already be free of the shapes
        /// it exists to reject: no leading '/', no '..' segment, no drive
        /// letter, no UNC prefix, no trailing space/dot on any segment.
        #[test]
        fn accepted_paths_never_contain_a_rejected_shape(raw in "[A-Za-z0-9 _./\\\\:-]{0,40}") {
            if let Some(p) = canonical_repo_relative_path(&raw) {
                prop_assert!(!p.starts_with('/'));
                prop_assert!(!p.split('/').any(|seg| seg == ".." || seg.is_empty()));
                prop_assert!(!is_windows_absolute(&p));
                for seg in p.split('/') {
                    prop_assert_eq!(seg.trim_end_matches([' ', '.']), seg);
                }
            }
        }
    }
}
