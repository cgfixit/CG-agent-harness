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
