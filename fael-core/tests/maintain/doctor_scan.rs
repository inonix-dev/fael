use crate::common::*;
use fael_core::*;
use std::fs;

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
    assert!(
        ks.contains(&(ProblemKind::Union, Severity::Error)),
        "{ks:?}"
    );
    assert!(
        ks.contains(&(ProblemKind::Ignored, Severity::Error)),
        "{ks:?}"
    );
}
