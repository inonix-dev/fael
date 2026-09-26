//! `fael bump` + urgent over MCP: the binary in a throwaway git repo.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fael(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fael"));
    c.args(args)
        .current_dir(dir)
        .env("FAEL_STATE_DIR", dir.join("state"));
    let o = c.output().unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (o.status.success(), s(&o.stdout), s(&o.stderr))
}

fn repo() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-bump-{}", fael_core::ulid()));
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
fn mcp_bump_moves_urgent() {
    use std::io::Write;
    let d = repo();
    let (ok, out, err) = fael(
        &d,
        &["add", "issue", "cli hot", "--files", "src/a.rs", "--urgent"],
    );
    assert!(ok, "{err}");
    let id = out.split_whitespace().next().unwrap().to_string();
    let req = |n: u32, name: &str, args: serde_json::Value| {
        serde_json::json!({"jsonrpc":"2.0","id":n,"method":"tools/call","params":{"name":name,"arguments":args}})
            .to_string()
    };
    let msgs = [
        req(
            1,
            "add",
            serde_json::json!({"kind":"issue","text":"mcp second","files":["src/a.rs"],"urgent_before":id}),
        ),
        req(
            2,
            "bump",
            serde_json::json!({"id":id,"not_urgent":true,"to":"Ploy"}),
        ),
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
    assert_eq!(r[0]["result"]["isError"], false, "{out}");
    assert_eq!(r[1]["result"]["isError"], false, "{out}");
    // MCP filed above the CLI row (half the top) and bumped it out of the queue
    let (_, out, _) = fael(&d, &["find", "--json", "--all"]);
    let rows: Vec<serde_json::Value> = out
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let second = rows
        .iter()
        .find(|v| v["text"].as_str().unwrap().contains("mcp second"))
        .unwrap();
    assert_eq!(second["urgent"].as_f64(), Some(0.5), "{second}");
    let hot = rows
        .iter()
        .filter(|v| v["text"].as_str().unwrap().contains("cli hot"))
        .max_by_key(|v| v["id"].as_str().unwrap_or("").to_string())
        .unwrap();
    assert!(hot.get("urgent").is_none(), "{hot}");
    assert_eq!(hot["to"].as_str().unwrap(), "ploy");
}
