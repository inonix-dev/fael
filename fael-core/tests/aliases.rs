//! L2 aliases: expand chains/dirs/anchors, current, from_log, push-through-rename.

use fael_core::*;
use serde_json::json;

fn al(pairs: &[(&str, &str)]) -> Aliases {
    Aliases::from_pairs(
        pairs
            .iter()
            .map(|(o, n)| (o.to_string(), n.to_string()))
            .collect(),
    )
}

fn row_on(id: &str, files: &[&str]) -> Row {
    Row {
        id: id.into(),
        ts: "2026-09-20T00:00:00Z".into(),
        kind: "decision".into(),
        text: format!("text of {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        ..Row::default()
    }
}

#[test]
fn expand_follows_chains() {
    let a = al(&[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")]);
    assert_eq!(a.expand("src/c.rs"), ["src/c.rs", "src/b.rs", "src/a.rs"]);
    assert_eq!(a.expand("src/b.rs"), ["src/b.rs", "src/a.rs"]);
    assert_eq!(a.expand("src/a.rs"), ["src/a.rs"]);
    assert_eq!(a.expand("src/other.rs"), ["src/other.rs"]);
}

#[test]
fn expand_maps_zone_queries_across_dir_renames() {
    let a = al(&[("src/old/x.rs", "src/new/x.rs")]);
    assert_eq!(a.expand("src/new/x.rs"), ["src/new/x.rs", "src/old/x.rs"]);
    // a zone query for the new directory finds the old one
    let got = a.expand("src/new");
    assert!(got.contains(&"src/new".to_string()), "{got:?}");
    assert!(got.contains(&"src/old".to_string()), "{got:?}");
    // ...but a lone same-dir rename says nothing about neighbours:
    // "src" is not a parent dir of "src/new/x.rs" in the path-boundary sense
    let b = al(&[("src/a.rs", "src/b.rs")]);
    assert_eq!(b.expand("src"), ["src"]);
}

#[test]
fn expand_swap_pushes_both_sides() {
    let a = al(&[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/a.rs")]);
    assert!(a.expand("src/a.rs").contains(&"src/b.rs".to_string()));
    assert!(a.expand("src/b.rs").contains(&"src/a.rs".to_string()));
}

#[test]
fn expand_anchors_exact_only_and_globs_pass_through() {
    let a = al(&[("doc:pricing", "doc:pricing-2027")]);
    assert_eq!(
        a.expand("doc:pricing-2027"),
        ["doc:pricing-2027", "doc:pricing"]
    );
    // `/` inside an anchor ref is not a directory
    let b = al(&[("doc:a/x", "doc:b/x")]);
    assert_eq!(b.expand("doc:b/y"), ["doc:b/y"]);
    // globs are find's job, not the alias set's
    assert_eq!(a.expand("src/*.rs"), ["src/*.rs"]);
    // no pairs = identity
    assert_eq!(Aliases::default().expand("src/a.rs"), ["src/a.rs"]);
}

#[test]
fn current_follows_to_the_end() {
    let a = al(&[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")]);
    assert_eq!(a.current("src/a.rs").as_deref(), Some("src/c.rs"));
    assert_eq!(a.current("src/b.rs").as_deref(), Some("src/c.rs"));
    assert_eq!(a.current("src/c.rs"), None); // nowhere further — don't know
    assert_eq!(a.current("src/other.rs"), None);
}

#[test]
fn current_cycle_and_dir_prefix() {
    let swap = al(&[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/a.rs")]);
    assert_eq!(swap.current("src/a.rs"), None); // no single answer
    // a directory-level pair (from `fael mv`) maps everything under it
    let dir = al(&[("src/old", "src/new")]);
    assert_eq!(dir.current("src/old/x.rs").as_deref(), Some("src/new/x.rs"));
    assert_eq!(dir.current("src/other/x.rs"), None);
    assert_eq!(dir.current("src/old2/x.rs"), None); // boundary: old2 ≠ old/
}

#[test]
fn from_log_reads_moved_rows() {
    let mut moved = row_on("M0000000000000000000000001", &[]);
    moved.kind.clear();
    moved.text = "mv".into();
    moved.files.clear();
    moved
        .extra
        .insert("moved".into(), json!({"from": "doc:a", "to": "doc:b"}));
    let log = Log {
        rows: vec![moved],
        ..Log::default()
    };
    let a = Aliases::from_log(&log);
    assert_eq!(a.expand("doc:b"), ["doc:b", "doc:a"]);
}

#[test]
fn push_finds_rows_filed_under_the_old_path() {
    let log = Log {
        rows: vec![row_on("A0000000000000000000000001", &["src/a.rs"])],
        ..Log::default()
    };
    // across directories, so only the alias can match (same-dir tier would
    // hit too if the rename stayed in src/)
    let renamed = al(&[("src/a.rs", "lib/b.rs")]);
    let q = |f: &str| vec![f.to_string()];
    assert_eq!(push(&log, &q("lib/b.rs"), &renamed).len(), 1);
    assert!(push(&log, &q("lib/b.rs"), &Aliases::default()).is_empty());
    // the old path still matches too (the log is append-only)
    assert_eq!(push(&log, &q("src/a.rs"), &renames_chain()).len(), 1);
}

fn renames_chain() -> Aliases {
    al(&[("src/a.rs", "src/b.rs"), ("src/b.rs", "src/c.rs")])
}

#[test]
fn push_at_chain_end_finds_the_original_row() {
    let log = Log {
        rows: vec![row_on("A0000000000000000000000001", &["src/a.rs"])],
        ..Log::default()
    };
    let got = push(&log, &["src/c.rs".to_string()], &renames_chain());
    assert_eq!(got.len(), 1);
    // a zone query for the new directory finds old-dir rows
    let log2 = Log {
        rows: vec![row_on("A0000000000000000000000002", &["src/old/x.rs"])],
        ..Log::default()
    };
    let dir = al(&[("src/old/x.rs", "src/new/x.rs")]);
    assert_eq!(push(&log2, &["src/new".to_string()], &dir).len(), 1);
}

#[test]
fn moved_row_validates_and_feeds_from_log() {
    let row = Row::moved("t-1234", "src/a.rs", "src/b.rs");
    assert!(row.kind.is_empty() && row.files.is_empty());
    validate_alias(&row, &Config::default()).unwrap();
    let moved = row.extra.get("moved").unwrap();
    assert_eq!(moved.get("from").and_then(|v| v.as_str()), Some("src/a.rs"));
    assert_eq!(moved.get("to").and_then(|v| v.as_str()), Some("src/b.rs"));
    let log = Log {
        rows: vec![row],
        ..Log::default()
    };
    assert_eq!(
        Aliases::from_log(&log).expand("src/b.rs"),
        ["src/b.rs", "src/a.rs"]
    );
}

#[test]
fn validate_alias_rejects_bad_shapes() {
    let cfg = Config::default();
    // self-pair
    assert!(validate_alias(&Row::moved("t-1", "src/a.rs", "src/a.rs"), &cfg).is_err());
    // empty ends
    assert!(validate_alias(&Row::moved("t-1", "", "src/b.rs"), &cfg).is_err());
    assert!(validate_alias(&Row::moved("t-1", "src/a.rs", ""), &cfg).is_err());
    // not repo-relative
    assert!(validate_alias(&Row::moved("t-1", "../a.rs", "src/b.rs"), &cfg).is_err());
    // a carrier must not smuggle kind/files/ref
    let mut kinded = Row::moved("t-1", "src/a.rs", "src/b.rs");
    kinded.kind = "note".into();
    assert!(validate_alias(&kinded, &cfg).is_err());
    let mut filed = Row::moved("t-1", "src/a.rs", "src/b.rs");
    filed.files = vec!["src/a.rs".into()];
    assert!(validate_alias(&filed, &cfg).is_err());
    let mut closed = Row::moved("t-1", "src/a.rs", "src/b.rs");
    closed.reference = Some("A0000000000000000000000001".into());
    assert!(validate_alias(&closed, &cfg).is_err());
    // no moved object at all
    assert!(validate_alias(&row_on("A0000000000000000000000001", &["src/a.rs"]), &cfg).is_err());
    // anchors move too (git never sees them)
    validate_alias(&Row::moved("t-1", "doc:a", "doc:b"), &cfg).unwrap();
}

#[test]
fn missing_lists_only_open_unresolved_paths() {
    let dir = std::env::temp_dir().join(format!("fael-missing-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("here.rs"), "// here\n").unwrap();
    let present = row_on("A0000000000000000000000001", &["here.rs"]);
    let gone = row_on("A0000000000000000000000002", &["away.rs"]);
    let anchored = row_on("A0000000000000000000000003", &["doc:pricing"]);
    let shut = row_on("A0000000000000000000000004", &["shut.rs"]);
    let mut carrier = row_on("M0000000000000000000000001", &[]);
    carrier.kind.clear();
    carrier.files.clear();
    carrier
        .extra
        .insert("moved".into(), json!({"from": "away.rs", "to": "here.rs"}));
    let log = Log {
        rows: vec![present, gone.clone(), anchored, shut.clone(), carrier],
        closes: vec![Row::close("t-1", &shut.id, "done")],
        ..Log::default()
    };
    let al = Aliases::default();
    // away.rs is missing; here.rs exists; anchors/closed rows/carriers never list
    assert_eq!(al.missing(&dir, &log), ["away.rs"]);
    // ...unless a rename resolves it
    let al2 = Aliases::from_log(&log);
    assert!(al2.missing(&dir, &log).is_empty());
    let _ = gone;
}

#[test]
fn find_and_push_skip_alias_carrier_rows() {
    let mut moved = row_on("M0000000000000000000000001", &[]);
    moved.kind.clear();
    moved
        .extra
        .insert("moved".into(), json!({"from": "doc:a", "to": "doc:b"}));
    let log = Log {
        rows: vec![row_on("A0000000000000000000000001", &["src/a.rs"]), moved],
        ..Log::default()
    };
    let all = find(
        &log,
        &Filter {
            all: true,
            ..Filter::default()
        },
    );
    assert_eq!(all.len(), 1);
    assert_eq!(
        push(&log, &["doc:b".to_string()], &Aliases::from_log(&log)).len(),
        0
    );
}
