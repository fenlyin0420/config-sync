//! `path.rules`: where one managed source repo lives on each platform.
//!
//! The directory name in the central repo is only a label (`nvim`, `ghostty`).
//! The same app does not sit at the same path everywhere -- nvim is
//! `~/.config/nvim` on linux and `%LOCALAPPDATA%\nvim` on windows -- so the
//! path cannot be the directory name. This file is the mapping, committed with
//! everything else:
//!
//!   linux = ~/.config/nvim
//!   win   = %LOCALAPPDATA%\nvim
//!
//! `~`, `$VAR` and `%VAR%` are expanded when the file is read, so every machine
//! picks its own line and the other lines are just documentation.

use std::path::{Path, PathBuf};

use crate::central::normalize_os;
use crate::fsx::{read, write};

pub const NAME: &str = "path.rules";

pub fn template() -> String {
    "\
# path.rules -- 这个应用在每个平台上的真实目录
# 语法：一行一个平台，<os> = <路径>；'#' 注释
#   '~' 是家目录；$VAR 和 %VAR% 读环境变量（%APPDATA% 这类 Windows 写法也认）
# 中心仓库的目录名只是个标签，这张表才是真正的路径映射
# add 会写下当前平台的那一行；换到别的机器，在那台机器上 add 一次补上它自己的行
# 同一平台已经有一行、路径又不一样时 add 会拒绝覆盖，改路径请用 edit -p
# 示例：
# linux = ~/.config/nvim
# win   = %LOCALAPPDATA%\\nvim
# macos = ~/.config/nvim
"
        .to_string()
}

/// `(platform, path)` pairs in file order, paths expanded.
pub fn load(dir: &Path) -> Vec<(String, PathBuf)> {
    read(&dir.join(NAME)).lines().filter_map(parse).collect()
}

/// The path this platform uses, `None` when the file has no line for it.
pub fn path_for(dir: &Path, os: &str) -> Option<PathBuf> {
    load(dir)
        .into_iter()
        .find(|(platform, _)| platform == os)
        .map(|(_, path)| path)
}

/// Record `os = path`, replacing the line this platform already has and leaving
/// every other line and comment alone. Creates the file from the template when
/// there is none yet.
pub fn set(dir: &Path, os: &str, source: &Path) {
    let file = dir.join(NAME);
    let mut text = read(&file);
    if text.trim().is_empty() {
        text = template();
    }
    let line = format!("{os} = {}", compact(source));
    let mut written = false;
    let mut out: Vec<String> = Vec::new();
    for existing in text.lines() {
        match platform_of(existing) {
            Some(p) if p == os && !written => {
                out.push(line.clone());
                written = true;
            }
            _ => out.push(existing.to_string()),
        }
    }
    if !written {
        out.push(line);
    }
    let mut text = out.join("\n");
    text.push('\n');
    write(&file, &text);
}

/// The platform a non-comment `os = path` line names, normalized.
fn platform_of(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (os, path) = line.split_once('=')?;
    if path.trim().is_empty() {
        return None;
    }
    Some(normalize_os(os))
}

fn parse(line: &str) -> Option<(String, PathBuf)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (os, path) = line.split_once('=')?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some((normalize_os(os), expand(path)))
}

/// `~`, `$VAR`, `${VAR}` and `%VAR%` -> a path. An unknown variable is left
/// alone, so a typo shows up as a missing directory instead of `/`.
pub fn expand(text: &str) -> PathBuf {
    let text = expand_vars(text.trim().trim_matches('"'));
    if let Some(rest) = text.strip_prefix('~') {
        if let Some(home) = home() {
            let rest = rest.trim_start_matches(['/', '\\']);
            return if rest.is_empty() { home } else { home.join(rest) };
        }
    }
    PathBuf::from(text)
}

