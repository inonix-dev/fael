//! Stop-event blocks: commits without a row, edits after the last row, bug
//! signals without an issue row — and fail-open on garbage.

use super::{commit, fael, fael_at, json, repo, state, transcript};

#[test]
fn stop_blocks_commit_without_row_then_allows() {
    let d = repo();
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
    let s2 = state(&d).join("s2");
    let (ok, out, _) = fael_at(&s2, &d, &["hook", "stop", "--client", "claude"], &input);
    assert!(ok && !out.contains("block"), "{out}");

    // a new session with a new commit and no row blocks again
    let t = transcript(&d, "t2.jsonl");
    commit(&d, "tweak again");
    let input = format!(r#"{{"cwd":{},"session":{}}}"#, json(&d), json(&t));
    let (ok, out, _) = fael_at(&s2, &d, &["hook", "stop"], &input);
    assert!(ok && out.contains(r#""block":true"#), "{out}");
}

#[test]
fn stop_blocks_edits_after_last_row() {
    // agents told never to commit: the edit hook's list is the work signal
    let d = repo();
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
    let d = repo();
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
    let (ok, out, _) = fael_at(&state(&d).join("s2"), &d, &["hook", "stop"], &input);
    assert!(ok && out.contains(r#""block":false"#), "{out}");
}

#[test]
fn stop_fails_open() {
    let d = repo();
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
