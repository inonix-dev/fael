//! The real binary in a throwaway git repo: add → find → close → keys → kickoff.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fael(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_fael"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (o.status.success(), s(&o.stdout), s(&o.stderr))
}

fn repo() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-cli-{}", fael_core::ulid()));
    std::fs::create_dir_all(d.join("src")).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test User"],
        &["config", "user.email", "t@example.com"],
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

#[test]
fn add_find_close_round_trip() {
    let d = repo();
    // relative to cwd: `a.rs` from src/ is stored as src/a.rs
    let (ok, out, err) = fael(
        &d.join("src"),
        &[
            "add",
            "issue",
            "token expiry breaks login",
            "--files",
            "a.rs",
            "--key",
            "auth:session",
        ],
    );
    assert!(ok, "{err}");
    let id = out.split_whitespace().next().unwrap().to_string();
    // Windows prints `\` separators
    assert!(
        out.replace('\\', "/").contains(".fael/log/test-user-"),
        "{out}"
    );

    let (ok, _, err) = fael(&d, &["add", "note", "x", "--files", "../elsewhere.rs"]);
    assert!(!ok && err.contains("outside the repo"), "{err}");
    let (ok, _, err) = fael(&d, &["add", "note", "x"]);
    assert!(!ok && err.contains("files is required"), "{err}");

    let (_, out, _) = fael(&d, &["find", "--files", "src"]);
    assert!(
        out.starts_with("- [")
            && out.contains("issue #auth:session token expiry breaks login → src/a.rs"),
        "{out}"
    );
    let (_, out, _) = fael(&d, &["find", "--json"]);
    assert!(out.contains("\"files\":[\"src/a.rs\"]"), "{out}");

    let (ok, _, err) = fael(&d, &["close", &id[..12], "fixed"]);
    assert!(ok, "{err}");
    let (_, out, err) = fael(&d, &["find", "--files", "src/a.rs"]);
    assert!(out.is_empty() && err.contains("no rows match"), "{out}");
    let (_, out, _) = fael(&d, &["find", "--all", "--files", "src/a.rs"]);
    assert!(out.contains("issue (closed)"), "{out}");

    let (_, out, _) = fael(&d, &["keys"]);
    assert_eq!(
        out,
        format!(
            "- auth:session ×1 (last {})\n",
            &fael_core::rfc3339(fael_core::now_ms())[..10]
        )
    );
    let (ok, _, _) = fael(&d, &["kickoff", "src/a.rs"]);
    assert!(ok);
}

#[test]
fn config_kinds_and_bad_config() {
    let d = repo();
    std::fs::create_dir_all(d.join(".fael")).unwrap();
    std::fs::write(d.join(".fael/config.toml"), "kinds = [\"risk\"]\n").unwrap();
    let (ok, _, err) = fael(
        &d,
        &["add", "risk", "vendor may fold", "--files", "doc:vendors"],
    );
    assert!(ok, "{err}");
    std::fs::write(d.join(".fael/config.toml"), "kinds = [\n").unwrap();
    let (ok, _, err) = fael(&d, &["find"]);
    assert!(!ok && err.contains("config.toml"), "{err}");
}

#[test]
fn mcp_round_trip() {
    use std::io::Write;
    let d = repo();
    let msgs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"add","arguments":{"kind":"issue","text":"login loops","files":[]}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"add","arguments":{"kind":"issue","text":"login loops","files":["src/a.rs"]}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"find","arguments":{"files":["src"]}}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"nope"}"#,
        "not json",
    ];
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"))
        .arg("mcp")
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
    assert_eq!(r.len(), 7, "notification must get no reply: {out}");
    assert_eq!(r[0]["result"]["serverInfo"]["name"], "fael");
    let names: Vec<_> = r[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["find", "add", "close"]);
    assert_eq!(r[2]["result"]["isError"], true);
    assert!(
        r[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("files is required"),
        "{out}"
    );
    assert_eq!(r[3]["result"]["isError"], false, "{out}");
    let text = r[4]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("issue login loops → src/a.rs"), "{text}");
    assert_eq!(r[5]["error"]["code"], -32601);
    assert_eq!(r[6]["error"]["code"], -32700);
}

#[test]
fn worktree_root_is_where_dot_git_file_sits() {
    // a linked worktree has `.git` as a file — root must be the worktree, not the main repo
    let d = repo();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&d)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = d.with_extension("wt");
    git(&["worktree", "add", "-q", wt.to_str().unwrap()]);
    std::fs::create_dir_all(wt.join("src")).unwrap();
    let (ok, _, err) = fael(
        &wt.join("src"),
        &["add", "note", "wt row", "--files", "a.rs"],
    );
    assert!(ok, "{err}");
    assert!(wt.join(".fael/log").is_dir() && !d.join(".fael").exists());
    let (_, out, _) = fael(&wt, &["find", "--files", "src/a.rs"]);
    assert!(out.contains("wt row"), "{out}");
}
