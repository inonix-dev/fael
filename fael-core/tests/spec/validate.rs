use crate::common::*;
use fael_core::*;
use std::path::PathBuf;

#[test]
fn empty_files_rejected() {
    let e = validate(&row("note", &[]), &Config::default()).unwrap_err();
    assert_eq!(
        e,
        "rejected: files is required — name the file(s) this is about"
    );
    assert!(validate(&row("note", &["  "]), &Config::default()).is_err());
}

#[test]
fn undeclared_kind_rejected_with_allowed_list() {
    let e = validate(&row("bug", &["a.rs"]), &Config::default()).unwrap_err();
    assert!(
        e.starts_with("rejected: kind must be one of decision|issue|note (+ config.kinds)"),
        "{e}"
    );
    let cfg = Config {
        kinds: vec!["bug".into()],
        ..Config::default()
    };
    assert!(validate(&row("bug", &["a.rs"]), &cfg).is_ok());
}

#[test]
fn key_rules() {
    for bad in [
        "Auth:Session",
        "auth::x",
        ":auth",
        "auth:",
        "a b",
        &"a".repeat(65),
    ] {
        assert!(valid_key(bad).is_err(), "{bad} should be rejected");
    }
    for good in [
        "auth",
        "auth:session:timeout",
        "v0.1_x-y:z",
        &"a".repeat(64),
    ] {
        assert!(valid_key(good).is_ok(), "{good} should pass");
    }
    let mut r = row("note", &["a.rs"]);
    r.key = Some("Auth:Session".into());
    assert!(validate(&r, &Config::default()).is_err());
}

#[test]
fn size_and_secret_rejected() {
    let mut r = row("note", &["a.rs"]);
    r.text = "ก".repeat(3500); // 10.5 KB of Thai
    assert!(
        validate(&r, &Config::default())
            .unwrap_err()
            .contains("limit is 10240")
    );
    r.text = "token ghp_abcdefghijklmnopqrstuvwxyz0123 leaked".into();
    assert!(
        validate(&r, &Config::default())
            .unwrap_err()
            .contains("secret")
    );
    r.text = "AKIA is the prefix AWS keys use".into(); // prose about a prefix is fine
    assert!(validate(&r, &Config::default()).is_ok());
}

#[test]
fn files_normalised_to_repo_relative() {
    let (root, cwd) = (PathBuf::from("/r/repo"), PathBuf::from("/r/repo/src"));
    let n = |f: &str| normalize_files(&[f.to_string()], &cwd, &root);
    assert_eq!(n("auth.rs").unwrap(), ["src/auth.rs"]); // relative = from cwd
    assert_eq!(n("./auth.rs").unwrap(), ["src/auth.rs"]);
    assert_eq!(n("../docs//a.md").unwrap(), ["docs/a.md"]);
    assert_eq!(n("/r/repo/src/x.rs").unwrap(), ["src/x.rs"]); // hooks send absolute paths
    assert_eq!(n("sub\\win.rs").unwrap(), ["src/sub/win.rs"]);
    assert_eq!(n(" doc:pricing ").unwrap(), ["doc:pricing"]);
    assert_eq!(n("issue:#12").unwrap(), ["issue:#12"]);
    assert_eq!(n("doc:pricing/2026").unwrap(), ["doc:pricing/2026"]); // ref is opaque
    assert!(n("doc:").unwrap_err().contains("no ref"));
    for bad in [
        "../../etc/passwd",
        "/etc/passwd",
        "/r/repo2/a.rs",
        "..",
        "/r/repo",
    ] {
        assert!(n(bad).unwrap_err().contains("outside the repo"), "{bad}");
    }
    let win = normalize_files(
        &["C:\\w\\repo\\a.rs".into()],
        &PathBuf::from("C:\\w\\repo"),
        &PathBuf::from("C:\\w\\repo"),
    );
    assert_eq!(win.unwrap(), ["a.rs"]);
}

#[test]
fn validate_rejects_non_canonical_files() {
    for bad in [
        "./a.rs",
        "../a.rs",
        "/abs/a.rs",
        "C:/a.rs",
        "a\\b.rs",
        "a//b.rs",
        "a/./b.rs",
        "a/",
        "doc:",
        "doc: ",
    ] {
        let e = validate(&row("note", &[bad]), &Config::default()).unwrap_err();
        assert!(e.contains("not repo-relative"), "{bad}: {e}");
    }
    for ok in [
        "a.rs",
        "src/a.rs",
        ".github/ci.yml",
        "doc:pricing",
        "issue:#12",
    ] {
        assert!(
            validate(&row("note", &[ok]), &Config::default()).is_ok(),
            "{ok}"
        );
    }
}
