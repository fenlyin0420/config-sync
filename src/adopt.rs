//! adopt.rules: which native files cfgsync manages, written in `.gitignore`
//! syntax.
//!
//! Same pattern language as a `.gitignore` line -- `#` comments, `!` negation,
//! `*`, `?`, `[seq]`, `**`, anchored patterns (a `/` at the start or inside)
//! and a trailing `/` for whole directories. Two deliberate differences: a path
//! no line claims is **not** adopted, and the last matching line wins.

use std::path::Path;

use crate::rules::{glob_match, segs_match};

pub const NAME: &str = "adopt.rules";

#[derive(Clone, Debug)]
pub struct Pattern {
    segs: Vec<String>,
    negated: bool,
    anchored: bool,
    dir_only: bool,
}

impl Pattern {
    /// `None` for blank lines and comments
    pub fn parse(line: &str) -> Option<Pattern> {
        let mut s = line.trim_end();
        if s.trim().is_empty() || s.starts_with('#') {
            return None;
        }
        let negated = match s.strip_prefix('!') {
            Some(rest) => {
                s = rest;
                true
            }
            None => false,
        };
        let dir_only = s.ends_with('/');
        let core = s.trim_end_matches('/');
        // a slash anywhere but the trail anchors the pattern to the repo root
        let anchored = core.contains('/');
        let segs: Vec<String> = core
            .split('/')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        if segs.is_empty() {
            return None;
        }
        Some(Pattern {
            segs,
            negated,
            anchored,
            dir_only,
        })
    }

    /// The path itself or any of its ancestor directories, like
    /// `git check-ignore`: a directory line covers everything below it.
    fn hits(&self, path: &[&str]) -> bool {
        (0..path.len()).rev().any(|k| {
            let is_dir = k + 1 < path.len();
            if self.dir_only && !is_dir {
                return false;
            }
            if self.anchored {
                segs_match(&self.segs, &path[..=k])
            } else {
                glob_match(&self.segs[0], path[k])
            }
        })
    }
}

pub fn load(file: &Path) -> Vec<Pattern> {
    // lossy on purpose: one bad byte must not look like an empty rule table
    let bytes = std::fs::read(file).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    patterns(text.trim_start_matches('\u{feff}'))
}

pub fn patterns(text: &str) -> Vec<Pattern> {
    text.lines().filter_map(Pattern::parse).collect()
}

/// Last matching line wins; nothing matched -> not adopted.
pub fn adopts(pats: &[Pattern], path: &str) -> bool {
    let path: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut adopted = false;
    for pat in pats {
        if pat.hits(&path) {
            adopted = !pat.negated;
        }
    }
    adopted
}

/// The scaffold `add` writes: the syntax, then the files it found but no line
/// claims yet, commented out so adoption stays an explicit decision.
pub fn template(candidates: &[String]) -> String {
    let mut out = String::from(
        "# adopt.rules -- which native files cfgsync manages
# 语法同 .gitignore：一行一个 pattern，'#' 注释，'!' 前缀表示永不采纳。
# 默认：未被任何行命中的文件不采纳；同一路径命中多行时以最后一行为准。
#   config.json       无 '/' -> 匹配任意目录下的同名文件
#   *.ini             任意目录下的 .ini
#   ui/               以 '/' 结尾 -> ui 目录下的所有文件
#   /config.json      以 '/' 开头 -> 只匹配根目录的这个文件
#   a/**/key.json     '**' 跨任意层目录
#   !secret.json      例外：这个永不采纳
# 被采纳的文件内容原样成为中心仓库 base/ 的初始基准，由 harvest 完成采纳。
",
    );
    if !candidates.is_empty() {
        out.push_str("\n# 检测到的候选，去掉行首 '#' 即纳入采纳：\n");
        for name in candidates {
            out.push_str(&format!("# {name}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adopts_with(rules: &str, path: &str) -> bool {
        adopts(&patterns(rules), path)
    }

    #[test]
    fn unanchored_matches_at_any_depth() {
        assert!(adopts_with("*.json", "config.json"));
        assert!(adopts_with("*.json", "ui/deep/key.json"));
        assert!(adopts_with("config.json", "ui/config.json"));
        assert!(!adopts_with("*.json", "ui/key.ini"));
    }

    #[test]
    fn slash_anchors_to_the_root() {
        assert!(adopts_with("/config.json", "config.json"));
        assert!(!adopts_with("/config.json", "ui/config.json"));
        assert!(adopts_with("ui/config.json", "ui/config.json"));
        assert!(!adopts_with("ui/config.json", "deep/ui/config.json"));
    }

    #[test]
    fn star_does_not_cross_a_slash() {
        assert!(adopts_with("ui/*.json", "ui/key.json"));
        assert!(!adopts_with("ui/*.json", "ui/deep/key.json"));
        assert!(adopts_with("ui/**/key.json", "ui/deep/key.json"));
        assert!(adopts_with("ui/**/key.json", "ui/key.json"));
    }

    #[test]
    fn trailing_slash_covers_a_directory_only() {
        assert!(adopts_with("ui/", "ui/deep/key.json"));
        assert!(!adopts_with("ui/", "ui"));
        assert!(adopts_with("ui", "ui"));
    }

    #[test]
    fn last_match_wins_and_bang_rejects() {
        let rules = "*.json\nui/\n!ui/secret.json\n";
        assert!(adopts_with(rules, "config.json"));
        assert!(adopts_with(rules, "ui/key.json"));
        assert!(!adopts_with(rules, "ui/secret.json"));
        assert!(!adopts_with("!config.json\n", "config.json"), "only a rejection");
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        assert_eq!(patterns("# nothing here\n\n   \n").len(), 0);
        assert!(adopts_with("# config.json\n*.json", "config.json"));
    }
}
