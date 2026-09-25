//! Chunk 4 (PLAN-fael-path-integrity): `fael add` without `--files` inherits
//! the session's edited files, and mistyped paths are rejected with the
//! closest name. `FAEL_STATE_DIR` is process-global, so every test holds `LOCK`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn fael(dir: &Path, args: &[&str], stdin: &str) -> (bool, String, String) {
    fael_as(dir, args, stdin, None)
}

/// `session` = the `CLAUDE_CODE_SESSION_ID` the caller runs under; `None`
/// clears it, so the agent running these tests never leaks its own id in.
fn fael_as(
    dir: &Path,
    args: &[&str],
    stdin: &str,
    session: Option<&str>,
) -> (bool, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"));
    c.args(args).current_dir(dir);
    match session {
        Some(s) => c.env("CLAUDE_CODE_SESSION_ID", s),
        None => c.env_remove("CLAUDE_CODE_SESSION_ID"),
    };
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
    let d = std::env::temp_dir().join(format!("fael-write-{}", fael_core::ulid()));
    std::fs::create_dir_all(d.join("src")).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Write Test"],
        &["config", "user.email", "write@example.com"],
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
    // the edit hook only records for adopted repos, and the tests below edit
    // strictly after the first row, so the derive filter (`at > last row`)
    // never ties at ms precision
    unsafe { std::env::set_var("FAEL_STATE_DIR", d.join("state")) };
    let (ok, _, err) = fael(&d, &["add", "note", "seed", "--files", "doc:seed"], "");
    assert!(ok, "{err}");
    std::thread::sleep(std::time::Duration::from_millis(5));
    d
}

/// Record an edit-hook event for `session` touching `files` (absolute paths).
fn edit(d: &Path, session: &str, files: &[PathBuf]) {
    let input = serde_json::json!({"cwd": d, "session": session, "files": files}).to_string();
    let (ok, _, err) = fael(d, &["hook", "edit"], &input);
    assert!(ok, "hook must always exit 0: {err}");
    std::thread::sleep(std::time::Duration::from_millis(5));
}

fn row_files(d: &Path, needle: &str) -> Vec<String> {
    let (_, out, _) = fael(d, &["find", "--json", "--all"], "");
    out.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["text"].as_str().is_some_and(|t| t.contains(needle)))
        .and_then(|v| {
            v["files"].as_array().map(|a| {
                a.iter()
                    .filter_map(|f| f.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

#[test]
fn add_without_files_derives_session_edits() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    edit(&d, "s1", &[d.join("src/a.rs")]);
    let (ok, _, err) = fael(&d, &["add", "note", "derived row"], "");
    assert!(ok, "{err}");
    assert_eq!(row_files(&d, "derived row"), ["src/a.rs"]);
}

#[test]
fn add_without_files_or_edits_still_requires_files() {
    let _g = lock();
    let d = repo();
    let (ok, _, err) = fael(&d, &["add", "note", "nothing to derive from"], "");
    assert!(!ok && err.contains("files is required"), "{err}");
}

#[test]
fn edits_before_the_last_row_do_not_derive() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    edit(&d, "s1", &[d.join("src/a.rs")]);
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "covers a.rs", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    // no edits since that row — the older edit must not leak into the next row
    let (ok, _, err) = fael(&d, &["add", "note", "nothing new"], "");
    assert!(!ok && err.contains("files is required"), "{err}");
}

#[test]
fn typo_is_rejected_with_the_closest_name() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/auth.rs"), "//\n").unwrap();
    let (ok, _, err) = fael(&d, &["add", "note", "x", "--files", "src/autn.rs"], "");
    assert!(!ok, "a near-miss of an existing file must not be filed");
    assert!(
        err.contains("src/auth.rs") && err.contains("did you mean"),
        "{err}"
    );
}

#[test]
fn unmatched_path_without_a_close_sibling_is_warned_not_blocked() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/live.rs"), "//\n").unwrap();
    // rows about deleted or planned files stay fileable — kickoff/doctor,
    // not the write path, judge those
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "about gone", "--files", "src/gone.rs"],
        "",
    );
    assert!(ok, "{err}");
    assert!(err.contains("matches nothing on disk"), "{err}");
    assert_eq!(row_files(&d, "about gone"), ["src/gone.rs"]);
}

#[test]
fn edited_then_deleted_file_passes_silently() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/tmp.rs"), "//\n").unwrap();
    edit(&d, "s1", &[d.join("src/tmp.rs")]);
    std::fs::remove_file(d.join("src/tmp.rs")).unwrap();
    // in this session's edits: evidence, even though it is gone from disk
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "about tmp", "--files", "src/tmp.rs"],
        "",
    );
    assert!(ok, "{err}");
    assert!(!err.contains("matches nothing"), "{err}");
}

