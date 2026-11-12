//! sync.rules: one glob per line, `#` comments, `[section]` lines ignored.
//!
//!   path-glob              matches Op.path in every managed file
//!   file-glob:path-glob    e.g.  settings.json:defaultTools   *.ini:platform.*
//!                                /agent/settings.json:defaultTools
//!   !...                   the same line, vetoed: this key is never synced
//!
//! Matching is fnmatch-compatible: `*`, `?`, `[seq]`, `[!seq]`. A file-glob
//! follows the adopt.rules reading of a path: no `/` matches the file name at
//! any depth, a `/` in it anchors the glob to the source repo root, and `**`
//! spans directories. A `path-glob` also covers the keys below it, so `!secrets`
//! drops a whole object (or ini section) and not just one leaf.

use std::path::Path;

pub const NAME: &str = "sync.rules";

pub fn load(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !s.starts_with('#'))
        .filter(|s| !(s.starts_with('[') && s.ends_with(']')))
        .map(str::to_string)
        .collect()
}

/// Where one key path belongs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// machine owned -> `platform/<os>/` overlay
    Owned,
    /// shared -> `base/`
    Shared,
    /// `!` line: neither, the key stays in the app's own file and is never
    /// committed to the central repo
    Excluded,
}

/// `file` is the managed file's path relative to the source repo root, so a
/// rule can scope itself to one file anywhere in the tree.
pub fn route(pats: &[String], file: &str, path: &str) -> Route {
    let mut owned = false;
    let mut segs: Vec<&str> = path.split('.').collect();
    loop {
        let candidate = segs.join(".");
        for pat in pats {
            let (negated, body) = match pat.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, pat.trim()),
            };
            let (fileglob, pathglob) = match body.split_once(':') {
                Some((f, p)) => (f.trim(), p.trim()),
                None => ("*", body),
            };
            if !path_match(fileglob, file) || !hits(pathglob, &candidate) {
                continue;
            }
            // a veto always wins: "never sync this" is the narrower intent
            if negated {
                return Route::Excluded;
            }
            owned = true;
        }
        // a rule naming a parent covers its children, so `!secrets` drops the
        // whole subtree and `platform.*` owns everything below the section
        if segs.is_empty() {
            break;
        }
        segs.pop();
    }
    if owned {
        Route::Owned
    } else {
        Route::Shared
    }
}

/// Strictly deleting a sync.rules line means "not machine-owned", which is why
/// this is not a plain `!route.is_excluded()`.
pub fn is_excluded(pats: &[String], file: &str, path: &str) -> bool {
    route(pats, file, path) == Route::Excluded
}

/// A file-glob: `*.json` matches the name at any depth, anything with a `/`
/// (leading or not) is anchored to the repo root, `**` spans directories.
pub fn path_match(pattern: &str, path: &str) -> bool {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return false;
    }
    if !pattern.contains('/') {
        return glob_match(pattern, segs[segs.len() - 1]);
    }
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    !pat.is_empty() && segs_match(&pat, &segs)
}

/// `**` spans zero or more whole segments; everything else is fnmatch inside
/// one segment, so `*` never crosses a `/`.
pub fn segs_match<P: AsRef<str>, Q: AsRef<str>>(pat: &[P], path: &[Q]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0].as_ref() == "**" {
        return (0..=path.len()).any(|k| segs_match(&pat[1..], &path[k..]));
    }
    !path.is_empty()
        && glob_match(pat[0].as_ref(), path[0].as_ref())
        && segs_match(&pat[1..], &path[1..])
}

/// `keybind#2` is matched by a `keybind` rule as well: the `#N` an adapter adds
/// for a repeated key addresses one occurrence, it does not hide the key.
fn hits(pathglob: &str, candidate: &str) -> bool {
    if glob_match(pathglob, candidate) {
        return true;
    }
    match candidate.rsplit_once('#') {
        Some((head, tail))
            if !head.is_empty() && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) =>
        {
            glob_match(pathglob, head)
        }
        _ => false,
    }
}

pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = name.chars().collect();
    match_here(&p, &s)
}

fn match_here(p: &[char], s: &[char]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    match p[0] {
        '*' => {
            let mut i = 1;
            while i < p.len() && p[i] == '*' {
                i += 1;
            }
            (0..=s.len()).any(|k| match_here(&p[i..], &s[k..]))
        }
        '?' => !s.is_empty() && match_here(&p[1..], &s[1..]),
        '[' => {
            if s.is_empty() {
                return false;
            }
            match class_end(p) {
                Some((end, negated, ranges)) => {
                    let hit = ranges.iter().any(|(lo, hi)| *lo <= s[0] && s[0] <= *hi);
                    if hit != negated {
                        match_here(&p[end + 1..], &s[1..])
                    } else {
                        false
                    }
                }
                None => s[0] == '[' && match_here(&p[1..], &s[1..]),
            }
        }
        c => !s.is_empty() && s[0] == c && match_here(&p[1..], &s[1..]),
    }
}

