//! Paging in the real binary: `--limit/--offset` page after ranking, the cut
//! line prints the exact next call, and MCP `find` returns the same rows.
//! (Lives here, not cli.rs — that file is already over the 400-line limit.)

use std::path::{Path, PathBuf};
use std::process::Command;

fn state_env(c: &mut Command, dir: &Path) {
    let root = dir.ancestors().find(|p| p.join(".git").exists()).unwrap();
    c.env("FAEL_STATE_DIR", root.join("state"));
}

fn fael(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"));
    c.args(args).current_dir(dir);
    state_env(&mut c, dir);
    let o = c.output().unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (o.status.success(), s(&o.stdout), s(&o.stderr))
}

fn repo() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-paging-{}", fael_core::ulid()));
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

fn add_issue(d: &Path, text: &str) {
    let (ok, _, err) = fael(d, &["add", "issue", text, "--files", "src/a.rs"]);
    assert!(ok, "{err}");
}

#[test]
fn find_pages_and_names_the_next_call() {
    let d = repo();
    for t in ["paging alpha", "paging beta", "paging gamma"] {
        add_issue(&d, t);
    }
    // newest first: gamma, beta — plus the exact next call
    let (ok, out, err) = fael(&d, &["find", "--kind", "issue", "--limit", "2"]);
    assert!(ok, "{err}");
    assert!(out.contains("paging gamma") && out.contains("paging beta"), "{out}");
    assert!(!out.contains("paging alpha"), "{out}");
    assert!(
        out.ends_with("… +1 more — next: fael find --kind issue --limit 2 --offset 2\n"),
        "{out}"
    );
    // rerunning that line returns the rest, with no cut line left
    let (ok, out, err) = fael(&d, &["find", "--kind", "issue", "--limit", "2", "--offset", "2"]);
    assert!(ok, "{err}");
    assert!(out.contains("paging alpha"), "{out}");
    assert!(!out.contains("more — next:"), "{out}");
    // past the end: empty output, exit 0
    let (ok, out, err) = fael(&d, &["find", "--kind", "issue", "--offset", "9"]);
    assert!(ok, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("no rows match"), "{err}");
    // not a number: rejected, not silently ignored
    let (ok, _, err) = fael(&d, &["find", "--limit", "x"]);
    assert!(!ok && err.contains("--limit needs a number"), "{err}");
}

#[test]
fn kickoff_pages_too() {
    let d = repo();
    // kickoff drops rows whose files are all gone — the file must exist
    std::fs::write(d.join("src/a.rs"), "").unwrap();
    for t in ["paging alpha", "paging beta", "paging gamma"] {
        add_issue(&d, t);
    }
    let (ok, out, err) = fael(&d, &["kickoff", "--limit", "2"]);
    assert!(ok, "{err}");
    assert_eq!(out.lines().count(), 3, "{out}");
    assert!(out.ends_with("… +1 more — next: fael kickoff --limit 2 --offset 2\n"), "{out}");
}

#[test]
fn mcp_find_pages_like_the_cli() {
    use std::io::Write;
    let d = repo();
    for t in ["paging alpha", "paging beta", "paging gamma"] {
        add_issue(&d, t);
    }
    let msgs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"find","arguments":{"kind":"issue","limit":2}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"find","arguments":{"kind":"issue","limit":2,"offset":2}}}"#,
    ];
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"))
        .arg("mcp")
        .env("FAEL_STATE_DIR", d.join("state"))
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
    let p1 = r[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(p1.contains("paging gamma") && p1.contains("paging beta"), "{p1}");
    assert!(!p1.contains("paging alpha"), "{p1}");
    assert!(p1.ends_with("… +1 more — next: offset=2\n"), "{p1}");
    let p2 = r[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(p2.contains("paging alpha"), "{p2}");
    assert!(!p2.contains("more — next:"), "{p2}");
}
