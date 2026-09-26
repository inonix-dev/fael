//! urgent queue + bump + the 6-step rank — fractional ordering, MVCC-style
//! new versions, and one ordering for every list.

use super::ids;
use fael_core::*;

fn issue(id: &str, urgent: Option<f64>) -> Row {
    Row {
        id: id.into(),
        ts: "2026-09-20T00:00:00Z".into(),
        kind: "issue".into(),
        text: format!("text of {id}"),
        files: vec!["src/a.rs".into()],
        urgent,
        ..Row::default()
    }
}

fn queue() -> Log {
    Log {
        rows: vec![
            issue("U0000000000000000000000001", Some(2.0)),
            issue("U0000000000000000000000002", Some(1.0)),
        ],
        closes: vec![],
        warnings: vec![],
    }
}

#[test]
fn urgent_end_is_max_plus_one() {
    let empty = Log::default();
    assert_eq!(
        resolve_urgent(&empty, &Urgent::End).unwrap(),
        Some(1.0)
    );
    let l = queue();
    assert_eq!(resolve_urgent(&l, &Urgent::End).unwrap(), Some(3.0));
    assert_eq!(resolve_urgent(&l, &Urgent::Unset).unwrap(), None);
}

#[test]
fn urgent_before_is_the_midpoint_above() {
    let l = queue();
    // the top halves (the queue starts at 1, so halving never crosses zero)
    assert_eq!(
        resolve_urgent(&l, &Urgent::Before("U0000000000000000000000002".into())).unwrap(),
        Some(0.5)
    );
    assert_eq!(
        resolve_urgent(&l, &Urgent::Before("U0000000000000000000000001".into())).unwrap(),
        Some(1.5)
    );
}

#[test]
fn urgent_before_rejects_non_queue_rows() {
    let mut l = queue();
    l.rows.push(issue("U0000000000000000000000003", None));
    let e = resolve_urgent(&l, &Urgent::Before("U0000000000000000000000003".into())).unwrap_err();
    assert!(e.contains("has no number"), "{e}");
    let e = resolve_urgent(&l, &Urgent::Before("U0000000000000000000000009".into())).unwrap_err();
    assert!(e.contains("no row"), "{e}");
    l.closes.push(Row::close(
        "t-0000",
        "U0000000000000000000000002",
        "done",
    ));
    let e = resolve_urgent(&l, &Urgent::Before("U0000000000000000000000002".into())).unwrap_err();
    assert!(e.contains("closed or superseded"), "{e}");
}

