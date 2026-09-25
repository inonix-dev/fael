//! Chunk 6: `doctor --fix` per SPEC §11 case, `compact`, `import` — against
//! real files in throwaway dirs (months are injected, never the wall clock).

use fael_core::*;
use std::fs;
use std::path::{Path, PathBuf};

const MONTH: &str = "2026-09";

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-maint-{}", ulid()));
    fs::create_dir_all(&d).unwrap();
    d
}

/// A repo root with the union line present (tests that want it missing delete it).
fn root() -> PathBuf {
    let r = tmp();
    fs::write(r.join(".gitattributes"), "*.jsonl merge=union\n").unwrap();
    r
}

fn fael_of(root: &Path) -> PathBuf {
    root.join(".fael")
}

fn row(id: &str, kind: &str, files: &[&str]) -> Row {
    Row {
        v: Some(1),
        id: id.into(),
        ts: "2026-07-01T00:00:00.000Z".into(),
        by: "tester-0000".into(),
        kind: kind.into(),
        text: format!("text of {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        ..Row::default()
    }
}

fn month_file(fael: &Path, writer: &str, month: &str, close: bool) -> PathBuf {
    let dir = fael.join("log").join(writer);
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{month}{}.jsonl", if close { ".close" } else { "" }))
}

fn write_lines(path: &Path, lines: &[String]) {
    let mut body = lines.join("\n");
    if !lines.is_empty() {
        body.push('\n');
    }
    fs::write(path, body).unwrap();
}

fn kinds(rep: &DoctorReport) -> Vec<(ProblemKind, Severity)> {
    rep.problems.iter().map(|p| (p.kind.clone(), p.severity)).collect()
}

// --- doctor: scan ---

#[test]
fn clean_log_passes() {
    let r = root();
    let fael = fael_of(&r);
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line()],
    );
    let rep = doctor_scan(&fael, &r, false, MONTH);
    assert!(rep.problems.is_empty(), "{rep:?}");
}

#[test]
fn no_log_is_info_not_error() {
    let r = root();
    let rep = doctor_scan(&fael_of(&r), &r, false, MONTH);
    assert_eq!(kinds(&rep), vec![(ProblemKind::NoLog, Severity::Info)]);
    assert_eq!(rep.errors().count(), 0);
}

#[test]
fn missing_union_and_ignored() {
    let r = tmp(); // no .gitattributes here
    let fael = fael_of(&r);
    // nothing adopted: only the NoLog note, even with gitignore + no union line
    let rep = doctor_scan(&fael, &r, true, MONTH);
    assert_eq!(kinds(&rep), vec![(ProblemKind::NoLog, Severity::Info)]);
    // adopted (any log file): union + ignored fire
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line()],
    );
    let rep = doctor_scan(&fael, &r, true, MONTH);
    let ks = kinds(&rep);
    assert!(ks.contains(&(ProblemKind::Union, Severity::Error)), "{ks:?}");
    assert!(ks.contains(&(ProblemKind::Ignored, Severity::Error)), "{ks:?}");
}

// --- doctor: --fix content repairs ---

fn scan_fix_rescan(fael: &Path, root: &Path) -> (DoctorReport, Vec<String>, DoctorReport) {
    let before = doctor_scan(fael, root, false, MONTH);
    let done = doctor_fix(fael, root, &before).unwrap();
    let after = doctor_scan(fael, root, false, MONTH);
    (before, done, after)
}

#[test]
fn broken_lines_move_to_quarantine_never_deleted() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    let (a, b) = (
        row("A0000000000000000000000001", "note", &["a.rs"]).to_line(),
        row("A0000000000000000000000002", "note", &["b.rs"]).to_line(),
    );
    write_lines(&p, &[a.clone(), "not json".into(), b.clone()]);
    let (before, done, after) = scan_fix_rescan(&fael, &r);
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::Broken && x.fixable), "{before:?}");
    assert!(done.iter().any(|d| d.contains("1 line(s) to quarantine")), "{done:?}");
    // the byte survived — in quarantine, not in the log
    let q: Vec<PathBuf> = fs::read_dir(fael.join("quarantine")).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(q.len(), 1);
    assert!(fs::read_to_string(&q[0]).unwrap().contains("not json"));
    assert!(!fs::read_to_string(&p).unwrap().contains("not json"));
    assert_eq!(read(&fael).rows.len(), 2);
    assert!(after.problems.iter().all(|x| x.kind != ProblemKind::Broken), "{after:?}");
}

