//! `ling.ignore` — a project-level ignore file (one glob pattern per line,
//! `#` comments and blank lines skipped) consulted by anything in `lingfu`
//! that walks "the whole project": `seed`'s manifest (on top of, not
//! instead of, `git ls-files`) and `publish`'s tarball (which otherwise has
//! no ignore-file awareness at all — it only skips a hardcoded handful of
//! build/VCS directory names by exact match).
//!
//! The point is a single place to keep locally-sensitive or oversized
//! material — keyfiles, Shamir recovery shares, huge local-only assets —
//! out of *every* process that reads a project's files by default, so nothing
//! has to remember to exclude them by hand each time (e.g. a `seed`
//! keyfile sitting inside the very tree `seed` hashes, or an oversized
//! asset getting swept into a `publish` tarball).

use std::path::Path;

pub const IGNORE_FILE: &str = "ling.ignore";

/// Load patterns from `<root>/ling.ignore`; empty if the file doesn't exist.
pub fn load(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join(IGNORE_FILE)) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// True if `rel` (forward-slash path relative to the project root) matches
/// any pattern: as a full-path glob, as a bare basename glob (gitignore
/// style -- `*.key` matches at any depth), or as a directory prefix (a
/// trailing-`/` pattern).
pub fn is_ignored(patterns: &[String], rel: &str) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let basename = rel.rsplit('/').next().unwrap_or(rel);
    patterns.iter().any(|pat| {
        if let Some(dir) = pat.strip_suffix('/') {
            return rel == dir || rel.starts_with(&format!("{dir}/"));
        }
        glob_match(pat, rel) || (!pat.contains('/') && glob_match(pat, basename))
    })
}

// ─── glob matching ──────────────────────────────────────────────────────────
// Ported from `ling`'s `expand_includes` matcher (src/main.rs) so `[includes]`
// and `ling.ignore` patterns behave identically: `*`/`?` stay within a path
// segment, `**` matches across any number of segments.

fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let seg: Vec<&str> = path.split('/').collect();
    seg_match(&pat, &seg)
}

fn seg_match(pat: &[&str], seg: &[&str]) -> bool {
    match pat.first() {
        None => seg.is_empty(),
        Some(&"**") => (0..=seg.len()).any(|i| seg_match(&pat[1..], &seg[i..])),
        Some(p) => match seg.first() {
            Some(s) if wildcard(p.as_bytes(), s.as_bytes()) => seg_match(&pat[1..], &seg[1..]),
            _ => false,
        },
    }
}

/// `*` (any run) and `?` (one char) matching within a single segment.
fn wildcard(pat: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while si < s.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_filename_matches_any_depth() {
        let pats = vec!["x.wav".to_string()];
        assert!(is_ignored(&pats, "x.wav"));
        assert!(is_ignored(&pats, "packages/tensormap/x.wav"));
        assert!(!is_ignored(&pats, "y.wav"));
    }

    #[test]
    fn glob_matches_extension() {
        let pats = vec!["*.key".to_string()];
        assert!(is_ignored(&pats, "seed-recovery-1.key"));
        assert!(is_ignored(&pats, "secrets/root.key"));
        assert!(!is_ignored(&pats, "seed.wav"));
    }

    #[test]
    fn directory_prefix_matches() {
        let pats = vec!["secrets/".to_string()];
        assert!(is_ignored(&pats, "secrets/a.txt"));
        assert!(is_ignored(&pats, "secrets/nested/b.txt"));
        assert!(!is_ignored(&pats, "not-secrets/a.txt"));
    }
}