/// `[seq]` -> (index of ']', negated, ranges)
fn class_end(p: &[char]) -> Option<(usize, bool, Vec<(char, char)>)> {
    let mut i = 1;
    let negated = matches!(p.get(i), Some('!') | Some('^'));
    if negated {
        i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((i, negated, ranges));
        }
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            ranges.push((p[i], p[i + 2]));
            i += 3;
        } else {
            ranges.push((p[i], p[i]));
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*.ini", "config.ini"));
        assert!(glob_match("core.*", "core.theme"));
        assert!(!glob_match("core.*", "limits.timeout"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("[abc]x", "bx"));
        assert!(glob_match("[!abc]x", "zx"));
        assert!(glob_match("a?c", "abc"));
    }

    #[test]
    fn routing() {
        let pats = vec!["core.*".to_string(), "keymap.properties:*".to_string()];
        assert_eq!(route(&pats, "config.ini", "core.theme"), Route::Owned);
        assert_eq!(route(&pats, "config.ini", "limits.timeout"), Route::Shared);
        assert_eq!(route(&pats, "keymap.properties", "search"), Route::Owned);
        assert_eq!(route(&pats, "config.json", "search"), Route::Shared);
    }

    #[test]
    fn a_file_glob_without_a_slash_matches_any_depth() {
        let pats = vec!["settings.json:defaultTools".to_string()];
        assert_eq!(route(&pats, "settings.json", "defaultTools"), Route::Owned);
        assert_eq!(route(&pats, "agent/settings.json", "defaultTools"), Route::Owned);
        assert_eq!(route(&pats, "agent/other.json", "defaultTools"), Route::Shared);
    }

    #[test]
    fn a_file_glob_with_a_slash_is_anchored_to_the_root() {
        let pats = vec!["/agent/settings.json:defaultTools".to_string()];
        assert_eq!(route(&pats, "agent/settings.json", "defaultTools"), Route::Owned);
        assert_eq!(route(&pats, "settings.json", "defaultTools"), Route::Shared);
        assert_eq!(route(&pats, "deep/agent/settings.json", "defaultTools"), Route::Shared);
        // the leading '/' is decoration: the slash itself anchors
        let same = vec!["agent/settings.json:defaultTools".to_string()];
        assert_eq!(route(&same, "agent/settings.json", "defaultTools"), Route::Owned);
        assert_eq!(route(&same, "deep/agent/settings.json", "defaultTools"), Route::Shared);
    }

    #[test]
    fn a_bang_line_vetoes_the_key_and_its_subtree() {
        let pats = vec!["core.*".to_string(), "!/agent/settings.json:apiKey".to_string()];
        assert_eq!(route(&pats, "config.ini", "core.theme"), Route::Owned);
        assert_eq!(
            route(&pats, "agent/settings.json", "apiKey"),
            Route::Excluded
        );
        // subtree: a vetoed object covers the keys below it
        assert_eq!(
            route(&pats, "agent/settings.json", "apiKey.value"),
            Route::Excluded
        );
        // a positive line naming the parent is what a veto has to pre-empt
        let both = vec!["secrets.*".to_string(), "!secrets".to_string()];
        assert_eq!(route(&both, "a.json", "secrets.token"), Route::Excluded);
        // the veto is scoped to its file like any other line
        assert_eq!(route(&pats, "settings.json", "apiKey"), Route::Shared);
    }

    #[test]
    fn a_rule_naming_a_key_covers_its_ordinals() {
        let pats = vec!["keybind".to_string(), "!font-family".to_string()];
        assert_eq!(route(&pats, "agent/config", "keybind"), Route::Owned);
        assert_eq!(route(&pats, "agent/config", "keybind#2"), Route::Owned);
        assert_eq!(route(&pats, "agent/config", "keybind#13"), Route::Owned);
        assert_eq!(route(&pats, "agent/config", "font-family#2"), Route::Excluded);
        assert_eq!(route(&pats, "agent/config", "theme"), Route::Shared);
    }

    #[test]
    fn path_globs_keep_star_inside_one_segment() {
        assert!(path_match("agent/*.json", "agent/settings.json"));
        assert!(!path_match("agent/*.json", "agent/deep/settings.json"));
        assert!(path_match("**/settings.json", "settings.json"));
        assert!(path_match("**/settings.json", "a/b/settings.json"));
        assert!(path_match("/agent/settings.json", "agent/settings.json"));
        assert!(!path_match("", "agent/settings.json"));
    }
}
