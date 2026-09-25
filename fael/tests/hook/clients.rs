//! Client shapes: codex apply_patch edits + last message, claude
//! NotebookEdit paths.

use super::{fael, lock, repo, state, transcript};

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
