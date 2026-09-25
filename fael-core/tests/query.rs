//! find · brief · keys · render · resolve · warnings — against an in-memory `Log`.

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

#[test]
fn find_hides_closed_and_superseded_newest_first() {
    let l = log();
    assert_eq!(ids(&find(&l, &Filter::default())), ["15", "14", "13", "12"]);
    let all = Filter {
        all: true,
        ..Filter::default()
    };
    assert_eq!(ids(&find(&l, &all)), ["15", "14", "13", "12", "11", "10"]);
}

#[test]
fn files_exact_zone_glob_and_legacy() {
    let l = log();
    assert_eq!(ids(&find(&l, &files(&["src/a.rs"]))), ["14"]);
    assert_eq!(ids(&find(&l, &files(&["src"]))), ["14", "13", "12"]); // dir = zone
    assert_eq!(ids(&find(&l, &files(&["src/"]))), ["14", "13", "12"]);
    assert!(find(&l, &files(&["sr"])).is_empty()); // prefix must end at a `/`
    assert_eq!(ids(&find(&l, &files(&["src/c.rs"]))), ["13"]); // `.\src\c.rs` read leniently
    assert_eq!(ids(&find(&l, &files(&["src/*/*.rs"]))), ["12"]);
    // an anchor ref is opaque: no zone match on `/`
    assert!(find(&l, &files(&["doc:pricing"])).is_empty());
    assert_eq!(ids(&find(&l, &files(&["doc:pricing/2026"]))), ["15"]);
    assert_eq!(ids(&find(&l, &files(&["doc:*"]))), ["15"]);
}

#[test]
fn key_kind_text_since() {
    let l = log();
    let f = |f: Filter| ids(&find(&l, &f));
    assert_eq!(
        f(Filter {
            key: Some("auth:*".into()),
            ..Filter::default()
        }),
        ["14"]
    );
    assert_eq!(
        f(Filter {
            kind: Some("issue".into()),
            ..Filter::default()
        }),
        ["13"]
    );
    assert_eq!(
        f(Filter {
            text: Some("TEXT OF A0".into()),
            ..Filter::default()
        }),
        ["14", "13", "12"]
    );
    assert_eq!(
        f(Filter {
            since: Some("2026-09-14".into()),
            ..Filter::default()
        }),
        ["15", "14"]
    );
}

#[test]
fn brief_puts_issues_then_decisions_then_notes() {
    let l = log();
    assert_eq!(
        ids(&brief(&l, &Filter::default())),
        ["13", "14", "15", "12"]
    );
}

#[test]
fn push_ranks_exact_then_dir_then_key() {
    let l = log();
    let q = |f: &[&str]| ids(&push(&l, &f.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
    // src/a.rs: 14 exact, 13 same dir (src/c.rs); 10 closed and 11 superseded never push
    assert_eq!(q(&["src/a.rs"]), ["14", "13"]);
    // a directory query is a zone: everything under src/, issues first
    assert_eq!(q(&["src"]), ["13", "14", "12"]);
    // anchors push only on exact ref
    assert_eq!(q(&["doc:pricing/2026"]), ["15"]);
    assert!(q(&["doc:pricing"]).is_empty());
    assert!(push(&l, &[]).is_empty());
}

#[test]
fn push_shares_key_with_exact_hit() {
    let mut l = log();
    l.rows.push(row(
        "C0000000000000000000000016",
        "note",
        &["elsewhere/z.rs"],
        Some("auth:session"), // same key as the exact hit 14
    ));
    let got: Vec<String> = push(
        &l,
        &["src/a.rs".to_string()],
    )
    .iter()
    .map(|r| r.id[24..].to_string())
    .collect();
    assert_eq!(got, ["14", "13", "16"]);
}

#[test]
fn redis_glob() {
    assert!(glob("auth:*", "auth:session:timeout"));
    assert!(glob("a?c", "abc") && !glob("a?c", "ac"));
    assert!(glob("h[ae]llo", "hello") && !glob("h[ae]llo", "hillo"));
    assert!(glob("h[^e]llo", "hallo") && !glob("h[^e]llo", "hello"));
    assert!(glob("v[0-9]", "v7") && !glob("v[0-9]", "vx"));
    assert!(glob("a\\*", "a*") && !glob("a\\*", "ab"));
    assert!(!glob("auth:*", "billing:x"));
}

#[test]
fn resolve_by_unique_prefix() {
    let l = log();
    assert_eq!(resolve(&l, "b0").unwrap().id, "B0000000000000000000000015"); // case-insensitive
    assert!(resolve(&l, "A000").unwrap_err().contains("matches 5 rows"));
    assert!(resolve(&l, "Z").unwrap_err().contains("no row"));
    assert!(resolve(&l, "").is_err());
}

#[test]
fn render_cuts_at_budget_but_shows_one_row() {
    let l = log();
    let rows = find(&l, &Filter::default());
    let out = render(&l, &rows, 1);
    assert_eq!(out.lines().count(), 2, "{out}");
    // A…10 and A…11 differ only in the last char → the one width every id gets is all 26
    assert!(out.starts_with("- [B0000000000000000000000015] note text of B0000000000000000000000015 → doc:pricing/2026\n"));
    assert!(out.ends_with("… +3 more over the 1-token budget — narrow the filter\n"));
    let all = Filter {
        all: true,
        ..Filter::default()
    };
    let out = render(&l, &find(&l, &all), 10_000);
    assert!(
        out.contains("- [A0000000000000000000000011] decision (superseded) text"),
        "{out}"
    );
    assert!(out.contains("issue (closed) #auth:session"), "{out}");
}

#[test]
fn est_tokens_counts_thai_per_char() {
    assert_eq!(est_tokens("abcdefgh"), 2);
    assert_eq!(est_tokens("ไทย"), 3);
}

#[test]
fn keys_by_count() {
    let mut l = log();
    l.rows.push(row(
        "C0000000000000000000000016",
        "note",
        &["x"],
        Some("billing:invoice"),
    ));
    l.rows.push(row(
        "C0000000000000000000000017",
        "note",
        &["x"],
        Some("billing:invoice"),
    ));
    let k = keys(&l, None);
    assert_eq!(k[0].key, "billing:invoice");
    assert_eq!(k[0].count, 3);
    assert_eq!(k[0].last, "2026-09-17T00:00:00Z");
    assert_eq!((k[1].key.as_str(), k[1].count), ("auth:session", 2));
    assert_eq!(keys(&l, Some("auth:*")).len(), 1);
}

#[test]
fn add_warnings_never_reject() {
    let l = log();
    let cfg = Config {
        key_domains: vec!["auth".into()],
        warn_row_tokens: 3,
        ..Config::default()
    };
    let mut r = row(
        "D0000000000000000000000017",
        "note",
        &["x"],
        Some("auth:sesion"),
    );
    let w = warnings(&r, &l, &cfg);
    assert!(w[0].contains("similar keys exist: auth:session"), "{w:?}");
    assert!(w[1].contains("tokens (warn at 3)"), "{w:?}");
    r.key = Some("hiring:backend".into());
    assert!(warnings(&r, &l, &cfg)[0].contains("not in config key_domains"));
    r.key = Some("auth:session".into()); // existing key: no similarity noise
    r.text = "ok".into();
    assert!(warnings(&r, &l, &cfg).is_empty());
}
