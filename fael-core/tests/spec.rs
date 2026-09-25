//! SPEC-fael-v0 §10 fail examples + §11 read-side healing, against the real files.

use fael_core::*;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-test-{}", ulid()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn row(kind: &str, files: &[&str]) -> Row {
    Row::new(
        "tester-0000",
        kind,
        "some standalone text",
        files.iter().map(|s| s.to_string()).collect(),
    )
}

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
fn close_needs_ref_and_text() {
    let c = Row::close("tester-0000", "", "done");
    assert!(close(&tmp(), &c, &Config::default()).is_err());
    let c = Row::close("tester-0000", "01ABC", "fixed in abc123");
    let dir = tmp();
    let p = close(&dir, &c, &Config::default()).unwrap();
    assert!(p.to_string_lossy().ends_with(".close.jsonl"));
    assert_eq!(read(&dir).closes, vec![c]);
}

#[test]
fn append_path_is_writer_and_month() {
    let dir = tmp();
    let r = row("note", &["a.rs"]);
    let p = add(&dir, &r, &Config::default()).unwrap();
    assert_eq!(
        p,
        dir.join("log/tester-0000")
            .join(format!("{}.jsonl", &r.ts[..7]))
    );
    for by in ["", "_import", "../x", "a/b", ".hidden"] {
        let mut bad = r.clone();
        bad.by = by.into();
        assert!(
            append(&dir, &bad, false).is_err(),
            "writer {by:?} should be refused"
        );
    }
}

// On APFS/ext4 one O_APPEND write_all is already atomic, so this passes even without the lock —
// it pins the §10 outcome (whole lines, none lost); the lock itself guards seal-then-write and rewrites.
#[test]
fn concurrent_adds_never_interleave() {
    let dir = tmp();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                for i in 0..200 {
                    let mut r = row("note", &["a.rs"]);
                    r.text = format!("thread {t} row {i} {}", "x".repeat(2000));
                    add(&dir, &r, &Config::default()).unwrap();
                }
            })
        })
        .collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    let log = read(&dir);
    assert!(
        log.warnings.is_empty(),
        "{:?}",
        &log.warnings[..3.min(log.warnings.len())]
    );
    assert_eq!(log.rows.len(), 1600);
}

#[test]
fn torn_write_is_skipped_then_sealed() {
    let dir = tmp();
    let r1 = row("note", &["a.rs"]);
    let p = add(&dir, &r1, &Config::default()).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .unwrap()
        .write_all(br#"{"v":1,"id":"TORN","te"#)
        .unwrap();

    let log = read(&dir);
    assert_eq!(log.rows, vec![r1.clone()]);
    assert!(log.warnings[0].contains("torn"), "{:?}", log.warnings);

    let r2 = row("issue", &["b.rs"]);
    add(&dir, &r2, &Config::default()).unwrap();
    let log = read(&dir);
    assert_eq!(log.rows, vec![r1, r2]);
    assert_eq!(log.warnings.len(), 1);
    assert!(
        log.warnings[0].contains(":2: broken line"),
        "{:?}",
        log.warnings
    );
}

#[test]
fn union_duplicate_deduped_by_id() {
    let dir = tmp();
    let r = row("decision", &["a.rs"]);
    let p = add(&dir, &r, &Config::default()).unwrap();
    let line = fs::read_to_string(&p).unwrap();
    fs::write(&p, line.repeat(3)).unwrap();
    // the same row can also arrive through a second file (another writer folder / an import)
    fs::create_dir_all(dir.join("log/_import")).unwrap();
    fs::write(dir.join("log/_import/X.jsonl"), &line).unwrap();
    assert_eq!(read(&dir).rows, vec![r]);
}

#[test]
fn month_file_over_50mib_refuses_append() {
    let dir = tmp();
    let r = row("note", &["a.rs"]);
    let p = add(&dir, &r, &Config::default()).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(MONTH_MAX)
        .unwrap(); // sparse
    let e = add(&dir, &row("note", &["a.rs"]), &Config::default()).unwrap_err();
    assert!(e.contains("fael compact"), "{e}");
}

#[test]
fn read_heals_bom_crlf_conflicts_junk() {
    let a = r#"{"v":1,"id":"A","ts":"2026-09-01T00:00:00Z","by":"w","kind":"note","text":"a","files":["x"]}"#;
    let b = r#"{"v":1,"id":"B","ts":"2026-09-01T00:00:00Z","by":"w","kind":"note","text":"b","files":["x"]}"#;
    let raw = format!("\u{feff}{a}\r\n<<<<<<< HEAD\r\n{b}\n=======\nnot json\n>>>>>>> dev\n\n");
    let (mut rows, mut warn) = (vec![], vec![]);
    fael_core::parse(raw.as_bytes(), "f.jsonl", &mut rows, &mut warn);
    assert_eq!(
        rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(warn.len(), 1);
    assert!(warn[0].starts_with("f.jsonl:5: broken line"), "{warn:?}");

    let (mut rows, mut warn) = (vec![], vec![]);
    fael_core::parse(b"\xff\xfe{\"id\":1}\n", "g.jsonl", &mut rows, &mut warn); // bad utf-8 + wrong type
    assert!(rows.is_empty() && warn.len() == 1);
}

#[test]
fn unknown_fields_and_legacy_rows_round_trip() {
    let line = r#"{"id":"legacy-1a2b3c4d","ts":"2026-01-01T00:00:00Z","by":"old","kind":"claim","text":"t","sha":"abc","closed":{"id":"C","text":"x"},"future":[1,2]}"#;
    let r: Row = serde_json::from_str(line).unwrap();
    assert_eq!(r.v, None);
    assert!(r.files.is_empty());
    let back: serde_json::Value = serde_json::from_str(&r.to_line()).unwrap();
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(line).unwrap()
    );
}

#[test]
fn ids_and_clock() {
    let a = ulid_at(1_000);
    let b = ulid_at(2_000);
    assert_eq!(a.len(), 26);
    assert!(a < b, "ULIDs sort by time");
    assert!(
        a.bytes()
            .all(|c| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&c))
    );
    assert_ne!(ulid_at(5), ulid_at(5), "random part differs within one ms");
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(1_790_330_400_000), "2026-09-25T10:00:00.000Z");
    assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(rfc3339(1_790_330_400_123), "2026-09-25T10:00:00.123Z");
    assert_eq!(ts_ms("2026-09-25T10:00:00.123Z"), Some(1_790_330_400_123));
    assert_eq!(ts_ms("2026-09-25T10:00:00Z"), Some(1_790_330_400_000));
}

#[test]
fn writer_id_hides_email() {
    let a = writer_id("Dela Mind", Some("Kire@Example.com"), "host");
    let b = writer_id("dela.mind", Some("kire@example.com"), "other");
    assert_eq!(a[..a.len() - 5], *"dela-mind");
    assert_eq!(
        a[a.len() - 4..],
        b[b.len() - 4..],
        "same email, any case → same hash"
    );
    assert_ne!(a, writer_id("Dela Mind", Some("x@y.z"), "host"));
    assert!(!a.contains("example"));
    assert!(
        writer_id("__", None, "h").starts_with("anon-"),
        "never starts with _"
    );
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
