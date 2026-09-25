use crate::common::*;
use fael_core::*;
use std::fs;
use std::io::Write;

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
