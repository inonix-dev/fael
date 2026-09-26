//! find · brief · push · gone · kickoff · `to` — row selection over the shared log.

use super::{files, ids, log, row};
use fael_core::*;

#[test]
fn find_hides_closed_and_superseded_ranked() {
    let l = log();
    assert_eq!(ids(&find(&l, &Filter::default())), ["13", "14", "15", "12"]);
    let all = Filter {
        all: true,
        ..Filter::default()
    };
    assert_eq!(ids(&find(&l, &all)), ["13", "10", "14", "11", "15", "12"]);
}

#[test]
fn files_exact_zone_glob_and_legacy() {
    let l = log();
    assert_eq!(ids(&find(&l, &files(&["src/a.rs"]))), ["14"]);
    assert_eq!(ids(&find(&l, &files(&["src"]))), ["13", "14", "12"]); // dir = zone
    assert_eq!(ids(&find(&l, &files(&["src/"]))), ["13", "14", "12"]);
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
        ["13", "14", "12"]
    );
    assert_eq!(
        f(Filter {
            since: Some("2026-09-14".into()),
            ..Filter::default()
        }),
        ["14", "15"]
    );
}

#[test]
fn to_matches_name_part_full_id_and_no_prefix() {
    assert!(to_matches("ploy", "ploy-1a2b"));
    assert!(to_matches("ploy-1a2b", "ploy-1a2b"));
    assert!(to_matches("Ploy", "ploy-1a2b")); // case-insensitive (write lowercases)
    assert!(to_matches("mary-jane", "mary-jane-ab12")); // slug keeps inner dashes
    assert!(!to_matches("plo", "ploy-1a2b")); // no prefix match
    assert!(!to_matches("mary", "mary-jane-ab12"));
    assert!(!to_matches("ploy", "delamind-d88f"));
    assert!(!to_matches("", "ploy-1a2b"));
    assert!(!to_matches("ploy", ""));
}

#[test]
fn find_to_narrows_only_and_render_shows_to() {
    let mut l = log();
    let mut r = row("C0000000000000000000000016", "issue", &["src/a.rs"], None);
    r.to = Some("ploy".into());
    l.rows.push(r);
    let f = |f: Filter| ids(&find(&l, &f));
    assert_eq!(
        f(Filter {
            to: Some("ploy".into()),
            ..Filter::default()
        }),
        ["16"]
    );
    assert!(
        f(Filter {
            to: Some("delamind".into()),
            ..Filter::default()
        })
        .is_empty()
    );
    // `to` narrows only — the row still matches by file, and a `to`-only
    // filter is a real filter (query runs find, not the brief)
    assert_eq!(f(files(&["src/a.rs"])), ["16", "14"]);
    let (rows, _, _) = query(
        &l,
        &Filter {
            to: Some("ploy".into()),
            ..Filter::default()
        },
        &Config::default(),
    );
    assert_eq!(ids(&rows), ["16"]);
    let out = render(&l, &rows, 10_000);
    assert!(
        out.contains("text of C0000000000000000000000016 (to: ploy) → src/a.rs"),
        "{out}"
    );
    // a full writer id and its name part match either way round
    let mut r = row("C0000000000000000000000017", "issue", &["src/a.rs"], None);
    r.to = Some("delamind-d88f".into());
    l.rows.push(r);
    let to = |t: &str| {
        ids(&find(
            &l,
            &Filter {
                to: Some(t.into()),
                ..Filter::default()
            },
        ))
    };
    assert_eq!(to("delamind"), ["17"]);
    assert_eq!(to("ploy-a1b2"), ["16"]);
    // rows without `to` render exactly as before (no empty suffix)
    let out = render(&l, &find(&l, &Filter::default()), 10_000);
    assert!(!out.contains("(to:)"), "{out}");
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
    let q = |f: &[&str]| {
        ids(&push(
            &l,
            &f.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &Aliases::default(),
        ))
    };
    // src/a.rs: 14 exact, 13 same dir (src/c.rs); 10 closed and 11 superseded never push
    assert_eq!(q(&["src/a.rs"]), ["14", "13"]);
    // a directory query is a zone: everything under src/, issues first
    assert_eq!(q(&["src"]), ["13", "14", "12"]);
    // anchors push only on exact ref
    assert_eq!(q(&["doc:pricing/2026"]), ["15"]);
    assert!(q(&["doc:pricing"]).is_empty());
    assert!(push(&l, &[], &Aliases::default()).is_empty());
}

#[test]
fn push_ignores_same_dir_for_markdown() {
    let mut l = log();
    l.rows.push(row(
        "C0000000000000000000000016",
        "note",
        &[".fapony/plan/PLAN-a.md"],
        None,
    ));
    l.rows.push(row(
        "C0000000000000000000000017",
        "note",
        &[".fapony/plan/PLAN-b.md"],
        None,
    ));
    let q = |f: &[&str]| {
        ids(&push(
            &l,
            &f.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &Aliases::default(),
        ))
    };
    // the exact markdown file still pushes; the neighbouring plan does not —
    // a dir of docs is a pile of unrelated documents (chunk 6)
    assert_eq!(q(&[".fapony/plan/PLAN-a.md"]), ["16"]);
    // code keeps its same-dir tier
    assert_eq!(q(&["src/a.rs"]), ["14", "13"]);
    // a tail that ends mid multi-byte char is not markdown, and no panic
    assert!(q(&["docs/บันทึก1"]).is_empty());
}