#[test]
fn bump_rewrites_only_the_moved_row() {
    let dir = std::env::temp_dir().join(format!("fael-bump-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config::default();
    let st = Stamp {
        by: "tester-0000".into(),
        branch: None,
        sha: None,
    };
    let file = |text: &str, opt: &Urgent| {
        let log = read(&dir);
        let mut r = Row::new("tester-0000", "issue", text, vec!["src/a.rs".into()]);
        r.key = Some("auth:session".into());
        r.urgent = resolve_urgent(&log, opt).unwrap();
        add_row(&dir, &log, &cfg, &st, r, None).unwrap().0
    };
    let a = file("A keeps", &Urgent::End); // 1.0
    let b = file("B moves", &Urgent::End); // 2.0
    // done criteria: bump B --urgent-before A with A=1,B=2 → new B=0.5
    let (b2, _, _) = bump_row(
        &dir,
        &read(&dir),
        &cfg,
        &st,
        &b.id,
        Some("Ploy".into()),
        UrgentChange::Before(a.id.clone()),
    )
    .unwrap();
    assert_eq!(b2.urgent, Some(0.5));
    assert_eq!(b2.to.as_deref(), Some("ploy")); // lowercased on write
    assert_eq!(b2.text, "B moves");
    assert_eq!(b2.files, ["src/a.rs"]);
    assert_eq!(b2.key.as_deref(), Some("auth:session"));
    assert_eq!(b2.supersedes.as_deref(), Some(b.id.as_str()));
    // the old B left every list; A kept its id and number
    let l = read(&dir);
    let got = ids(&find(&l, &Filter::default()));
    assert!(!got.contains(&b.id[24..].to_string()), "{got:?}");
    let a_still = find(&l, &Filter::default())
        .into_iter()
        .find(|r| r.id == a.id)
        .unwrap();
    assert_eq!(a_still.urgent_value(), Some(1.0));
    // leaving the queue keeps the routing
    let (b3, _, _) = bump_row(&dir, &read(&dir), &cfg, &st, &b2.id, None, UrgentChange::Remove)
        .unwrap();
    assert!(b3.urgent.is_none());
    assert_eq!(b3.to.as_deref(), Some("ploy"));
}

#[test]
fn bump_rejects_hidden_rows_and_non_issue_urgent() {
    let dir = std::env::temp_dir().join(format!("fael-bump-no-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config::default();
    let st = Stamp {
        by: "tester-0000".into(),
        branch: None,
        sha: None,
    };
    let log = read(&dir);
    let mut d = Row::new("tester-0000", "decision", "locked", vec!["src/a.rs".into()]);
    d.urgent = None;
    let d = add_row(&dir, &log, &cfg, &st, d, None).unwrap().0;
    let e = bump_row(&dir, &read(&dir), &cfg, &st, &d.id, None, UrgentChange::End).unwrap_err();
    assert!(e.contains("urgent is for issues"), "{e}");
    let (gone, _, _) = bump_row(
        &dir,
        &read(&dir),
        &cfg,
        &st,
        &d.id,
        Some("ploy".into()),
        UrgentChange::Keep,
    )
    .unwrap();
    assert_eq!(gone.to.as_deref(), Some("ploy"));
    // the old version is superseded — bump the newer one instead
    let e = bump_row(&dir, &read(&dir), &cfg, &st, &d.id, None, UrgentChange::Keep).unwrap_err();
    assert!(e.contains("already superseded"), "{e}");
    close_row(&dir, &read(&dir), &cfg, &st, &gone.id, "done").unwrap();
    let e = bump_row(
        &dir,
        &read(&dir),
        &cfg,
        &st,
        &gone.id,
        None,
        UrgentChange::Keep,
    )
    .unwrap_err();
    assert!(e.contains("already closed"), "{e}");
}

/// A row with every ranking signal set — the 6-step key reads it directly.
fn full(
    id: &str,
    kind: &str,
    ts: &str,
    to: Option<&str>,
    urgent: Option<f64>,
    files: &[&str],
) -> Row {
    Row {
        id: id.into(),
        ts: ts.into(),
        kind: kind.into(),
        text: format!("text of {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        to: to.map(String::from),
        urgent,
        ..Row::default()
    }
}

fn open_log(rows: Vec<Row>) -> Log {
    Log {
        rows,
        closes: vec![],
        warnings: vec![],
    }
}

#[test]
fn rank_to_reader_beats_urgent() {
    let l = open_log(vec![
        full(
            "R0000000000000000000000001",
            "issue",
            "2026-09-20T00:00:00Z",
            None,
            Some(1.0),
            &["a.rs"],
        ),
        full(
            "R0000000000000000000000002",
            "issue",
            "2026-09-10T00:00:00Z",
            Some("ploy"),
            None,
            &["a.rs"],
        ),
    ]);
    let got = ranked(l.rows.iter().collect(), Some("ploy-1a2b"), |_| 0, fresh_ts);
    assert_eq!(ids(&got), ["02", "01"]);
}

#[test]
fn rank_urgent_beats_match_tier() {
    let l = open_log(vec![
        full(
            "R0000000000000000000000001",
            "note",
            "2026-09-10T00:00:00Z",
            None,
            Some(1.0),
            &["src/b.rs"],
        ),
        full(
            "R0000000000000000000000002",
            "issue",
            "2026-09-10T00:00:00Z",
            None,
            None,
            &["src/a.rs"],
        ),
    ]);
    // exact-file loses to same-dir once the same-dir row is urgent
    let got = push(&l, &["src/a.rs".to_string()], &Aliases::default());
    assert_eq!(ids(&got), ["01", "02"]);
}

#[test]
fn rank_kind_beats_freshness() {
    let l = open_log(vec![
        full(
            "R0000000000000000000000001",
            "note",
            "2026-09-20T00:00:00Z",
            None,
            None,
            &["a.rs"],
        ),
        full(
            "R0000000000000000000000002",
            "issue",
            "2026-09-10T00:00:00Z",
            None,
            None,
            &["a.rs"],
        ),
    ]);
    assert_eq!(ids(&find(&l, &Filter::default())), ["02", "01"]);
}

#[test]
fn rank_freshness_beats_id() {
    let l = open_log(vec![
        full(
            "B0000000000000000000000001",
            "note",
            "2026-09-10T00:00:00Z",
            None,
            None,
            &["a.rs"],
        ),
        full(
            "A0000000000000000000000002",
            "note",
            "2026-09-20T00:00:00Z",
            None,
            None,
            &["a.rs"],
        ),
    ]);
    assert_eq!(ids(&find(&l, &Filter::default())), ["02", "01"]);
}

#[test]
fn rank_id_desc_breaks_full_ties() {
    let l = open_log(vec![
        full(
            "A0000000000000000000000001",
            "note",
            "2026-09-10T00:00:00Z",
            None,
            None,
            &["a.rs"],
        ),
        full(
            "A0000000000000000000000002",
            "note",
            "2026-09-10T00:00:00Z",
            None,
            None,
            &["a.rs"],
        ),
    ]);
    assert_eq!(ids(&find(&l, &Filter::default())), ["02", "01"]);
}
