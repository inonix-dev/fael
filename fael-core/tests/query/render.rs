//! render · token estimates — one markdown line per row under a budget.

use super::log;
use fael_core::*;

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