#[test]
fn torn_tail_moves_to_quarantine() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    let good = row("A0000000000000000000000001", "note", &["a.rs"]).to_line();
    fs::write(&p, format!("{good}\n{{\"v\":1,\"id\":\"TOR")).unwrap();
    let (before, _, after) = scan_fix_rescan(&fael, &r);
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::Torn), "{before:?}");
    assert_eq!(read(&fael).rows.len(), 1);
    assert!(after.problems.iter().all(|x| x.kind != ProblemKind::Torn), "{after:?}");
    assert!(fs::read(&p).unwrap().ends_with(b"\n"));
}

#[test]
fn conflict_markers_stripped_both_sides_kept() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    let (a, b) = (
        row("A0000000000000000000000001", "note", &["a.rs"]).to_line(),
        row("A0000000000000000000000002", "note", &["b.rs"]).to_line(),
    );
    write_lines(&p, &[a.clone(), "<<<<<<< HEAD".into(), b.clone(), "=======".into(), ">>>>>>> dev".into()]);
    let (before, done, after) = scan_fix_rescan(&fael, &r);
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::Conflict), "{before:?}");
    assert!(done.iter().any(|d| d.contains("3 marker(s) stripped")), "{done:?}");
    let body = fs::read_to_string(&p).unwrap();
    assert!(!body.contains("<<<<<<<") && body.contains(&a) && body.contains(&b));
    assert_eq!(read(&fael).rows.len(), 2);
    assert!(after.problems.iter().all(|x| x.kind == ProblemKind::Conflict), "{after:?}");
}

#[test]
fn bom_and_crlf_normalised() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    let line = row("A0000000000000000000000001", "note", &["a.rs"]).to_line();
    let mut raw = vec![0xef, 0xbb, 0xbf];
    raw.extend_from_slice(format!("{line}\r\n").as_bytes());
    fs::write(&p, raw).unwrap();
    let (before, _, after) = scan_fix_rescan(&fael, &r);
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::Encoding), "{before:?}");
    let fixed = fs::read(&p).unwrap();
    assert!(!fixed.starts_with(b"\xef\xbb\xbf") && !fixed.contains(&b'\r'));
    assert_eq!(read(&fael).rows.len(), 1);
    assert!(after.problems.iter().all(|x| x.kind == ProblemKind::Encoding), "{after:?}");
}

#[test]
fn duplicates_are_report_only() {
    let r = root();
    let fael = fael_of(&r);
    let line = row("A0000000000000000000000001", "note", &["a.rs"]).to_line();
    write_lines(&month_file(&fael, "tester-0000", "2026-07", false), std::slice::from_ref(&line));
    write_lines(&month_file(&fael, "other-0000", "2026-07", false), &[line]);
    let (before, _, after) = scan_fix_rescan(&fael, &r);
    let dupe = before.problems.iter().find(|x| x.kind == ProblemKind::Duplicate).unwrap();
    assert!(!dupe.fixable);
    assert_eq!(read(&fael).rows.len(), 1); // read dedupes first-wins
    assert!(after.problems.iter().any(|x| x.kind == ProblemKind::Duplicate));
}

#[test]
fn rows_without_files_and_future_month_are_notes() {
    let r = root();
    let fael = fael_of(&r);
    let mut legacy = row("legacy-1a2b3c4d", "note", &[]);
    legacy.files = vec![];
    write_lines(&month_file(&fael, "tester-0000", "2026-07", false), &[legacy.to_line()]);
    write_lines(&month_file(&fael, "tester-0000", "2999-01", false), &[row("A0000000000000000000000009", "note", &["a.rs"]).to_line()]);
    let (before, done, _) = scan_fix_rescan(&fael, &r);
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::NoFiles && x.severity == Severity::Info), "{before:?}");
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::Future), "{before:?}");
    assert!(done.is_empty(), "nothing fixable here: {done:?}");
    assert_eq!(read(&fael).rows.len(), 2); // legacy row still read
}

