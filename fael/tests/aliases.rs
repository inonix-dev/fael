//! Chunk 1 (§3 criteria 1 + 3): a row filed under `a.rs` still pushes at the
//! new path after a committed `git mv`, and across a rename chain `a→b→c`.
//! Hook tests share `FAEL_STATE_DIR`, which is process-global, so every test
//! holds `LOCK` and points the state at a scratch dir — usage lines from
//! throwaway repos must never land in the real `usage.jsonl`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
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
    let d = std::env::temp_dir().join(format!("fael-alias-{}", fael_core::ulid()));
    std::fs::create_dir_all(d.join("src")).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Alias Test"],
        &["config", "user.email", "alias@example.com"],
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
    d
}

fn git(d: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(d)
            .status()
            .unwrap()
            .success()
    );
}

fn commit_all(d: &Path, msg: &str) {
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", msg]);
}

fn json(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// `fael add` + return the row id. The file must exist — `add` does not check
/// that (chunk 4 does), but the hook push and kickoff only make sense for one.
fn add(d: &Path, files: &str) -> String {
    std::fs::write(d.join(files), "// v1\n").unwrap();
    commit_all(d, format!("add {files}").as_str());
    let (ok, out, err) = fael(d, &["add", "decision", "choice about a", "--files", files], "");
    assert!(ok, "{err}");
    out.split_whitespace().next().unwrap().to_string()
}

fn hook_read(d: &Path, files: &str) -> String {
    let input = format!(
        r#"{{"cwd":{},"files":[{}]}}"#,
        json(&d.to_string_lossy()),
        json(files)
    );
    let (ok, out, _) = fael(d, &["hook", "read"], &input);
    assert!(ok, "hook must always exit 0");
    out
}

fn session_start(d: &Path) {
    let input = format!(r#"{{"cwd":{}}}"#, json(&d.to_string_lossy()));
    let (ok, _, _) = fael(d, &["hook", "session-start"], &input);
    assert!(ok, "hook must always exit 0");
}

#[test]
fn rename_pushes_at_the_new_path() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", d.join("state")) };
    let id = add(&d, "src/a.rs");

    git(&d, &["mv", "src/a.rs", "src/b.rs"]);
    commit_all(&d, "rename a to b");

    // no session-start ran: the first read builds the cache itself
    let out = hook_read(&d, "src/b.rs");
    assert!(out.contains(&id[..8]), "{out}");

    let (ok, out, err) = fael(&d, &["find", "--files", "src/b.rs"], "");
    assert!(ok, "{err}");
    assert!(out.contains(&id[..8]), "{out}");

    // the cache and its gitignore landed where they should
    assert!(d.join(".fael/cache/aliases.json").is_file());
    let ignore = std::fs::read_to_string(d.join(".fael/.gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l.trim() == "cache/"), "{ignore}");
}

#[test]
fn rename_chain_pushes_at_the_end() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", d.join("state")) };
    let id = add(&d, "src/a.rs");

    git(&d, &["mv", "src/a.rs", "src/b.rs"]);
    commit_all(&d, "a to b");
    git(&d, &["mv", "src/b.rs", "src/c.rs"]);
    commit_all(&d, "b to c");

    let out = hook_read(&d, "src/c.rs");
    assert!(out.contains(&id[..8]), "{out}");
}

#[test]
fn session_start_refresh_picks_up_later_renames() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", d.join("state")) };
    let id = add(&d, "src/a.rs");

    // first read builds the cache at this HEAD …
    assert!(hook_read(&d, "src/a.rs").contains(&id[..8]));
    // … a rename committed after that is picked up by the next session-start
    git(&d, &["mv", "src/a.rs", "src/b.rs"]);
    commit_all(&d, "rename a to b");
    session_start(&d);
    assert!(hook_read(&d, "src/b.rs").contains(&id[..8]));
}

#[test]
fn resolve_false_returns_to_pre_resolver_matching() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", d.join("state")) };
    let id = add(&d, "src/a.rs");
    std::fs::write(d.join(".fael/config.toml"), "resolve = false\n").unwrap();

    git(&d, &["mv", "src/a.rs", "src/b.rs"]);
    commit_all(&d, "rename a to b");
    session_start(&d);

    let (ok, out, _) = fael(&d, &["find", "--files", "src/b.rs"], "");
    assert!(ok && out.is_empty(), "{out}");
    let (ok, out, err) = fael(&d, &["find", "--files", "src/a.rs"], "");
    assert!(ok && out.contains(&id[..8]), "{err}");
}