#[test]
fn kickoff_matches_plan_anchor() {
    let r = std::env::temp_dir().join(format!("fael-kick-plan-{}", ulid()));
    std::fs::create_dir_all(r.join(".fapony/plan")).unwrap();
    std::fs::write(r.join(".fapony/plan/PLAN-foo.md"), "plan").unwrap();
    let l = Log {
        rows: vec![
            row("A0000000000000000000000010", "note", &["plan:foo"], None),
            row(
                "A0000000000000000000000011",
                "note",
                &[".fapony/plan/PLAN-bar.md"],
                None,
            ),
        ],
        closes: vec![],
        warnings: vec![],
    };
    // the PLAN path widens to its `plan:<name>` anchor; the other plan stays out
    let f = Filter {
        files: vec![".fapony/plan/PLAN-foo.md".into()],
        ..Filter::default()
    };
    assert_eq!(ids(&kickoff(&l, &f, &r, &Aliases::default())), ["10"]);
    // a PLAN- name ending mid multi-byte char widens to nothing, no panic
    assert!(kickoff(&l, &files(&["PLAN-แผน1"]), &r, &Aliases::default()).is_empty());
    // a non-plan query never matches the anchor
    assert!(kickoff(&l, &files(&["src/a.rs"]), &r, &Aliases::default()).is_empty());
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
    let got: Vec<String> = push(&l, &["src/a.rs".to_string()], &Aliases::default())
        .iter()
        .map(|r| r.id[24..].to_string())
        .collect();
    assert_eq!(got, ["14", "13", "16"]);
}

#[test]
fn gone_resolves_renames_before_calling_a_file_missing() {
    let r = std::env::temp_dir().join(format!("fael-gone-{}", ulid()));
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(r.join("b.rs"), "x").unwrap();
    std::fs::write(r.join("c.rs"), "x").unwrap();
    let al = Aliases::from_pairs(vec![
        ("a.rs".to_string(), "b.rs".to_string()),
        ("x.rs".to_string(), "y.rs".to_string()), // chain link 1
        ("y.rs".to_string(), "c.rs".to_string()), // chain link 2
    ]);
    // renamed and present under the new path: not gone (chunk 2)
    assert!(!gone(
        &r,
        &row("A0000000000000000000000010", "note", &["a.rs"], None),
        &al
    ));
    // same row without the resolver: gone, the pre-chunk-2 behaviour
    assert!(gone(
        &r,
        &row("A0000000000000000000000010", "note", &["a.rs"], None),
        &Aliases::default()
    ));
    // rename chain resolves to the end: x.rs lives at c.rs now
    assert!(!gone(
        &r,
        &row("A0000000000000000000000011", "note", &["x.rs"], None),
        &al
    ));
    // no alias and no file: still gone
    assert!(gone(
        &r,
        &row("A0000000000000000000000012", "note", &["del.rs"], None),
        &al
    ));
    // renamed but the new path is missing too: still gone
    let al2 = Aliases::from_pairs(vec![("old.rs".to_string(), "gone2.rs".to_string())]);
    assert!(gone(
        &r,
        &row("A0000000000000000000000013", "note", &["old.rs"], None),
        &al2
    ));
    // revert then rename, in incremental-cache order: a→m, m→a, a→c still reaches c.rs
    // (01M3CVK41 — current() hit the a→m→a cycle and called it gone)
    let al3 = Aliases::from_pairs(vec![
        ("p.rs".to_string(), "m.rs".to_string()),
        ("m.rs".to_string(), "p.rs".to_string()),
        ("p.rs".to_string(), "c.rs".to_string()),
    ]);
    assert!(!gone(
        &r,
        &row("A0000000000000000000000016", "note", &["p.rs"], None),
        &al3
    ));
    // anchors and file-less rows never go
    assert!(!gone(
        &r,
        &row("A0000000000000000000000014", "note", &["doc:x"], None),
        &al
    ));
    assert!(!gone(
        &r,
        &row("A0000000000000000000000015", "note", &[], None),
        &al
    ));
}

#[test]
fn kickoff_keeps_rows_whose_files_were_renamed() {
    let r = std::env::temp_dir().join(format!("fael-kick-{}", ulid()));
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(r.join("b.rs"), "x").unwrap();
    let l = Log {
        rows: vec![row("A0000000000000000000000010", "note", &["a.rs"], None)],
        closes: vec![],
        warnings: vec![],
    };
    let al = Aliases::from_pairs(vec![("a.rs".to_string(), "b.rs".to_string())]);
    assert!(
        kickoff(&l, &Filter::default(), &r, &al)
            .iter()
            .any(|x| x.id == "A0000000000000000000000010")
    );
    // without the resolver the moved row is dropped, as before
    assert!(kickoff(&l, &Filter::default(), &r, &Aliases::default()).is_empty());
}