#[test]
fn missing_union_line_is_added() {
    let r = tmp();
    let fael = fael_of(&r);
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line()],
    );
    let before = doctor_scan(&fael, &r, false, MONTH);
    assert!(before.problems.iter().any(|x| x.kind == ProblemKind::Union), "{before:?}");
    doctor_fix(&fael, &r, &before).unwrap();
    assert!(fs::read_to_string(r.join(".gitattributes")).unwrap().contains(".fael/log/**/*.jsonl merge=union"));
    let after = doctor_scan(&fael, &r, false, MONTH);
    assert!(after.problems.iter().all(|x| x.kind != ProblemKind::Union), "{after:?}");
}

#[test]
fn several_fael_dirs_are_reported() {
    let r = root();
    let fael = fael_of(&r);
    let sub = r.join("sub").join(".fael").join("log").join("w-0000");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("2026-07.jsonl"), "{}\n").unwrap();
    let rep = doctor_scan(&fael, &r, false, MONTH);
    let m = rep.problems.iter().find(|x| x.kind == ProblemKind::MultiFael).unwrap();
    assert!(m.detail.contains("sub/.fael"), "{}", m.detail);
}

#[test]
fn oversize_month_is_an_error() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    write_lines(&p, &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line()]);
    fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(MONTH_MAX).unwrap();
    let rep = doctor_scan(&fael, &r, false, MONTH);
    assert!(rep.problems.iter().any(|x| x.kind == ProblemKind::Oversize && !x.fixable), "{rep:?}");
}

// --- compact ---