/// The way `set` writes a path down: home collapsed to `~`, `/` separators, so
/// the committed file is readable and survives being copied to another machine.
pub fn compact(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if let Some(home) = home() {
        let home = home.to_string_lossy().replace('\\', "/");
        let home = home.trim_end_matches('/');
        if !home.is_empty() {
            if text == home {
                return "~".to_string();
            }
            let head = text.get(..home.len()).unwrap_or("");
            if head.eq_ignore_ascii_case(home) && text.as_bytes().get(home.len()) == Some(&b'/') {
                return format!("~/{}", &text[home.len() + 1..]);
            }
        }
    }
    text
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn expand_vars(text: &str) -> String {
    let mut out = String::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '%' => {
                let mut name = String::new();
                let mut closed = false;
                for c in it.by_ref() {
                    if c == '%' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                match (closed, std::env::var(&name)) {
                    (true, Ok(value)) => out.push_str(&value),
                    (true, Err(_)) => out.push_str(&format!("%{name}%")),
                    (false, _) => out.push_str(&format!("%{name}")),
                }
            }
            '$' => {
                let braced = it.peek() == Some(&'{');
                if braced {
                    it.next();
                }
                let mut name = String::new();
                while let Some(&c) = it.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        it.next();
                    } else {
                        break;
                    }
                }
                if braced && it.peek() == Some(&'}') {
                    it.next();
                }
                match std::env::var(&name) {
                    Ok(value) if !name.is_empty() => out.push_str(&value),
                    _ => {
                        out.push('$');
                        if braced {
                            out.push('{');
                        }
                        out.push_str(&name);
                        if braced {
                            out.push('}');
                        }
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cfgsync-paths-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_platform_line_is_read_back_and_replaced_in_place() {
        let dir = scratch("set");
        let source = PathBuf::from("/srv/nvim");
        set(&dir, "linux", &source);
        assert_eq!(path_for(&dir, "linux").unwrap(), source);
        // a second platform appends, the first line keeps its place
        set(&dir, "win", Path::new("/srv/nvim-win"));
        let text = read(&dir.join(NAME));
        let linux = text.lines().position(|l| l.starts_with("linux = ")).unwrap();
        let win = text.lines().position(|l| l.starts_with("win = ")).unwrap();
        assert!(linux < win, "{text}");
        // the same platform is rewritten, not duplicated
        set(&dir, "linux", Path::new("/srv/other"));
        let text = read(&dir.join(NAME));
        let live = |prefix: &str| {
            text.lines()
                .filter(|l| l.starts_with(prefix))
                .collect::<Vec<_>>()
        };
        assert_eq!(live("linux = ").len(), 1, "{text}");
        assert_eq!(live("linux = "), ["linux = /srv/other"], "{text}");
        assert_eq!(live("win = ").len(), 1, "{text}");
    }

    #[test]
    fn comments_and_blank_lines_survive_a_set() {
        let dir = scratch("comments");
        write(&dir.join(NAME), "# keep me\n\nlinux = /a\n");
        set(&dir, "linux", Path::new("/b"));
        assert_eq!(read(&dir.join(NAME)), "# keep me\n\nlinux = /b\n");
    }

    #[test]
    fn an_unknown_platform_has_no_path() {
        let dir = scratch("unknown");
        set(&dir, "linux", Path::new("/a"));
        assert!(path_for(&dir, "win").is_none());
        assert!(path_for(&dir, "freebsd").is_none());
    }

    #[test]
    fn variables_and_tilde_expand() {
        std::env::set_var("CFGSYNC_TEST_DIR", "/srv/x");
        assert_eq!(expand("$CFGSYNC_TEST_DIR/a"), PathBuf::from("/srv/x/a"));
        assert_eq!(expand("${CFGSYNC_TEST_DIR}/a"), PathBuf::from("/srv/x/a"));
        assert_eq!(expand("%CFGSYNC_TEST_DIR%/a"), PathBuf::from("/srv/x/a"));
        // unknown names stay visible instead of collapsing to an empty path
        assert_eq!(expand("%NOPE_NOT_SET%/a"), PathBuf::from("%NOPE_NOT_SET%/a"));
        if let Some(home) = home() {
            assert_eq!(expand("~/a"), home.join("a"));
        }
    }

    #[test]
    fn compact_collapses_home_and_separators() {
        let Some(home) = home() else { return };
        assert_eq!(compact(&home.join("a").join("b")), "~/a/b");
        assert_eq!(compact(Path::new("/srv/x")), "/srv/x");
    }

    #[test]
    fn the_template_examples_are_comments() {
        let dir = scratch("template");
        write(&dir.join(NAME), &template());
        assert!(load(&dir).is_empty(), "{}", read(&dir.join(NAME)));
    }
}
