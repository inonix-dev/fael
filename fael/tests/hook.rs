//! `fael hook` + `fael stats` against throwaway git repos. Hook tests share
//! `FAEL_STATE_DIR`, which is process-global, so every test holds `LOCK`.

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

/// A transcript file created strictly before the next commit, so the commit
/// is newer than the session start even at 1-second git granularity.
fn transcript(d: &Path, name: &str) -> PathBuf {
    let p = d.join(name);
    std::fs::write(&p, "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    p
}

#[test]
fn stop_blocks_commit_without_row_then_allows() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    let (ok, _, err) = fael(
        &d,
        &["add", "decision", "old choice", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");

    let t = transcript(&d, "t1.jsonl");
    commit(&d, "tweak login");
    let input = format!(r#"{{"cwd":{},"transcript_path":{}}}"#, json(&d), json(&t));
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok, "hook must always exit 0");
    assert!(out.contains(r#""decision":"block""#), "{out}");
    assert!(out.contains("fael add <decision|issue|note>"), "{out}");

    // once per session — the second end lets through
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok && !out.contains("block"), "{out}");

    // a mem row filed for the work lets the turn through (fresh state dir,
    // so this allow comes from the row and not from the dedupe above)
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "tweaked login copy", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d).join("s2")) };
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok && !out.contains("block"), "{out}");

    // a new session with a new commit and no row blocks again
    let t = transcript(&d, "t2.jsonl");
    commit(&d, "tweak again");
    let input = format!(r#"{{"cwd":{},"session":{}}}"#, json(&d), json(&t));
    let (ok, out, _) = fael(&d, &["hook", "stop"], &input);
    assert!(ok && out.contains(r#""block":true"#), "{out}");
}

#[test]
fn stop_blocks_edits_after_last_row() {
    // agents told never to commit: the edit hook's list is the work signal
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    let (ok, _, err) = fael(
        &d,
        &["add", "decision", "old choice", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    let t = transcript(&d, "t1.jsonl");
    for f in ["src/a.rs", "src/b.rs"] {
        std::fs::write(d.join(f), "//\n").unwrap();
    }
    for f in ["src/b.rs", "src/b.rs", "src/a.rs"] {
        let edit = format!(
            r#"{{"cwd":{},"transcript_path":{},"tool_input":{{"file_path":{}}}}}"#,
            json(&d),
            json(&t),
            json(&d.join(f))
        );
        assert!(fael(&d, &["hook", "edit", "--client", "claude"], &edit).0);
    }
    let input = format!(r#"{{"cwd":{},"transcript_path":{}}}"#, json(&d), json(&t));
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok && out.contains("2 file(s) edited"), "{out}");
    assert!(out.contains("--files src/b.rs,src/a.rs"), "{out}");

    // a row for the work lets it through — same state dir: the dedupe is keyed
    // on the last row, and no edit follows the new one
    let (ok, _, err) = fael(
        &d,
        &["add", "note", "b.rs added", "--files", "src/b.rs"],
        "",
    );
    assert!(ok, "{err}");
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok && !out.contains("block"), "{out}");

    // work after that row blocks once more, listing only the later edit
    std::thread::sleep(std::time::Duration::from_millis(5));
    let edit = format!(
        r#"{{"cwd":{},"transcript_path":{},"tool_input":{{"file_path":"src/a.rs"}}}}"#,
        json(&d),
        json(&t)
    );
    assert!(fael(&d, &["hook", "edit", "--client", "claude"], &edit).0);
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(
        ok && out.contains("1 file(s) edited") && !out.contains("src/b.rs"),
        "{out}"
    );
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok && !out.contains("block"), "{out}");
}

#[test]
fn stop_bug_signal_needs_issue_row() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    let (ok, _, err) = fael(
        &d,
        &["add", "decision", "old choice", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");

    let t = d.join("t.jsonl");
    std::fs::write(
        &t,
        r#"{"message":{"role":"assistant","content":[{"type":"text","text":"I found a bug in login"}]}}"#,
    )
    .unwrap();
    let input = format!(r#"{{"cwd":{},"session":{}}}"#, json(&d), json(&t));
    let (ok, out, _) = fael(&d, &["hook", "stop"], &input);
    assert!(ok && out.contains("fael add issue"), "{out}");

    // any client: the assistant text arrives in the Event, no transcript needed
    let neutral = format!(
        r#"{{"cwd":{},"session":"2020-01-01T00:00:00Z","text":"the schema and the docs are out of sync"}}"#,
        json(&d)
    );
    let (ok, out, _) = fael(&d, &["hook", "stop"], &neutral);
    assert!(ok && out.contains("out of sync"), "{out}");

    // an issue row filed in this session clears it — same transcript, fresh
    // state dir, so the allow comes from the row and not from the dedupe
    let (ok, out, _) = fael(&d, &["stats", "--json"], "");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        ok && v["stop_blocks"]["stop-bug"]
            == serde_json::json!({"blocks": 2, "followed_by_row": 0}),
        "{out}"
    );
    let (ok, _, err) = fael(
        &d,
        &["add", "issue", "login loops", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    // stats sees the issue that followed the block
    let (_, out, _) = fael(&d, &["stats"], "");
    assert!(
        out.contains("stop-bug: 2 block(s) → 2 followed by a row"),
        "{out}"
    );
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d).join("s2")) };
    let (ok, out, _) = fael(&d, &["hook", "stop"], &input);
    assert!(ok && out.contains(r#""block":false"#), "{out}");
}

#[test]
fn stop_fails_open() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    // garbage in, no repoadopted log, already-fired hook — all allow, all exit 0
    let (ok, out, _) = fael(&d, &["hook", "stop"], "not json");
    assert!(ok && out.contains(r#""block":false"#), "{out}");
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "nope"], "{}");
    assert!(ok, "{out}");
    let (ok, out, _) = fael(
        &d,
        &["hook", "stop", "--client", "claude"],
        r#"{"cwd":"/","stop_hook_active":true}"#,
    );
    assert!(ok && out.is_empty(), "{out}");
}

#[test]
fn session_start_and_read_push() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    // empty log = silent, not an error
    let (ok, out, _) = fael(
        &d,
        &["hook", "session-start", "--client", "claude"],
        r#"{}"#,
    );
    assert!(ok && out.is_empty(), "{out}");

    let (ok, _, err) = fael(
        &d,
        &["add", "issue", "login loops", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    let input = format!(r#"{{"cwd":{}}}"#, json(&d));
    let (ok, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(
        ok && out.contains("SessionStart") && out.contains("login loops"),
        "{out}"
    );

    // read: claude shape in, PostToolUse context out
    let f = d.join("src/a.rs");
    std::fs::write(&f, "// a\n").unwrap();
    let input = format!(
        r#"{{"cwd":{},"tool_input":{{"file_path":{}}}}}"#,
        json(&d),
        json(&f)
    );
    let (ok, out, _) = fael(&d, &["hook", "read", "--client", "claude"], &input);
    assert!(
        ok && out.contains("PostToolUse") && out.contains("login loops"),
        "{out}"
    );

    // neutral shape: Event in, Reply out · a file outside the repo pushes nothing
    let input = format!(r#"{{"cwd":{},"files":["src/a.rs"]}}"#, json(&d));
    let (ok, out, _) = fael(&d, &["hook", "read"], &input);
    assert!(ok && out.contains("login loops"), "{out}");
    let (ok, out, _) = fael(&d, &["hook", "read"], r#"{"cwd":"/","files":["x.rs"]}"#);
    assert!(
        ok && out.contains(r#""block":false"#) && !out.contains("context"),
        "{out}"
    );

    // every injection is accounted, per machine
    let (ok, out, _) = fael(&d, &["stats"], "");
    assert!(ok && out.contains("3 injections"), "{out}");
    let (ok, out, _) = fael(&d, &["stats", "--json"], "");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        ok && v["events"] == 3 && v["by_event"]["read"]["events"] == 2,
        "{out}"
    );
}

fn json(v: &Path) -> String {
    serde_json::Value::String(v.to_string_lossy().into_owned()).to_string()
}

#[test]
fn codex_apply_patch_edits_and_last_message() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    let (ok, _, err) = fael(
        &d,
        &["add", "decision", "old choice", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    let t = transcript(&d, "rollout.jsonl");
    std::fs::write(d.join("src/b.rs"), "//\n").unwrap();
    let patch = "*** Begin Patch\n*** Add File: src/b.rs\n+//\n*** End Patch\n";
    let edit = serde_json::json!({"cwd": d, "transcript_path": t, "tool_name": "apply_patch",
        "tool_input": {"command": patch}})
    .to_string();
    assert!(fael(&d, &["hook", "edit", "--client", "codex"], &edit).0);
    // a bug claim in the last message is checked before the edit rule
    let stop = serde_json::json!({"cwd": d, "transcript_path": t, "stop_hook_active": false,
        "last_assistant_message": "Found a bug: the parser is broken on empty input."})
    .to_string();
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "codex"], &stop);
    assert!(
        ok && out.contains(r#""decision":"block""#) && out.contains("issue"),
        "{out}"
    );
    // next turn's message is clean: the edit rule sees the apply_patch file
    let stop = serde_json::json!({"cwd": d, "transcript_path": t,
        "last_assistant_message": "Added b.rs."})
    .to_string();
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "codex"], &stop);
    assert!(ok && out.contains("--files src/b.rs"), "{out}");
}

#[test]
fn claude_notebook_edit_is_recorded() {
    let _g = lock();
    let d = repo();
    unsafe { std::env::set_var("FAEL_STATE_DIR", state(&d)) };
    let (ok, _, err) = fael(
        &d,
        &["add", "decision", "old choice", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    let t = transcript(&d, "t.jsonl");
    std::fs::write(d.join("src/n.ipynb"), "{}").unwrap();
    let edit = serde_json::json!({"cwd": d, "transcript_path": t,
        "tool_input": {"notebook_path": d.join("src/n.ipynb")}})
    .to_string();
    assert!(fael(&d, &["hook", "edit", "--client", "claude"], &edit).0);
    let stop = serde_json::json!({"cwd": d, "transcript_path": t}).to_string();
    let (ok, out, _) = fael(&d, &["hook", "stop", "--client", "claude"], &stop);
    assert!(ok && out.contains("src/n.ipynb"), "{out}");
}