fn compact_month(fael: &Path, writer: &str, month: &str, ids: &[&str], closes: &[(&str, &str)]) {
    let lines: Vec<String> = ids.iter().map(|id| row(id, "note", &["a.rs"]).to_line()).collect();
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
    compact_month(&fael, "tester-0000", "2026-08", &["A0000000000000000000000003"], &[]);
    write_lines(
        &month_file(&fael, "tester-0000", "2026-09", false),
        &[row("A0000000000000000000000004", "note", &["a.rs"]).to_line()],
    );
    let rep = compact(&fael, &r, &CompactOpts::default(), MONTH).unwrap();
    assert_eq!(rep.writers.len(), 1);
    let w = &rep.writers[0];
    assert_eq!((w.rows, w.folded, w.carried, w.pruned), (3, 1, 0, 0));
    assert_eq!(w.deleted.len(), 4); // 07 + 07.close + 08 + 08.close
    assert!(!month_file(&fael, "tester-0000", "2026-07", false).exists());
    assert!(month_file(&fael, "tester-0000", "2026-09", false).exists());
    // one compact file, rows sorted by id, close folded into the row
    let found: Vec<PathBuf> = fs::read_dir(fael.join("log").join("tester-0000"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(found.iter().filter(|p| p.to_string_lossy().contains("compact.")).count(), 1);
    let log = read(&fael);
    assert_eq!(log.rows.len(), 4); // 3 compacted + 1 current
    assert!(closed(&log).contains("A0000000000000000000000001"));
    let gone: Vec<String> = find(&log, &Filter::default()).iter().map(|x| x.id.clone()).collect();
    assert!(!gone.contains(&"A0000000000000000000000001".to_string())); // hidden by default
    let all = Filter { all: true, ..Filter::default() };
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
    let e = compact(&fael, &r, &CompactOpts::default(), MONTH).unwrap_err();
    assert!(e.contains("no past months"), "{e}");
}

#[test]
fn compact_before_and_writer_filter() {
    let r = root();
    let fael = fael_of(&r);
    compact_month(&fael, "a-0000", "2026-07", &["A0000000000000000000000001"], &[]);
    compact_month(&fael, "a-0000", "2026-08", &["A0000000000000000000000002"], &[]);
    compact_month(&fael, "b-0000", "2026-07", &["A0000000000000000000000003"], &[]);
    let rep = compact(&fael, &r, &CompactOpts { before: Some("2026-08".into()), ..CompactOpts::default() }, MONTH).unwrap();
    assert_eq!(rep.writers.len(), 2); // 07 of both writers, 08 untouched
    assert!(rep.writers.iter().all(|w| w.rows == 1));
    assert!(month_file(&fael, "a-0000", "2026-08", false).exists());
    let rep = compact(
        &fael,
        &r,
        &CompactOpts { writer: Some("a-0000".into()), before: Some("2099-01".into()), ..CompactOpts::default() },
        "2099-02",
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
    let rep = compact(&fael, &r, &CompactOpts { prune: true, ..CompactOpts::default() }, MONTH).unwrap();
    assert_eq!(rep.writers[0].pruned, 1); // only the closed gone.rs row
    let ids: Vec<String> = read(&fael).rows.iter().map(|x| x.id.clone()).collect();
    assert!(!ids.contains(&"A0000000000000000000000001".to_string()));
    for id in ["A0000000000000000000000002", "A0000000000000000000000003", "A0000000000000000000000004", "A0000000000000000000000005"] {
        assert!(ids.contains(&id.to_string()), "{ids:?}");
    }
}

#[test]
fn compact_refuses_dirty_sources() {
    let r = root();
    let fael = fael_of(&r);
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line(), "broken".into()],
    );
    let e = compact(&fael, &r, &CompactOpts::default(), MONTH).unwrap_err();
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
    let rep = compact(&fael, &r, &CompactOpts::default(), MONTH).unwrap();
    assert_eq!((rep.writers[0].folded, rep.writers[0].carried), (0, 1));
    let log = read(&fael);
    assert_eq!(log.closes.len(), 1); // the companion .close.jsonl is read as closes
    assert_eq!(log.closes[0].reference.as_deref(), Some("MISSING"));
}

// --- import ---

fn legacy(kind: &str, id: &str, files: &str) -> String {
    format!(
        r#"{{"ts":"2026-01-01T00:00:00Z","agent":"delamind","id":"{id}","kind":"{kind}","text":"legacy {id}","files":[{files}],"v":2}}"#
    )
}

#[test]
fn fapony_legacy_mapping() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("old-memory");
    fs::create_dir_all(&src).unwrap();
    let lines = [
        legacy("decision", "muft0001", r#""src/a.rs""#),
        legacy("bug", "muft0002", r#""src/b.rs""#),
        legacy("next", "muft0003", r#""src/c.rs""#), // a cut kind → note + legacy_kind
        legacy("close", "muft0004", r#""src/a.rs""#).replace(r#""kind":"close""#, r#""kind":"close","ref":"muft0001""#),
        legacy("close", "muft0005", r#""src/a.rs""#).replace(r#""kind":"close""#, r#""kind":"close","ref":"missing-id""#),
        r#"{"ts":"2026-01-02T00:00:00Z","agent":"delamind","kind":"note","text":"no id here","files":["src/d.rs"],"v":2}"#.to_string(),
        r#"{"ts":"2026-01-03T00:00:00Z","agent":"delamind","id":"muft0007","kind":"note","text":"no files here","v":2}"#.to_string(),
        r#"{"ts":"2026-01-04T00:00:00Z","agent":"delamind","id":"muft0008","kind":"note","text":"anchor stays","files":["doc:pricing"],"v":2}"#.to_string(),
    ];
    fs::write(src.join("log.delamind.jsonl"), lines.join("\n") + "\n").unwrap();
    let rep = import(&fael, &src, &[], &ImportOpts::default()).unwrap();
    assert_eq!((rep.adds, rep.folded, rep.carried, rep.skipped), (6, 1, 1, 0), "{rep:?}");
    assert_eq!(rep.paths.len(), 2); // .jsonl + .close.jsonl companion
    let log = read(&fael);
    let by_id = |id: &str| log.rows.iter().find(|x| x.id == id).unwrap().clone();
    assert_eq!(by_id("muft0001").by, "delamind");
    assert_eq!(by_id("muft0002").kind, "issue");
    let n = by_id("muft0003");
    assert_eq!(n.kind, "note");
    assert_eq!(n.extra.get("legacy_kind").and_then(|v| v.as_str()), Some("next"));
    assert!(closed(&log).contains("muft0001")); // the close folded in
    assert!(by_id("muft0001").extra.contains_key("closed"));
    assert_eq!(log.closes.len(), 1); // the unresolvable close rides the companion
    let noid = log.rows.iter().find(|x| x.id.starts_with("legacy-")).unwrap();
    assert_eq!(noid.text, "no id here");
    let nofiles = by_id("muft0007");
    assert!(nofiles.files.is_empty()); // kept as-is, read-valid
    assert_eq!(by_id("muft0008").files, vec!["doc:pricing"]);
    // importing twice is safe — dedupe by id
    let rep2 = import(&fael, &src, &[], &ImportOpts::default()).unwrap();
    assert_eq!(rep2.adds, 6);
    assert_eq!(read(&fael).rows.len(), log.rows.len());
}

#[test]
fn import_map_rewrites_prefixes_not_anchors() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("old");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("log.jsonl"),
        legacy("decision", "muft0001", r#""old/svc/a.rs","doc:pricing""#) + "\n",
    )
    .unwrap();
    let rep = import(
        &fael,
        &src,
        &[],
        &ImportOpts { maps: vec![("old/".into(), "new/".into())] },
    )
    .unwrap();
    assert_eq!(rep.adds, 1);
    let row = &read(&fael).rows[0];
    assert_eq!(row.files, vec!["new/svc/a.rs", "doc:pricing"]);
}

#[test]
fn import_native_fael_log() {
    let r = root();
    let fael = fael_of(&r);
    let other = tmp().join("other").join(".fael").join("log");
    fs::create_dir_all(other.join("w-0000")).unwrap();
    let line = row("A0000000000000000000000001", "decision", &["x.rs"]).to_line();
    fs::write(other.join("w-0000").join("2026-01.jsonl"), line.clone() + "\n").unwrap();
    let rep = import(&fael, &other, &[], &ImportOpts::default()).unwrap();
    assert_eq!((rep.adds, rep.skipped), (1, 0));
    assert_eq!(read(&fael).rows.len(), 1);
}

#[test]
fn import_350_legacy_rows_drops_nothing() {
    let r = root();
    let fael = fael_of(&r);
    let src = r.join("big");
    fs::create_dir_all(&src).unwrap();
    let mut lines = vec![];
    for i in 0..350 {
        let kind = if i % 10 == 0 { "bug" } else if i % 15 == 0 { "next" } else { "note" };
        if i % 70 == 0 {
            lines.push(format!(
                r#"{{"ts":"2026-01-01T00:00:00Z","agent":"w","kind":"{kind}","text":"row {i}","files":["f{i}.rs"],"v":2}}"#
            )); // no id → legacy-…
        } else {
            lines.push(legacy(kind, &format!("id{i:04}"), &format!(r#""f{i}.rs""#)));
        }
    }
    for (c, t) in [("c0001", "id0001"), ("c0002", "id0002"), ("c0003", "nope")] {
        lines.push(format!(
            r#"{{"ts":"2026-01-02T00:00:00Z","agent":"w","id":"{c}","kind":"close","ref":"{t}","text":"done","files":[],"v":2}}"#
        ));
    }
    fs::write(src.join("log.jsonl"), lines.join("\n") + "\n").unwrap();
    let rep = import(&fael, &src, &[], &ImportOpts::default()).unwrap();
    assert_eq!(rep.skipped, 0, "{:?}", rep.warnings);
    assert_eq!(rep.adds + rep.folded + rep.carried, lines.len(), "{rep:?}");
    assert_eq!(rep.folded, 2);
    assert_eq!(rep.carried, 1);
}
