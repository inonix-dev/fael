use crate::common::*;
use fael_core::*;
use std::fs;
use std::path::{Path, PathBuf};

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
    assert!(
        before
            .problems
            .iter()
            .any(|x| x.kind == ProblemKind::Broken && x.fixable),
        "{before:?}"
    );
    assert!(
        done.iter().any(|d| d.contains("1 line(s) to quarantine")),
        "{done:?}"
    );
    // the byte survived — in quarantine, not in the log
    let q: Vec<PathBuf> = fs::read_dir(fael.join("quarantine"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(q.len(), 1);
    assert!(fs::read_to_string(&q[0]).unwrap().contains("not json"));
    assert!(!fs::read_to_string(&p).unwrap().contains("not json"));
    assert_eq!(read(&fael).rows.len(), 2);
    assert!(
        after.problems.iter().all(|x| x.kind != ProblemKind::Broken),
        "{after:?}"
    );
}

#[test]
fn torn_tail_moves_to_quarantine() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    let good = row("A0000000000000000000000001", "note", &["a.rs"]).to_line();
    fs::write(&p, format!("{good}\n{{\"v\":1,\"id\":\"TOR")).unwrap();
    let (before, _, after) = scan_fix_rescan(&fael, &r);
    assert!(
        before.problems.iter().any(|x| x.kind == ProblemKind::Torn),
        "{before:?}"
    );
    assert_eq!(read(&fael).rows.len(), 1);
    assert!(
        after.problems.iter().all(|x| x.kind != ProblemKind::Torn),
        "{after:?}"
    );
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
    write_lines(
        &p,
        &[
            a.clone(),
            "<<<<<<< HEAD".into(),
            b.clone(),
            "=======".into(),
            ">>>>>>> dev".into(),
        ],
    );
    let (before, done, after) = scan_fix_rescan(&fael, &r);
    assert!(
        before
            .problems
            .iter()
            .any(|x| x.kind == ProblemKind::Conflict),
        "{before:?}"
    );
    assert!(
        done.iter().any(|d| d.contains("3 marker(s) stripped")),
        "{done:?}"
    );
    let body = fs::read_to_string(&p).unwrap();
    assert!(!body.contains("<<<<<<<") && body.contains(&a) && body.contains(&b));
    assert_eq!(read(&fael).rows.len(), 2);
    assert!(
        after
            .problems
            .iter()
            .all(|x| x.kind == ProblemKind::Conflict),
        "{after:?}"
    );
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
    assert!(
        before
            .problems
            .iter()
            .any(|x| x.kind == ProblemKind::Encoding),
        "{before:?}"
    );
    let fixed = fs::read(&p).unwrap();
    assert!(!fixed.starts_with(b"\xef\xbb\xbf") && !fixed.contains(&b'\r'));
    assert_eq!(read(&fael).rows.len(), 1);
    assert!(
        after
            .problems
            .iter()
            .all(|x| x.kind == ProblemKind::Encoding),
        "{after:?}"
    );
}

#[test]
fn duplicates_are_report_only() {
    let r = root();
    let fael = fael_of(&r);
    let line = row("A0000000000000000000000001", "note", &["a.rs"]).to_line();
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        std::slice::from_ref(&line),
    );
    write_lines(&month_file(&fael, "other-0000", "2026-07", false), &[line]);
    let (before, _, after) = scan_fix_rescan(&fael, &r);
    let dupe = before
        .problems
        .iter()
        .find(|x| x.kind == ProblemKind::Duplicate)
        .unwrap();
    assert!(!dupe.fixable);
    assert_eq!(read(&fael).rows.len(), 1); // read dedupes first-wins
    assert!(
        after
            .problems
            .iter()
            .any(|x| x.kind == ProblemKind::Duplicate)
    );
}

#[test]
fn rows_without_files_and_future_month_are_notes() {
    let r = root();
    let fael = fael_of(&r);
    let mut legacy = row("legacy-1a2b3c4d", "note", &[]);
    legacy.files = vec![];
    write_lines(
        &month_file(&fael, "tester-0000", "2026-07", false),
        &[legacy.to_line()],
    );
    write_lines(
        &month_file(&fael, "tester-0000", "2999-01", false),
        &[row("A0000000000000000000000009", "note", &["a.rs"]).to_line()],
    );
    let (before, done, _) = scan_fix_rescan(&fael, &r);
    assert!(
        before
            .problems
            .iter()
            .any(|x| x.kind == ProblemKind::NoFiles && x.severity == Severity::Info),
        "{before:?}"
    );
    assert!(
        before
            .problems
            .iter()
            .any(|x| x.kind == ProblemKind::Future),
        "{before:?}"
    );
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
    assert!(
        before.problems.iter().any(|x| x.kind == ProblemKind::Union),
        "{before:?}"
    );
    doctor_fix(&fael, &r, &before).unwrap();
    assert!(
        fs::read_to_string(r.join(".gitattributes"))
            .unwrap()
            .contains(".fael/log/**/*.jsonl merge=union")
    );
    let after = doctor_scan(&fael, &r, false, MONTH);
    assert!(
        after.problems.iter().all(|x| x.kind != ProblemKind::Union),
        "{after:?}"
    );
}

#[test]
fn several_fael_dirs_are_reported() {
    let r = root();
    let fael = fael_of(&r);
    let sub = r.join("sub").join(".fael").join("log").join("w-0000");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("2026-07.jsonl"), "{}\n").unwrap();
    let rep = doctor_scan(&fael, &r, false, MONTH);
    let m = rep
        .problems
        .iter()
        .find(|x| x.kind == ProblemKind::MultiFael)
        .unwrap();
    assert!(m.detail.contains("sub/.fael"), "{}", m.detail);
}

#[test]
fn oversize_month_is_an_error() {
    let r = root();
    let fael = fael_of(&r);
    let p = month_file(&fael, "tester-0000", "2026-07", false);
    write_lines(
        &p,
        &[row("A0000000000000000000000001", "note", &["a.rs"]).to_line()],
    );
    fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(MONTH_MAX)
        .unwrap();
    let rep = doctor_scan(&fael, &r, false, MONTH);
    assert!(
        rep.problems
            .iter()
            .any(|x| x.kind == ProblemKind::Oversize && !x.fixable),
        "{rep:?}"
    );
}
