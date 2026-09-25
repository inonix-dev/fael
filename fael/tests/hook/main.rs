//! `fael hook` + `fael stats` against throwaway git repos. Hook tests share
//! `FAEL_STATE_DIR`, which is process-global, so every test holds `LOCK`.
//!
//! Thin entry only — the suites sit next to this file:
//! `stop` (turn-end blocks), `session` (session-start + read push),
//! `clients` (codex/claude shapes), `stats` (usage accounting).

mod clients;
mod session;
mod stats;
mod stop;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    // a failed test must not poison the rest — each test sets its own env anyway
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn fael(dir: &Path, args: &[&str], stdin: &str) -> (bool, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"));
    c.args(args).current_dir(dir);
    if !stdin.is_empty() {
        c.stdin(Stdio::piped());
    }
    let mut c = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if !stdin.is_empty() {
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    }
    let o = c.wait_with_output().unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (o.status.success(), s(&o.stdout), s(&o.stderr))
}

fn repo() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-hook-{}", fael_core::ulid()));
    std::fs::create_dir_all(d.join("src")).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Hook Test"],
        &["config", "user.email", "hook@example.com"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&d)
                .status()
                .unwrap()
                .success()
        );
    }
    Command::new("git")
        .args(["commit", "-q", "--allow-empty", "-m", "init"])
        .current_dir(&d)
        .status()
        .unwrap();
    d
}

fn state(d: &Path) -> PathBuf {
    d.join("state")
}

fn commit(d: &Path, msg: &str) {
    std::fs::write(d.join("src/a.rs"), format!("// {msg}\n")).unwrap();
    assert!(
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(d)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-q", "-m", msg])
            .current_dir(d)
            .status()
            .unwrap()
            .success()
    );
}

/// A transcript file created strictly after the previous row and before the next commit, so the commit
/// is newer than the session start even at 1-second git granularity.
fn transcript(d: &Path, name: &str) -> PathBuf {
    // a row filed just before must land in an earlier ms than the birthtime,
    // or the hook (`>=` at ms precision) counts it as this session's row
    std::thread::sleep(std::time::Duration::from_millis(5));
    let p = d.join(name);
    std::fs::write(&p, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    p
}

fn json(v: &Path) -> String {
    serde_json::Value::String(v.to_string_lossy().into_owned()).to_string()
}
