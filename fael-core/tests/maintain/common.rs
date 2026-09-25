use fael_core::*;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const MONTH: &str = "2026-09";

pub(crate) fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fael-maint-{}", ulid()));
    fs::create_dir_all(&d).unwrap();
    d
}

/// A repo root with the union line present (tests that want it missing delete it).
pub(crate) fn root() -> PathBuf {
    let r = tmp();
    fs::write(r.join(".gitattributes"), "*.jsonl merge=union\n").unwrap();
    r
}

pub(crate) fn fael_of(root: &Path) -> PathBuf {
    root.join(".fael")
}

pub(crate) fn row(id: &str, kind: &str, files: &[&str]) -> Row {
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

pub(crate) fn month_file(fael: &Path, writer: &str, month: &str, close: bool) -> PathBuf {
    let dir = fael.join("log").join(writer);
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!(
        "{month}{}.jsonl",
        if close { ".close" } else { "" }
    ))
}

pub(crate) fn write_lines(path: &Path, lines: &[String]) {
    let mut body = lines.join("\n");
    if !lines.is_empty() {
        body.push('\n');
    }
    fs::write(path, body).unwrap();
}

pub(crate) fn kinds(rep: &DoctorReport) -> Vec<(ProblemKind, Severity)> {
    rep.problems
        .iter()
        .map(|p| (p.kind.clone(), p.severity))
        .collect()
}
