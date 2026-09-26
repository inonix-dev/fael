//! Chunk 4 (PLAN-fael-path-integrity): `fael add` without `--files` inherits
//! the session's edited files, and mistyped paths are rejected with the
//! closest name.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Per-child `FAEL_STATE_DIR` at `<repo root>/state`, so a real session on this
/// machine never leaks in and tests run in parallel without a global env lock.
fn state_env(c: &mut Command, dir: &Path) {
    let root = dir.ancestors().find(|p| p.join(".git").exists()).unwrap();
    c.env("FAEL_STATE_DIR", root.join("state"));
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
    state_env(&mut c, dir);
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
    row_json(d, needle)
        .get("files")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| f.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The newest version of the row whose text contains `needle` — bumps supersede,
/// so the highest id wins.
fn row_json(d: &Path, needle: &str) -> serde_json::Value {
    let (_, out, _) = fael(d, &["find", "--json", "--all"], "");
    out.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["text"].as_str().is_some_and(|t| t.contains(needle)))
        .max_by_key(|v| v["id"].as_str().unwrap_or("").to_string())
        .unwrap()
}

#[test]
fn add_without_files_derives_session_edits() {
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    edit(&d, "s1", &[d.join("src/a.rs")]);
    let (ok, _, err) = fael(&d, &["add", "note", "derived row"], "");
    assert!(ok, "{err}");
    assert_eq!(row_files(&d, "derived row"), ["src/a.rs"]);
}

#[test]
fn add_without_files_or_edits_still_requires_files() {
    let d = repo();
    let (ok, _, err) = fael(&d, &["add", "note", "nothing to derive from"], "");
    assert!(!ok && err.contains("files is required"), "{err}");
}

#[test]
fn edits_before_the_last_row_do_not_derive() {
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
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    edit(&d, "s1", &[d.join("src/a.rs")]);
    let msgs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add","arguments":{"kind":"note","text":"via mcp"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add","arguments":{"kind":"note","text":"nope"}}}"#,
    ];
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"))
        .arg("mcp")
        .env("FAEL_STATE_DIR", d.join("state"))
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

#[test]
fn urgent_and_bump_round_trip() {
    let d = repo();
    // --urgent files at the back of the queue: 1.0, then 2.0
    for text in ["first hot", "second hot"] {
        let (ok, _, err) = fael(
            &d,
            &["add", "issue", text, "--files", "doc:a", "--urgent"],
            "",
        );
        assert!(ok, "{err}");
    }
    assert_eq!(row_json(&d, "first hot")["urgent"].as_f64(), Some(1.0));
    assert_eq!(
        row_json(&d, "second hot")["urgent"].as_f64(),
        Some(2.0)
    );
    // --urgent on a decision is rejected: the queue holds issues
    let (ok, _, err) = fael(
        &d,
        &["add", "decision", "not hot", "--files", "doc:a", "--urgent"],
        "",
    );
    assert!(!ok && err.contains("urgent is for issues"), "{err}");
    // bump the second above the first: half the top → 0.5, only it rewritten
    let id_b = row_json(&d, "second hot")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let id_a = row_json(&d, "first hot")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (ok, out, err) = fael(&d, &["bump", &id_b, "--urgent-before", &id_a], "");
    assert!(ok, "{err}");
    let id_b2 = out.split_whitespace().next().unwrap().to_string();
    assert_ne!(id_b, id_b2);
    let b2 = row_json(&d, "second hot");
    assert_eq!(b2["urgent"].as_f64(), Some(0.5));
    assert_eq!(b2["id"].as_str().unwrap(), id_b2);
    assert_eq!(b2["supersedes"].as_str().unwrap(), id_b);
    // the queue order follows the new number, rendered on the line
    let (_, out, _) = fael(&d, &["find", "--kind", "issue"], "");
    assert!(out.contains("second hot (urgent 0.5)"), "{out}");
    assert!(out.find("second hot").unwrap() < out.find("first hot").unwrap(), "{out}");
    // --not-urgent leaves the queue, --to routes (lowercased)
    let (ok, _, err) = fael(&d, &["bump", &id_b2, "--not-urgent", "--to", "Ploy"], "");
    assert!(ok, "{err}");
    let b3 = row_json(&d, "second hot");
    assert!(b3.get("urgent").is_none(), "{b3}");
    assert_eq!(b3["to"].as_str().unwrap(), "ploy");
}
