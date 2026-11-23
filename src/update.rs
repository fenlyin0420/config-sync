//! `cfgsync update`: fetch the latest release and replace the running binary.
//!
//! This is the one command that talks to the network and the one that writes
//! outside the central repo -- it finds the newest tag, downloads the raw
//! binary built for this machine (`cfgsync-linux-x86_64`,
//! `cfgsync-windows-x86_64.exe`) and swaps it in for the binary that is
//! running, wherever that binary lives. Nothing of the central repo or of a
//! source repo is read or written.
//!
//! The tag comes from `/releases/latest` on the release page: it redirects to
//! `/releases/tag/<tag>`, so one `curl` resolves it, with no API and no rate
//! limit to be locked out of. The asset is then the release's own
//! `/releases/download/<tag>/<name>`. `curl` does the fetching -- a static
//! binary has no TLS stack, and a tool that already shells out to `git` can
//! shell out to a downloader rather than carry a crate for it.
//!
//! Three knobs exist so the end-to-end tests can run against a local release
//! instead of the real one, and are honored exactly like a real run otherwise:
//!
//!   CFGSYNC_UPDATE_BASE    the release base URL, instead of github.com's
//!   CFGSYNC_UPDATE_TARGET  the target slug, instead of the host's
//!   CFGSYNC_UPDATE_EXE     the file to install over, instead of this one

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ui;

const REPO: &str = "fenlyin0420/config-sync";
const BASE: &str = "https://github.com/fenlyin0420/config-sync";

const ENV_BASE: &str = "CFGSYNC_UPDATE_BASE";
const ENV_TARGET: &str = "CFGSYNC_UPDATE_TARGET";
const ENV_EXE: &str = "CFGSYNC_UPDATE_EXE";

/// The version this binary was built as -- what `--version` prints.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn run(dry: bool, force: bool, check: bool) -> i32 {
    match look(dry, force, check) {
        Ok(code) => code,
        Err(message) => {
            fail!("{message}");
            1
        }
    }
}

fn look(dry: bool, force: bool, check: bool) -> Result<i32, String> {
    let exe = install_path();
    let slug = target_slug()?;
    let base = base();
    let tag = latest(&base)?;
    let newest = parse_version(&tag);
    if newest.is_empty() {
        return Err(format!(
            "the latest release is tagged {tag}, which carries no version to compare with"
        ));
    }

    // the tag as github spells it addresses the download, `0.6.0` is what a
    // person reads
    let shown = tag.trim_start_matches(['v', 'V']);
    if !is_newer(&newest, &parse_version(current())) {
        if is_newer(&parse_version(current()), &newest) {
            say!(
                "update",
                "{} is newer than the latest release ({shown}) -- nothing to do",
                current()
            );
        } else {
            say!("update", "{} is the latest release", current());
        }
        if !force {
            return Ok(0);
        }
        say!("update", "{}", ui::dim("--force: installing it again"));
    }
    say!(
        "update",
        "cfgsync {} -> {shown}  {}",
        current(),
        ui::dim(&format!("({REPO})"))
    );

    let name = asset_name(&slug);
    let url = format!("{}/releases/download/{tag}/{name}", base.trim_end_matches('/'));
    if check || dry {
        say!("update", "{name}  {}", ui::dim(&url));
        say!(
            "update",
            "{}",
            ui::dim(if dry {
                "would download it and install it over the running binary"
            } else {
                "--check: not downloading it"
            })
        );
        say!("update", "{}", ui::dim(&format!("would install over {}", exe.display())));
        return Ok(0);
    }

    let scratch = Scratch::new()?;
    let fresh = scratch.0.join("asset");
    download(&url, &fresh, &name)?;
    let bytes = std::fs::read(&fresh).map_err(|e| format!("cannot read the download: {e}"))?;
    if !looks_like_a_program(&bytes) {
        return Err(format!(
            "{name} came back as {} bytes that are not a program -- the download is a page, not a binary",
            bytes.len()
        ));
    }
    say!("update", "{name}  {}", ui::dim(&human(bytes.len())));
    install(&exe, &fresh)?;
    say!("update", "installed {}", exe.display());
    Ok(0)
}

