//! limit/offset paging — after ranking, before render; the cut line names the next call.

use super::log;
use fael_core::*;

#[test]
fn page_skips_then_takes_with_total() {
    let l = log();
    let rows = find(&l, &Filter::default());
    assert_eq!(rows.len(), 4);
    // ranked: the open issue first, then the decision, then notes newest first
    let (p1, t1) = page(rows.clone(), Some(2), 0);
    assert_eq!(ids(&p1), ["13", "14"]);
    assert_eq!(t1, 4);
    let (p2, t2) = page(rows.clone(), Some(2), 2);
    assert_eq!(ids(&p2), ["15", "12"]);
    assert_eq!(t2, 4);
    // past the end: empty page, the total still counts
    let (p3, t3) = page(rows, Some(2), 9);
    assert!(p3.is_empty());
    assert_eq!(t3, 4);
}

#[test]
fn query_applies_filter_paging() {
    let l = log();
    let f = Filter {
        limit: Some(2),
        offset: 1,
        ..Filter::default()
    };
    let (rows, _, total) = query(&l, &f, &Config::default());
    assert_eq!(ids(&rows), ["14", "15"]);
    assert_eq!(total, 4);
}

#[test]
fn render_page_cut_names_next_call() {
    let l = log();
    let (rows, _, total) = query(
        &l,
        &Filter {
            limit: Some(2),
            ..Filter::default()
        },
        &Config::default(),
    );
    let out = render_page(
        &l,
        &rows,
        10_000,
        Cut {
            total,
            offset: 0,
            next: &|n| format!("fael find --limit 2 --offset {n}"),
        },
    );
    assert_eq!(out.lines().count(), 3, "{out}");
    assert!(
        out.ends_with("… +2 more — next: fael find --limit 2 --offset 2\n"),
        "{out}"
    );
    // the last page names nothing
    let (rows, _, total) = query(
        &l,
        &Filter {
            limit: Some(2),
            offset: 2,
            ..Filter::default()
        },
        &Config::default(),
    );
    let out = render_page(
        &l,
        &rows,
        10_000,
        Cut {
            total,
            offset: 2,
            next: &|n| format!("fael find --limit 2 --offset {n}"),
        },
    );
    assert!(!out.contains("more — next:"), "{out}");
}

#[test]
fn render_page_budget_cut_offsets_by_shown() {
    let l = log();
    let rows = find(&l, &Filter::default());
    let out = render_page(
        &l,
        &rows,
        1,
        Cut {
            total: rows.len(),
            offset: 0,
            next: &|n| format!("fael find --offset {n}"),
        },
    );
    // one row shown under budget 1, three left, the offset follows the shown rows
    assert!(
        out.ends_with("… +3 more — next: fael find --offset 1\n"),
        "{out}"
    );
}

fn ids(rows: &[&Row]) -> Vec<String> {
    rows.iter().map(|r| r.id[24..].to_string()).collect()
}
