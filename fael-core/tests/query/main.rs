//! find · brief · keys · render · resolve · warnings — against an in-memory `Log`.
//!
//! Thin entry only — the suites sit next to this file:
//! `select` (find/brief/push/gone/kickoff/`to`), `render` (render/tokens),
//! `lookup` (resolve/keys/warnings/glob). The shared builders live here.

mod lookup;
mod render;
mod select;

use fael_core::*;

fn row(id: &str, kind: &str, files: &[&str], key: Option<&str>) -> Row {
    Row {
        id: id.into(),
        ts: format!("2026-09-{}T00:00:00Z", &id[id.len() - 2..]),
        kind: kind.into(),
        text: format!("text of {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        key: key.map(String::from),
        ..Row::default()
    }
}

fn log() -> Log {
    let mut sup = row(
        "A0000000000000000000000014",
        "decision",
        &["src/a.rs"],
        Some("auth:session"),
    );
    sup.supersedes = Some("A0000000000000000000000011".into());
    Log {
        rows: vec![
            row(
                "A0000000000000000000000010",
                "issue",
                &["src/a.rs"],
                Some("auth:session"),
            ),
            row(
                "A0000000000000000000000011",
                "decision",
                &["src/a.rs"],
                None,
            ),
            row(
                "A0000000000000000000000012",
                "note",
                &["src/sub/b.rs"],
                Some("billing:invoice"),
            ),
            row(
                "A0000000000000000000000013",
                "issue",
                &[".\\src\\c.rs"],
                None,
            ), // legacy spelling
            sup,
            row(
                "B0000000000000000000000015",
                "note",
                &["doc:pricing/2026"],
                None,
            ),
        ],
        closes: vec![Row::close("t-0000", "A0000000000000000000000010", "fixed")],
        warnings: vec![],
    }
}

fn ids(rows: &[&Row]) -> Vec<String> {
    rows.iter().map(|r| r.id[24..].to_string()).collect()
}

fn files(f: &[&str]) -> Filter {
    Filter {
        files: f.iter().map(|s| s.to_string()).collect(),
        ..Filter::default()
    }
}