// ------------------------------------------------------------------ release

fn base() -> String {
    std::env::var(ENV_BASE)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| BASE.to_string())
}

/// The tag of the latest release: `/releases/latest` redirects to
/// `/releases/tag/<tag>`, so the tag is the last segment of where curl ended
/// up. No API call, so no rate limit and no token to hold.
fn latest(base: &str) -> Result<String, String> {
    let url = format!("{}/releases/latest", base.trim_end_matches('/'));
    let scratch = Scratch::new()?;
    let page = scratch.0.join("latest");
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&page)
        .args(["-w", "%{url_effective}"])
        .arg(&url)
        .output()
        .map_err(|e| {
            format!("cannot run curl: {e}\n  `cfgsync update` downloads the release with curl")
        })?;
    if !out.status.success() {
        return Err(format!("cannot read {url}: {}", first_line(&out.stderr)));
    }
    let landed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let tag = landed
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    // a base that answers 200 without redirecting leaves the literal `latest`
    if tag.is_empty() || tag == "latest" {
        return Err(format!("{url} did not lead to a tag, so the latest release is unknown"));
    }
    Ok(tag)
}

/// The raw binary a release carries, named `cfgsync-<slug>` -- the same asset
/// the workflow uploads next to the archives, which are for unpacking by hand.
fn asset_name(slug: &str) -> String {
    format!("cfgsync-{slug}{}", if cfg!(windows) { ".exe" } else { "" })
}

/// The target slug the release workflow builds: `linux-x86_64` and friends,
/// the same words `path.rules` uses for platforms plus the architecture.
fn target_slug() -> Result<String, String> {
    if let Ok(slug) = std::env::var(ENV_TARGET) {
        if !slug.trim().is_empty() {
            return Ok(slug.trim().to_string());
        }
    }
    let slug = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        (os, arch) => {
            return Err(format!(
                "no prebuilt cfgsync for {os}/{arch} -- build it from source with \
                 `cargo install --path .`"
            ))
        }
    };
    Ok(slug.to_string())
}

