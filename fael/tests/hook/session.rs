//! Session-start kickoff + gitignore warning, and the read push (each row
//! once per session).

use super::{fael, json, lock, repo, state};

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

    // kickoff drops rows whose files are all gone, so the file must exist
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
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
    assert!(!out.contains("gitignored"), "{out}");
    // the cached check-ignore answer follows .gitignore both ways
    std::fs::write(d.join(".gitignore"), ".fael/\n").unwrap();
    let (_, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(out.contains("gitignored"), "{out}");
    std::fs::remove_file(d.join(".gitignore")).unwrap();
    let (_, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(!out.contains("gitignored"), "{out}");
    // .git/info/exclude is a deliberate local-only choice — no warning
    std::fs::write(d.join(".git/info/exclude"), ".fael/log/\n").unwrap();
    let (_, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(!out.contains("gitignored"), "{out}");

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

    // with a session, a row pushes once — the second read of the same file is silent
    let input = format!(
        r#"{{"cwd":{},"session_id":"s1","tool_input":{{"file_path":{}}}}}"#,
        json(&d),
        json(&f)
    );
    let (_, out, _) = fael(&d, &["hook", "read", "--client", "claude"], &input);
    assert!(out.contains("login loops"), "{out}");
    let (ok, out, _) = fael(&d, &["hook", "read", "--client", "claude"], &input);
    assert!(ok && !out.contains("login loops"), "{out}");

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
    assert!(ok && out.contains("7 injections"), "{out}");
    let (ok, out, _) = fael(&d, &["stats", "--json"], "");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        ok && v["events"] == 7 && v["by_event"]["read"]["events"] == 3,
        "{out}"
    );
}
