//! Session-start kickoff + gitignore warning, and the read push (each row
//! once per session).

use super::{fael, json, repo};

#[test]
fn session_start_and_read_push() {
    let d = repo();
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
    // chunk 1: no row dump — one count line, the issue pushes on file touch
    assert!(
        ok && out.contains("SessionStart") && out.contains("1 open issue — fael find --kind issue"),
        "{out}"
    );
    assert!(!out.contains("login loops"), "{out}");
    // the explicit-arg path keeps today's kickoff: the row is still there
    let (ok, out, _) = fael(&d, &["kickoff", "src/a.rs"], "");
    assert!(ok && out.contains("login loops"), "{out}");
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

#[test]
fn session_start_decisions_opt_in() {
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    let input = format!(r#"{{"cwd":{}}}"#, json(&d));
    // zero open issues + default config = no count line, only the report line
    let (ok, _, err) = fael(
        &d,
        &[
            "add",
            "decision",
            "use kickoff order",
            "--files",
            "src/a.rs",
        ],
        "",
    );
    assert!(ok, "{err}");
    let (ok, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(ok && !out.contains("open issue"), "{out}");
    assert!(!out.contains("use kickoff order"), "{out}");
    // opt in: the freshest decision lists above the count line
    std::fs::write(
        d.join(".fael/config.toml"),
        "[budget]\nsession_decisions = 1\n",
    )
    .unwrap();
    let (ok, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(ok && out.contains("use kickoff order"), "{out}");
    assert!(!out.contains("open issue"), "{out}");
    // an open issue adds the count line below the decision
    let (ok, _, err) = fael(
        &d,
        &["add", "issue", "login loops", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    let (ok, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(ok && out.contains("use kickoff order"), "{out}");
    assert!(
        out.contains("1 open issue — fael find --kind issue"),
        "{out}"
    );
    assert!(!out.contains("login loops"), "{out}");
}

#[test]
fn session_start_lists_to_me_above_the_count() {
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    let input = format!(r#"{{"cwd":{}}}"#, json(&d));
    // the test repo's writer is Hook Test (hook-test-…): mixed case routes to it
    let (ok, _, err) = fael(
        &d,
        &[
            "add",
            "issue",
            "answer me",
            "--files",
            "src/a.rs",
            "--to",
            "Hook-Test",
        ],
        "",
    );
    assert!(ok, "{err}");
    let (ok, _, err) = fael(
        &d,
        &[
            "add",
            "issue",
            "answer them",
            "--files",
            "src/a.rs",
            "--to",
            "someone",
        ],
        "",
    );
    assert!(ok, "{err}");
    let (ok, _, err) = fael(
        &d,
        &["add", "issue", "answer anyone", "--files", "src/a.rs"],
        "",
    );
    assert!(ok, "{err}");
    let (ok, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(ok, "{out}");
    // mine in full with the suffix, above a count that covers every open issue
    assert!(out.contains("answer me (to: hook-test)"), "{out}");
    assert!(!out.contains("answer them"), "{out}");
    assert!(!out.contains("answer anyone"), "{out}");
    assert!(
        out.contains("1 to you (0 urgent) · 3 open issues — fael find --kind issue"),
        "{out}"
    );
    let (mine, count) = (
        out.find("answer me").unwrap(),
        out.find("3 open issues").unwrap(),
    );
    assert!(mine < count, "{out}");
}

#[test]
fn session_start_lists_mine_then_hot_urgent() {
    let d = repo();
    std::fs::write(d.join("src/a.rs"), "// a\n").unwrap();
    let input = format!(r#"{{"cwd":{}}}"#, json(&d));
    // the test repo's writer is Hook Test (hook-test-…): mixed case routes to it
    let add = |text: &str, extra: &[&str]| {
        let mut args = vec!["add", "issue", text, "--files", "src/a.rs"];
        args.extend(extra);
        let (ok, _, err) = fael(&d, &args, "");
        assert!(ok, "{err}");
    };
    add("mine plain", &["--to", "hook-test"]);
    add("hot unowned", &["--urgent"]);
    add("theirs urgent", &["--to", "someone", "--urgent"]);
    add("theirs plain", &["--to", "someone"]);
    add("anyone plain", &[]);
    let (ok, out, _) = fael(&d, &["hook", "session-start", "--client", "claude"], &input);
    assert!(ok, "{out}");
    // mine (even plain) and hot urgent list in full; routed and plain rest count only
    assert!(out.contains("mine plain (to: hook-test)"), "{out}");
    assert!(out.contains("hot unowned (urgent 1)"), "{out}");
    assert!(!out.contains("theirs urgent"), "{out}");
    assert!(!out.contains("theirs plain"), "{out}");
    assert!(!out.contains("anyone plain"), "{out}");
    assert!(
        out.contains(
            "1 to you (0 urgent) · 1 urgent unassigned · 5 open issues — fael find --kind issue"
        ),
        "{out}"
    );
    // routing first, then urgency: mine above hot above the count line
    let (mine, hot, count) = (
        out.find("mine plain").unwrap(),
        out.find("hot unowned").unwrap(),
        out.find("5 open issues").unwrap(),
    );
    assert!(mine < hot && hot < count, "{out}");
}
