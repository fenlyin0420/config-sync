//! `update` against a release served the way github.com serves one.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};

use crate::support::*;

/// A release served the way github.com serves one: `/releases/latest`
/// redirects to `/releases/tag/<tag>`, and that tag carries the raw binary of
/// this machine as `cfgsync-<slug>`. One request per connection, no TLS, so
/// the `update` tests need no network.
struct Release {
    base: String,
}

fn asset_of(slug: &str) -> String {
    format!("cfgsync-{slug}{}", if cfg!(windows) { ".exe" } else { "" })
}

impl Release {
    /// `payload` is the body of the asset, or `None` for a tag that carries
    /// only an archive -- the 404 an old release answers.
    fn start(tag: &str, slug: &str, payload: Option<Vec<u8>>) -> Release {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tag_path = format!("/releases/tag/{tag}");
        let asset_path = format!("/releases/download/{tag}/{}", asset_of(slug));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let response: Vec<u8> = match path.as_str() {
                    "/releases/latest" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: {tag_path}\r\n\
                         Content-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    p if p == tag_path => {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                    }
                    p if p == asset_path && payload.is_some() => {
                        let body = payload.clone().unwrap();
                        let mut out = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\
                             Content-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        out.extend_from_slice(&body);
                        out
                    }
                    _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                          Connection: close\r\n\r\n"
                        .to_vec(),
                };
                let _ = stream.write_all(&response);
            }
        });
        Release {
            base: format!("http://127.0.0.1:{port}"),
        }
    }

    fn asset_slug() -> &'static str {
        "linux-x86_64"
    }
}

/// `update` against a local release, installing over `exe` (a fixture path)
/// instead of the test binary itself.
fn run_update(f: &Fixture, release: &Release, exe: &str, args: &[&str]) -> Output {
    let exe = f.root.join(exe);
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(&f.root)
        .env("CFGSYNC_CENTRAL", f.central())
        .env("CFGSYNC_UPDATE_BASE", &release.base)
        .env("CFGSYNC_UPDATE_TARGET", Release::asset_slug())
        .env("CFGSYNC_UPDATE_EXE", &exe)
        .env_remove("CFGSYNC_OS");
    cmd.output().unwrap()
}

#[test]
fn update_installs_the_latest_release_over_the_running_binary() {
    let f = Fixture::new("update-install");
    // a payload has to be a program, so the fixture serves the real one
    let real = std::fs::read(BIN).unwrap();
    let release = Release::start("v99.0.0", Release::asset_slug(), Some(real.clone()));
    f.write("bin/cfgsync", "the old binary");

    let out = run_update(&f, &release, "bin/cfgsync", &["update"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("-> 99.0.0"), "{stdout}");
    assert!(stdout.contains("installed"), "{stdout}");
    assert_eq!(std::fs::read(f.root.join("bin/cfgsync")).unwrap(), real);
    // the old file is gone, not moved next to the new one
    let left: Vec<String> = std::fs::read_dir(f.root.join("bin"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["cfgsync".to_string()], "{left:?}");
}

#[test]
fn update_leaves_the_binary_alone_when_it_is_already_the_latest() {
    let f = Fixture::new("update-current");
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let release = Release::start(&tag, Release::asset_slug(), None);
    f.write("bin/cfgsync", "the old binary");

    let out = run_update(&f, &release, "bin/cfgsync", &["update"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("is the latest release"), "{stdout}");
    assert_eq!(f.read("bin/cfgsync"), "the old binary");
}

#[test]
fn update_check_and_dry_run_report_without_writing() {
    let f = Fixture::new("update-check");
    let release = Release::start("v99.0.0", Release::asset_slug(), Some(std::fs::read(BIN).unwrap()));

    let out = run_update(&f, &release, "bin/cfgsync", &["update", "--check"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("99.0.0") && stdout.contains("not downloading"), "{stdout}");
    assert!(!f.root.join("bin/cfgsync").exists(), "--check creates nothing");

    let out = run_update(&f, &release, "bin/cfgsync", &["update", "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("would install over"), "{stdout}");
    assert!(!f.root.join("bin/cfgsync").exists(), "a dry run creates nothing");
}

#[test]
fn update_refuses_a_release_without_a_binary_for_this_machine() {
    let f = Fixture::new("update-no-asset");
    // the release carries another target's binary, so this machine's is a 404
    let release = Release::start("v99.0.0", "macos-aarch64", Some(b"MZ".to_vec()));
    f.write("bin/cfgsync", "the old binary");

    let out = run_update(&f, &release, "bin/cfgsync", &["update"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&asset_of(Release::asset_slug())), "{err}");
    assert_eq!(f.read("bin/cfgsync"), "the old binary");
}

#[test]
fn update_refuses_a_download_that_is_not_a_program() {
    let f = Fixture::new("update-html");
    let release = Release::start("v99.0.0", Release::asset_slug(), Some(b"<!DOCTYPE html>".to_vec()));
    f.write("bin/cfgsync", "the old binary");

    let out = run_update(&f, &release, "bin/cfgsync", &["update"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not a program"), "{err}");
    assert_eq!(f.read("bin/cfgsync"), "the old binary");
}

#[test]
fn update_needs_cfgsync_and_not_a_repo_name() {
    let f = Fixture::new("update-usage");
    let out = f.run(&["update", "app"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: cfgsync update"));
}
