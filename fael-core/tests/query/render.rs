//! render · token estimates — one markdown line per row under a budget.

use super::log;
use fael_core::*;

#[test]
fn render_cuts_at_budget_but_shows_one_row() {
    let l = log();
    let rows = find(&l, &Filter::default());
    let out = render(&l, &rows, 1);
    assert_eq!(out.lines().count(), 2, "{out}");
    // ranked first is the open issue 13 (urgent, to, kind all beat newest-id)
    assert!(out.starts_with("- [A0000000000000000000000013] issue text of A0000000000000000000000013 → .\\src\\c.rs\n"));
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
fn render_shows_urgent_before_to() {
    let mut l = log();
    l.rows.push(Row {
        id: "C0000000000000000000000016".into(),
        kind: "issue".into(),
        text: "hot".into(),
        files: vec!["src/a.rs".into()],
        to: Some("ploy".into()),
        urgent: Some(1.0),
        ..Row::default()
    });
    l.rows.push(Row {
        id: "C0000000000000000000000017".into(),
        kind: "issue".into(),
        text: "half".into(),
        files: vec!["src/a.rs".into()],
        urgent: Some(0.5),
        ..Row::default()
    });
    let out = render(&l, &find(&l, &Filter::default()), 10_000);
    assert!(out.contains("hot (urgent 1, to: ploy) → src/a.rs"), "{out}");
    assert!(out.contains("half (urgent 0.5) → src/a.rs"), "{out}");
}

#[test]
fn est_tokens_counts_thai_per_char() {
    assert_eq!(est_tokens("abcdefgh"), 2);
    assert_eq!(est_tokens("ไทย"), 3);
}
