//! Terminal styling: one small palette, resolved once, no crate.
//!
//! Color is off unless the stream it is written to is a terminal, so a pipe, a
//! redirect, a test or `$(cfgsync cd)` never sees an escape byte. `NO_COLOR`
//! (`no-color.org`) and `CLICOLOR` / `CLICOLOR_FORCE` are honored, and
//! `CFGSYNC_COLOR=always|never` overrides the lot.
//!
//! Only structure is colored -- the `[command]` tag, a status letter, an op
//! label, a result word -- never a path or a value, so the text stays readable
//! when the escapes are stripped.

use std::io::IsTerminal;
use std::sync::OnceLock;

const RESET: &str = "\x1b[0m";

/// how wide a "key : value" row's first column may grow before it stops growing
const CELL_MAX: usize = 44;

static OUT: OnceLock<bool> = OnceLock::new();
static ERR: OnceLock<bool> = OnceLock::new();

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn color(tty: bool) -> bool {
    match env("CFGSYNC_COLOR").as_deref() {
        Some("always" | "yes" | "true" | "1") => return true,
        Some("never" | "no" | "false" | "0") => return false,
        _ => {}
    }
    if env("NO_COLOR").is_some() {
        return false;
    }
    if env("CLICOLOR").as_deref() == Some("0") {
        return false;
    }
    if env("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
        return true;
    }
    // a windows console needs virtual-terminal processing turned on by
    // someone else; without a known-VT host it would print the escapes raw
    if cfg!(windows) && env("WT_SESSION").is_none() && env("TERM").is_none() {
        return false;
    }
    tty
}

fn out_on() -> bool {
    *OUT.get_or_init(|| color(std::io::stdout().is_terminal()))
}

fn err_on() -> bool {
    *ERR.get_or_init(|| color(std::io::stderr().is_terminal()))
}

fn paint(code: &str, text: &str, on: bool) -> String {
    match on {
        true => format!("\x1b[{code}m{text}{RESET}"),
        false => text.to_string(),
    }
}

// ------------------------------------------------------------------ stdout

pub fn tag(cmd: &str) -> String {
    paint("1;36", &format!("[{cmd}]"), out_on())
}

pub fn dim(text: &str) -> String {
    paint("2", text, out_on())
}

pub fn bold(text: &str) -> String {
    paint("1", text, out_on())
}

/// something that happened, or nothing left to do
pub fn ok(text: &str) -> String {
    paint("32", text, out_on())
}

/// something the user should look at
pub fn warn(text: &str) -> String {
    paint("33", text, out_on())
}

pub fn bad(text: &str) -> String {
    paint("31", text, out_on())
}

pub fn accent(text: &str) -> String {
    paint("35", text, out_on())
}

/// one row of a block: two-space indent, `cell` padded to `width`, then the
/// detail in dim. The caller passes cells already gathered, so a block lines up.
pub fn row(width: usize, cell: &str, detail: &str) -> String {
    format!("  {cell:<width$}  {}", dim(detail))
}

/// the widest cell of a block, capped so one long path cannot push the whole
/// block off the screen
pub fn width<'a>(cells: impl Iterator<Item = &'a str>) -> usize {
    cells.map(|c| c.chars().count()).max().unwrap_or(0).min(CELL_MAX)
}

// ------------------------------------------------------------------ stderr

fn tag_err(cmd: &str, code: &str) -> String {
    paint(code, &format!("[{cmd}]"), err_on())
}

pub fn warn_tag() -> String {
    format!("{} {}", tag_err("sync", "1;33"), paint("33", "warning:", err_on()))
}

pub fn fail_tag() -> String {
    format!("{} {}", tag_err("sync", "1;31"), paint("31", "error:", err_on()))
}

/// a `[command]` note that is not a failure: nothing to adopt, nothing to pull
pub fn note_tag(cmd: &str) -> String {
    tag_err(cmd, "1;33")
}

/// a `[command]` line that stops the command
pub fn bad_tag(cmd: &str) -> String {
    tag_err(cmd, "1;31")
}

// ------------------------------------------------------------------ macros

/// stdout, `[cmd] ...`
macro_rules! say {
    ($cmd:expr, $($arg:tt)*) => {
        println!("{} {}", $crate::ui::tag($cmd), format_args!($($arg)*))
    };
}

/// stderr, `[sync] warning: ...`
macro_rules! warn {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::ui::warn_tag(), format_args!($($arg)*))
    };
}

/// stderr, `[sync] error: ...`
macro_rules! fail {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::ui::fail_tag(), format_args!($($arg)*))
    };
}

/// stderr, `[cmd] ...` for a note that is not a failure
macro_rules! note {
    ($cmd:expr, $($arg:tt)*) => {
        eprintln!("{} {}", $crate::ui::note_tag($cmd), format_args!($($arg)*))
    };
}

/// stderr, `[cmd] ...` for a refusal
macro_rules! refuse {
    ($cmd:expr, $($arg:tt)*) => {
        eprintln!("{} {}", $crate::ui::bad_tag($cmd), format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_of_rows_shares_one_column() {
        let cells = ["config.ini", "agent/settings.json"];
        let w = width(cells.iter().copied());
        assert_eq!(w, "agent/settings.json".len());
        assert!(row(w, "config.ini", "no changes").starts_with("  config.ini "));
    }

    #[test]
    fn one_long_cell_does_not_push_the_block_off_screen() {
        let long = "x".repeat(CELL_MAX * 2);
        assert_eq!(width(std::iter::once(long.as_str())), CELL_MAX);
    }

    #[test]
    fn no_color_turns_painting_off() {
        assert_eq!(paint("32", "yes", false), "yes");
        assert_eq!(paint("32", "yes", true), "\x1b[32myes\x1b[0m");
    }
}
