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
    assert!(out.contains(".fael/log/test-user-"), "{out}");

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