/// `0.5.0` / `v0.5.0` / `0.6.0-rc1` -> the numeric parts, the pre-release
/// suffix dropped: an update is about "is there a newer release", not about
/// ranking release candidates.
fn parse_version(text: &str) -> Vec<u32> {
    text.trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn is_newer(a: &[u32], b: &[u32]) -> bool {
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

// ------------------------------------------------------------------ install

fn install_path() -> PathBuf {
    std::env::var_os(ENV_EXE)
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("cfgsync"))
}

/// `<dir>/.<name>.new` -- same directory as the binary, so the step that puts
/// it in place is a rename on one filesystem, never a copy across two.
fn staged(exe: &Path) -> PathBuf {
    sibling(exe, &format!(".{}.new", name_of(exe)))
}

/// What the running binary is renamed to on windows, where the file in use
/// cannot be overwritten: the rename works, the delete afterwards may not.
fn retired(exe: &Path) -> PathBuf {
    sibling(exe, &format!("{}.old", name_of(exe)))
}

fn name_of(exe: &Path) -> String {
    exe.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cfgsync".to_string())
}

fn sibling(exe: &Path, name: &str) -> PathBuf {
    exe.parent().unwrap_or(Path::new(".")).join(name)
}

fn install(exe: &Path, fresh: &Path) -> Result<(), String> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    if !dir.is_dir() {
        return Err(format!("{} is not a directory, so there is nowhere to install", dir.display()));
    }
    // leftovers of an interrupted run, or of a windows binary that was still in
    // use when the last update retired it
    let _ = std::fs::remove_file(staged(exe));
    let _ = std::fs::remove_file(retired(exe));

    let staging = staged(exe);
    std::fs::copy(fresh, &staging).map_err(|e| {
        format!(
            "cannot write {}: {e} -- the directory of {} is not writable by you",
            staging.display(),
            exe.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot make {} executable: {e}", staging.display()))?;
    }
    if cfg!(windows) {
        // a running .exe cannot be deleted, but it can be renamed, so the new
        // one lands under the old name and the old one becomes `<name>.old`
        let old = retired(exe);
        std::fs::rename(exe, &old)
            .map_err(|e| format!("cannot move {} aside: {e}", exe.display()))?;
        if let Err(e) = std::fs::rename(&staging, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(format!("cannot put the new binary at {}: {e}", exe.display()));
        }
        return match std::fs::remove_file(&old) {
            Ok(()) => Ok(()),
            Err(_) => {
                note!(
                    "update",
                    "{} stays until the next run: this session is still running it",
                    old.display()
                );
                Ok(())
            }
        };
    }
    // one rename over the binary: the old one is gone, not left beside it
    std::fs::rename(&staging, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        format!("cannot replace {}: {e}", exe.display())
    })
}

/// An ELF, Mach-O, fat-Mach-O or PE header: a 404 page or a proxy's HTML
/// answer must never land where the binary was.
fn looks_like_a_program(bytes: &[u8]) -> bool {
    [
        b"\x7fELF".as_slice(),
        b"MZ".as_slice(),
        &[0xcf, 0xfa, 0xed, 0xfe],
        &[0xce, 0xfa, 0xed, 0xfe],
        &[0xca, 0xfe, 0xba, 0xbe],
    ]
    .iter()
    .any(|magic| bytes.starts_with(magic))
}

fn human(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

// ------------------------------------------------------------------ network

fn download(url: &str, out: &Path, what: &str) -> Result<(), String> {
    let done = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(out)
        .arg(url)
        .output()
        .map_err(|e| {
            format!("cannot run curl: {e}\n  `cfgsync update` downloads the release with curl")
        })?;
    if done.status.success() {
        return Ok(());
    }
    let why = first_line(&done.stderr);
    if why.contains(" 404") {
        return Err(format!(
            "the latest release has no {what} for this machine -- it may be a build from \
             source, or a release made before that asset existed: {url}"
        ));
    }
    Err(format!("cannot download {url}: {why}"))
}

fn first_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no answer")
        .to_string()
}

/// A download directory, gone when the run ends, however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        let dir = std::env::temp_dir().join(format!("cfgsync-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Vec<u32> {
        parse_version(text)
    }

    #[test]
    fn a_version_reads_as_numbers() {
        assert_eq!(v("0.5.0"), vec![0, 5, 0]);
        assert_eq!(v("v0.6.1"), vec![0, 6, 1]);
        assert_eq!(v(" 1.2 "), vec![1, 2]);
        assert_eq!(v("0.6.0-rc1"), vec![0, 6, 0]);
        assert!(v("latest").is_empty());
    }

    #[test]
    fn newer_is_compared_part_by_part() {
        assert!(is_newer(&v("0.6.0"), &v("0.5.9")));
        assert!(is_newer(&v("0.5.10"), &v("0.5.9")));
        assert!(is_newer(&v("0.5.0.1"), &v("0.5.0")));
        assert!(!is_newer(&v("0.5.0"), &v("0.5.0")));
        assert!(!is_newer(&v("0.5.0"), &v("0.5.0.0")));
        assert!(!is_newer(&v("0.4.9"), &v("0.5.0")));
    }

    #[test]
    fn only_a_program_header_passes_the_download_gate() {
        assert!(looks_like_a_program(b"\x7fELF\x02\x01"));
        assert!(looks_like_a_program(b"MZ\x90\x00"));
        assert!(!looks_like_a_program(b"<!DOCTYPE html>"));
        assert!(!looks_like_a_program(b""));
    }

    #[test]
    fn the_asset_is_the_raw_binary_of_this_target() {
        assert_eq!(
            asset_name("linux-x86_64"),
            format!("cfgsync-linux-x86_64{}", if cfg!(windows) { ".exe" } else { "" })
        );
    }

    #[test]
    fn the_names_it_swaps_beside_the_binary() {
        let exe = Path::new("/usr/local/bin/cfgsync");
        assert_eq!(staged(exe), PathBuf::from("/usr/local/bin/.cfgsync.new"));
        assert_eq!(retired(exe), PathBuf::from("/usr/local/bin/cfgsync.old"));
    }

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(human(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(human(2048), "2 KB");
    }
}
