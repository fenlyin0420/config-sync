//! Filesystem helpers shared by the central repo and the engine.

use std::io::Write;
use std::path::Path;

/// Whole file as text. Lossy on purpose -- one bad byte must not read as an
/// empty file -- and a UTF-8 BOM is tolerated but never propagated.
pub fn read(p: &Path) -> String {
    let bytes = std::fs::read(p).unwrap_or_default();
    String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .to_string()
}

/// Always LF, never a BOM: everything written here is diffed by git.
pub fn write(p: &Path, text: &str) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::File::create(p) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(text.as_bytes()) {
                warn!("cannot write {}: {e}", p.display());
            }
        }
        Err(e) => warn!("cannot write {}: {e}", p.display()),
    }
}

/// `p` relative to `root`, forward slashes, for logs and rule matching
pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

/// Every file under `dir`, relative, `/` separated, sorted.
///
/// `.git` is skipped: a nested checkout is the app's own history, not config.
/// Symlinked files are followed (dotfile setups are full of them), symlinked
/// directories are not (a loop would hang the walk).
pub fn walk(dir: &Path) -> Vec<String> {
    fn go(dir: &Path, prefix: &mut Vec<String>, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut names: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                prefix.push(name);
                go(&entry.path(), prefix, out);
                prefix.pop();
            } else if entry.path().is_file() {
                let mut parts = prefix.clone();
                parts.push(name);
                out.push(parts.join("/"));
            }
        }
    }
    let mut out = Vec::new();
    go(dir, &mut Vec::new(), &mut out);
    out
}