#[test]
fn glob_and_anchor_pass_without_evidence() {
    let _g = lock();
    let d = repo();
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "pattern row", "--files", "src/*.rs"],
        "",
    );
    assert!(ok, "{err}");
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "anchor row", "--files", "doc:pricing"],
        "",
    );
    assert!(ok, "{err}");
}

#[test]
fn untracked_shell_made_file_passes() {
    let _g = lock();
    let d = repo();
    // the edit hook never saw it (no hook event), but git status knows it
    std::fs::write(d.join("src/shell.rs"), "//\n").unwrap();
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "shell file", "--files", "src/shell.rs"],
        "",
    );
    assert!(ok, "{err}");
}

#[test]
fn edits_from_another_worktree_do_not_derive() {
    let _g = lock();
    let d = repo();
    let other = repo();
    std::fs::write(other.join("src/b.rs"), "// b\n").unwrap();
    // the edit happened in `other`, filed from `d` — same state dir, same
    // session id, different worktree
    edit(&other, "shared", &[other.join("src/b.rs")]);
    let (ok, _, err) = fael(&d, &["add", "note", "nothing here"], "");
    assert!(!ok && err.contains("files is required"), "{err}");
}

#[test]
fn mcp_add_without_files_derives_too() {
    use std::io::Write as _;
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    edit(&d, "s1", &[d.join("src/a.rs")]);
    let msgs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add","arguments":{"kind":"note","text":"via mcp"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add","arguments":{"kind":"note","text":"nope"}}}"#,
    ];
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"))
        .arg("mcp")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(&d)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all((msgs.join("\n") + "\n").as_bytes())
        .unwrap();
    let out = String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap();
    let r: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(r.len(), 2, "{out}");
    // first call derives src/a.rs; the second finds no new edits and fails
    assert_eq!(r[0]["result"]["isError"], false, "{out}");
    assert!(
        r[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("files is required"),
        "{out}"
    );
    assert_eq!(row_files(&d, "via mcp"), ["src/a.rs"]);
}

#[test]
fn concurrent_sessions_never_derive_each_others_files() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    std::fs::write(d.join("src/b.rs"), "// b\n").unwrap();
    edit(&d, "s1", &[d.join("src/a.rs")]);
    // Claude's hook keys the session by transcript path; the CLI sees the id
    edit(
        &d,
        "/home/u/.claude/projects/p/s2.jsonl",
        &[d.join("src/b.rs")],
    );
    // two active sessions and no id: ambiguous, so no derive
    let (ok, _, err) = fael(&d, &["add", "note", "whose"], "");
    assert!(!ok && err.contains("files is required"), "{err}");
    // the caller's own session only, matched through the transcript stem
    let (ok, _, err) = fael_as(&d, &["add", "note", "mine is b"], "", Some("s2"));
    assert!(ok, "{err}");
    assert_eq!(row_files(&d, "mine is b"), ["src/b.rs"]);
}

#[test]
fn force_files_a_planned_sibling_of_an_existing_file() {
    let _g = lock();
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "//\n").unwrap();
    let (ok, _, err) = fael(&d, &["add", "note", "plan b", "--files", "src/b.rs"], "");
    assert!(!ok && err.contains("--force"), "{err}");
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "plan b", "--files", "src/b.rs", "--force"],
        "",
    );
    assert!(ok && err.contains("matches nothing on disk"), "{err}");
    assert_eq!(row_files(&d, "plan b"), ["src/b.rs"]);
}

#[test]
fn staged_rename_source_is_not_evidence() {
    let _g = lock();
    let d = repo();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&d)
                .status()
                .unwrap()
                .success()
        );
    };
    std::fs::create_dir_all(d.join("xyzc")).unwrap();
    std::fs::write(d.join("xyzc/foo.rs"), "//\n").unwrap();
    git(&["add", "xyzc/foo.rs"]);
    git(&["commit", "-qm", "foo"]);
    std::fs::create_dir_all(d.join("lib")).unwrap();
    git(&["mv", "xyzc/foo.rs", "lib/foo.rs"]);
    // -z prints `R  lib/foo.rs\0src/foo.rs`; the source must not be read as
    // an entry of its own (`c/foo.rs` after chopping the status columns)
    let (ok, _, err) = fael(&d, &["add", "note", "chopped", "--files", "c/foo.rs"], "");
    assert!(ok && err.contains("matches nothing on disk"), "{err}");
}
