use crate::common::*;
use fael_core::*;
use std::fs;
use std::path::Path;

fn compact_month(fael: &Path, writer: &str, month: &str, ids: &[&str], closes: &[(&str, &str)]) {
    let lines: Vec<String> = ids
        .iter()
        .map(|id| row(id, "note", &["a.rs"]).to_line())
        .collect();
    write_lines(&month_file(fael, writer, month, false), &lines);
    let clines: Vec<String> = closes
        .iter()
        .map(|(cid, target)| {
            let mut c = Row::close("tester-0000", target, "done");
            c.id = cid.to_string();
            c.to_line()
        })
        .collect();
    write_lines(&month_file(fael, writer, month, false), &lines);
    write_lines(&month_file(fael, writer, month, true), &clines);
}

#[test]
fn compact_folds_sorts_and_deletes_past_months() {
    let r = root();
    let fael = fael_of(&r);
    compact_month(
        &fael,
        "tester-0000",
        "2026-07",
        &["A0000000000000000000000002", "A0000000000000000000000001"],
        &[("C0000000000000000000000001", "A0000000000000000000000001")],
    );
    compact_month(
        &fael,
        "tester-0000",
        "2026-08",
        &["A0000000000000000000000003"],
        &[],
    );
    write_lines(
        &month_file(&fael, "tester-0000", "2026-09", false),
        &[row("A0000000000000000000000004", "note", &["a.rs"]).to_line()],
    );
    let rep = compact(
        &fael,
        &r,
        &CompactOpts::default(),
        MONTH,
        &Aliases::default(),
    )
    .unwrap();
    assert_eq!(rep.writers.len(), 1);
    let w = &rep.writers[0];
    assert_eq!((w.rows, w.folded, w.carried, w.pruned), (3, 1, 0, 0));
    assert_eq!(w.deleted.len(), 4); // 07 + 07.close + 08 + 08.close
    assert!(!month_file(&fael, "tester-0000", "2026-07", false).exists());
    assert!(month_file(&fael, "tester-0000", "2026-09", false).exists());
    // one compact file, rows sorted by id, close folded into the row
    let found: Vec<std::path::PathBuf> = fs::read_dir(fael.join("log").join("tester-0000"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(
        found
            .iter()
            .filter(|p| p.to_string_lossy().contains("compact."))
            .count(),
        1
    );
    let log = read(&fael);
    assert_eq!(log.rows.len(), 4); // 3 compacted + 1 current
    assert!(closed(&log).contains("A0000000000000000000000001"));
    let gone: Vec<String> = find(&log, &Filter::default())
        .iter()
        .map(|x| x.id.clone())
        .collect();
    assert!(!gone.contains(&"A0000000000000000000000001".to_string())); // hidden by default
    let all = Filter {
        all: true,
        ..Filter::default()
    };
    let shown = find(&log, &all);
    assert!(shown.iter().any(|x| x.id == "A0000000000000000000000001"));
}

#[test]
fn compact_nothing_eligible_is_an_error() {
    let r = root();
    let fael = fael_of(&r);
    write_lines(
        &month_file(&fael, "tester-0000", "2026-09", false),
        &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line()],
    );
    let e = compact(
        &fael,
        &r,
        &CompactOpts::default(),
        MONTH,
        &Aliases::default(),
    )
    .unwrap_err();
    assert!(e.contains("no past months"), "{e}");
}

#[test]
fn compact_before_and_writer_filter() {
    let r = root();
    let fael = fael_of(&r);
    compact_month(
        &fael,
        "a-0000",
        "2026-07",
        &["A0000000000000000000000001"],
        &[],
    );
    compact_month(
        &fael,
        "a-0000",
        "2026-08",
        &["A0000000000000000000000002"],
        &[],
    );
    compact_month(
        &fael,
        "b-0000",
        "2026-07",
        &["A0000000000000000000000003"],
        &[],
    );
    let rep = compact(
        &fael,
        &r,
        &CompactOpts {
            before: Some("2026-08".into()),
            ..CompactOpts::default()
        },
        MONTH,
        &Aliases::default(),
    )
    .unwrap();
    assert_eq!(rep.writers.len(), 2); // 07 of both writers, 08 untouched
    assert!(rep.writers.iter().all(|w| w.rows == 1));
    assert!(month_file(&fael, "a-0000", "2026-08", false).exists());
    let rep = compact(
        &fael,
        &r,
        &CompactOpts {
            writer: Some("a-0000".into()),
            before: Some("2099-01".into()),
            ..CompactOpts::default()
        },
        "2099-02",
        &Aliases::default(),
    )
    .unwrap();
    assert_eq!(rep.writers.len(), 1);
    assert_eq!(rep.writers[0].writer, "a-0000"); // 08 compacted now 07 is gone
    assert_eq!(rep.writers[0].rows, 1);
}

#[test]
fn compact_prune_drops_only_closed_rows_whose_files_are_all_gone() {
    let r = root();
    let fael = fael_of(&r);
    fs::write(r.join("here.rs"), "x").unwrap();
    let dir = fael.join("log").join("tester-0000");
    fs::create_dir_all(&dir).unwrap();
    let gone = row("A0000000000000000000000001", "note", &["gone.rs"]);
    let kept = row("A0000000000000000000000002", "note", &["here.rs"]);
    let open = row("A0000000000000000000000003", "note", &["gone.rs"]);
    let anch = row("A0000000000000000000000004", "note", &["doc:pricing"]);
    let mut nofiles = row("A0000000000000000000000005", "note", &[]);
    nofiles.files = vec![];
    let mut lines = vec![];
    for x in [&gone, &kept, &open, &anch, &nofiles] {
        lines.push(x.to_line());
    }
    write_lines(&month_file(&fael, "tester-0000", "2026-07", false), &lines);
    let mut c1 = Row::close("tester-0000", "A0000000000000000000000001", "done");
    c1.id = "C0000000000000000000000001".into();
    let mut c2 = Row::close("tester-0000", "A0000000000000000000000002", "done");
    c2.id = "C0000000000000000000000002".into();
    let mut c4 = Row::close("tester-0000", "A0000000000000000000000004", "done");
    c4.id = "C0000000000000000000000004".into();
    let mut c5 = Row::close("tester-0000", "A0000000000000000000000005", "done");
    c5.id = "C0000000000000000000000005".into();
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", true),
        &[c1.to_line(), c2.to_line(), c4.to_line(), c5.to_line()],
    );
    let rep = compact(
        &fael,
        &r,
        &CompactOpts {
            prune: true,
            ..CompactOpts::default()
        },
        MONTH,
        &Aliases::default(),
    )
    .unwrap();
    assert_eq!(rep.writers[0].pruned, 1); // only the closed gone.rs row
    let ids: Vec<String> = read(&fael).rows.iter().map(|x| x.id.clone()).collect();
    assert!(!ids.contains(&"A0000000000000000000000001".to_string()));
    for id in [
        "A0000000000000000000000002",
        "A0000000000000000000000003",
        "A0000000000000000000000004",
        "A0000000000000000000000005",
    ] {
        assert!(ids.contains(&id.to_string()), "{ids:?}");
    }
}

