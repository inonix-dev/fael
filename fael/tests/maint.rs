//! Chunk 6 through the real binary: doctor exit codes + --fix, compact,
//! import (incl. the fapony legacy no-drop rule) — each in a throwaway repo.

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
    let d = std::env::temp_dir().join(format!("fael-maint-cli-{}", fael_core::ulid()));
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
fn doctor_fails_without_union_then_fix_repairs() {
    let d = repo();
    // no log yet: info only, exit 0
    let (ok, out, _) = fael(&d, &["doctor"]);
    assert!(ok && out.contains("1 problem(s)"), "{out}");
    // adopt fael without the union line: error, exit 1
    let (ok, _, _) = fael(&d, &["add", "note", "first row", "--files", "src/a.rs"]);
    assert!(ok);
    let (ok, out, _) = fael(&d, &["doctor"]);
    assert!(!ok && out.contains("error"), "{out}");
    let (ok, out, _) = fael(&d, &["doctor", "--fix"]);
    assert!(ok, "{out}");
    assert!(out.contains("merge=union"), "{out}");
    assert!(d.join(".gitattributes").exists());
    let (ok, out, _) = fael(&d, &["doctor"]);
    assert!(ok && out.contains("clean"), "{out}");
}

#[test]
fn doctor_quarantines_a_broken_line() {
    let d = repo();
    let (ok, _, err) = fael(&d, &["add", "decision", "keep me", "--files", "src/a.rs"]);
    assert!(ok, "{err}");
    let wdir = std::fs::read_dir(d.join(".fael/log"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let log: Vec<PathBuf> = std::fs::read_dir(&wdir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(log.len(), 1);
    let mut s = std::fs::read_to_string(&log[0]).unwrap();
    s.push_str("not json\n");
    std::fs::write(&log[0], s).unwrap();
    let (ok, _, _) = fael(&d, &["doctor"]);
    assert!(!ok);
    let (ok, out, _) = fael(&d, &["doctor", "--fix", "--json"]);
    assert!(ok, "{out}");
    let q: Vec<_> = std::fs::read_dir(d.join(".fael/quarantine"))
        .unwrap()
        .collect();
    assert_eq!(q.len(), 1);
    let (_, out, _) = fael(&d, &["find", "--all"]);
    assert!(out.contains("keep me"), "{out}");
}

#[test]
fn compact_round_trip_through_cli() {
    let d = repo();
    let (ok, _, err) = fael(&d, &["add", "note", "current row", "--files", "src/a.rs"]);
    assert!(ok, "{err}");
    // a past month with a close, written by hand (the CLI only writes this month)
    let by = std::fs::read_dir(d.join(".fael/log"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    let dir = d.join(".fael/log").join(by);
    let row = |id: &str, text: &str| {
        format!(
            r#"{{"v":1,"id":"{id}","ts":"2000-01-01T00:00:00.000Z","by":"test-user-","kind":"decision","text":"{text}","files":["old.rs"]}}"#
        )
    };
    std::fs::write(
        dir.join("2000-01.jsonl"),
        format!(
            "{}\n{}\n",
            row("A0000000000000000000000001", "old one"),
            row("A0000000000000000000000002", "old two")
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("2000-01.close.jsonl"),
        "{\"v\":1,\"id\":\"C0000000000000000000000001\",\"ts\":\"2000-01-02T00:00:00.000Z\",\"by\":\"test-user-\",\"ref\":\"A0000000000000000000000001\",\"text\":\"done\"}\n",
    )
    .unwrap();
    let (ok, out, err) = fael(&d, &["compact"]);
    assert!(ok, "{err}");
    assert!(out.contains("1 close(s) folded"), "{out}");
    assert!(!dir.join("2000-01.jsonl").exists());
    let (_, out, _) = fael(&d, &["find", "--all"]);
    assert!(
        out.contains("old one") && out.contains("old two") && out.contains("current row"),
        "{out}"
    );
    let (_, out, _) = fael(&d, &["find"]);
    assert!(
        !out.contains("old one") && out.contains("current row"),
        "{out}"
    ); // folded close hides by default
}

#[test]
fn import_fapony_legacy_drops_nothing() {
    let d = repo();
    let mem = d.join(".fapony").join(".memory");
    std::fs::create_dir_all(&mem).unwrap();
    let lines = [
        r#"{"ts":"2026-01-01T00:00:00Z","agent":"delamind","id":"muft0001","kind":"decision","text":"picked x","files":["src/a.rs"],"v":2}"#,
        r#"{"ts":"2026-01-02T00:00:00Z","agent":"delamind","id":"muft0002","kind":"bug","text":"login loops","files":["src/b.rs"],"v":2}"#,
        r#"{"ts":"2026-01-03T00:00:00Z","agent":"delamind","kind":"note","text":"no id","files":["src/c.rs"],"v":2}"#,
        r#"{"ts":"2026-01-04T00:00:00Z","agent":"delamind","id":"muft0004","kind":"close","ref":"muft0001","text":"done","files":[],"v":2}"#,
    ];
    std::fs::write(mem.join("log.delamind.jsonl"), lines.join("\n") + "\n").unwrap();
    let (ok, out, err) = fael(&d, &["import", ".fapony/.memory"]);
    assert!(ok, "{err}");
    assert!(out.contains("imported 3 row(s)"), "{out}"); // 4 lines = 3 adds + 1 folded close
    assert!(out.contains("1 close(s) folded"), "{out}");
    let (_, out, _) = fael(&d, &["find", "--files", "src"]);
    assert!(out.contains("issue login loops → src/b.rs"), "{out}");
    assert!(!out.contains("picked x")); // closed by the folded close
    let (_, out, _) = fael(&d, &["import", ".fapony/.memory"]);
    assert!(out.contains("imported 3 row(s)"), "{out}"); // twice is safe
    let (_, out, _) = fael(&d, &["keys"]);
    assert!(out.is_empty() || !out.contains("×0"), "{out}");
}

#[test]
fn import_map_moves_a_subtree() {
    let d = repo();
    std::fs::create_dir_all(d.join("old-mem")).unwrap();
    std::fs::write(
        d.join("old-mem/log.jsonl"),
        "{\"ts\":\"2026-01-01T00:00:00Z\",\"agent\":\"w\",\"id\":\"m1\",\"kind\":\"note\",\"text\":\"t\",\"files\":[\"svc/a.rs\"],\"v\":2}\n",
    )
    .unwrap();
    let (ok, _, err) = fael(&d, &["import", "old-mem", "--map", "svc/=services/svc/"]);
    assert!(ok, "{err}");
    let (_, out, _) = fael(&d, &["find", "--files", "services/svc/a.rs"]);
    assert!(out.contains("→ services/svc/a.rs"), "{out}");
    let (ok, _, err) = fael(&d, &["import", "old-mem", "--map", "no-equals-here"]);
    assert!(!ok && err.contains("--map"), "{err}");
}