#[test]
fn compact_prune_keeps_closed_rows_whose_files_were_renamed() {
    let r = root();
    let fael = fael_of(&r);
    fs::write(r.join("new.rs"), "x").unwrap();
    // old.rs never exists on disk; the alias says it lives at new.rs now
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[row("A0000000000000000000000001", "note", &["old.rs"]).to_line()],
    );
    let mut c1 = Row::close("tester-0000", "A0000000000000000000000001", "done");
    c1.id = "C0000000000000000000000001".into();
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", true),
        &[c1.to_line()],
    );
    let al = Aliases::from_pairs(vec![("old.rs".to_string(), "new.rs".to_string())]);
    let rep = compact(
        &fael,
        &r,
        &CompactOpts {
            prune: true,
            ..CompactOpts::default()
        },
        MONTH,
        &al,
    )
    .unwrap();
    assert_eq!(rep.writers[0].pruned, 0); // merely renamed, never pruned
    let ids: Vec<String> = read(&fael).rows.iter().map(|x| x.id.clone()).collect();
    assert!(
        ids.contains(&"A0000000000000000000000001".to_string()),
        "{ids:?}"
    );
}

#[test]
fn compact_refuses_dirty_sources() {
    let r = root();
    let fael = fael_of(&r);
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[
            row("A0000000000000000000000001", "note", &["a.rs"]).to_line(),
            "broken".into(),
        ],
    );
    let e = compact(
        &fael,
        &r,
        &CompactOpts::default(),
        MONTH,
        &Aliases::default(),
    )
    .unwrap_err();
    assert!(e.contains("doctor --fix"), "{e}");
    assert!(month_file(&fael, "tester-0000", "2026-07", false).exists()); // nothing deleted
}

#[test]
fn compact_carries_closes_with_no_target() {
    let r = root();
    let fael = fael_of(&r);
    compact_month(
        &fael,
        "tester-0000",
        "2026-07",
        &["A0000000000000000000000001"],
        &[("C0000000000000000000000001", "MISSING")],
    );
    let rep = compact(
        &fael,
        &r,
        &CompactOpts::default(),
        MONTH,
        &Aliases::default(),
    )
    .unwrap();
    assert_eq!((rep.writers[0].folded, rep.writers[0].carried), (0, 1));
    let log = read(&fael);
    assert_eq!(log.closes.len(), 1); // the companion .close.jsonl is read as closes
    assert_eq!(log.closes[0].reference.as_deref(), Some("MISSING"));
}
