//! Read-side CLI: `find` (with id lookup + `--full`), `kickoff`, `keys`.
//! Moved out of main.rs (file-size ratchet) — no logic of its own beyond the
//! title/body split: lists show titles, `find <id>` and `--full` show bodies.

use super::{Args, aliases};
use fael_core::{self as core, Filter, Log, Row};

pub(crate) fn find(a: &Args, text: Option<&String>) -> Result<(), String> {
    let r = super::repo()?;
    let log = super::read(&r);
    // `fael find <id>` pulls the body: an exact id or unique prefix wins over
    // text search (a text query equalling a unique id prefix means the id)
    if let Some(t) = text
        && let Ok(row) = core::resolve(&log, t)
    {
        return show_one(a, &log, row);
    }
    let files = core::normalize_files(&a.files(), &r.cwd, &r.root)?;
    let f = Filter {
        text: text.cloned(),
        files: aliases::load(&r, &log, true).expand_all(&files),
        key: a.one("key"),
        kind: a.one("kind"),
        since: a.one("since"),
        by: a.one("by"),
        to: a.one("to").map(|t| t.trim().to_lowercase()),
        all: a.has("all"),
    };
    let (rows, budget) = core::query(&log, &f, &r.cfg);
    show(a, &log, &rows, budget)?;
    // --all in JSON: also the close rows naming a shown row, so a consumer can tell closed from open
    if a.has("json") && f.all {
        let shown: std::collections::HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        log.closes
            .iter()
            .filter(|c| c.reference.as_deref().is_some_and(|id| shown.contains(id)))
            .for_each(|c| println!("{}", c.to_line()));
    }
    Ok(())
}

pub(crate) fn kickoff(a: &Args, anchor: Option<&String>) -> Result<(), String> {
    let r = super::repo()?;
    let files = core::normalize_files(&Vec::from_iter(anchor.cloned()), &r.cwd, &r.root)?;
    let log = super::read(&r);
    let al = aliases::load(&r, &log, true);
    let f = Filter {
        files: al.expand_all(&files),
        ..Filter::default()
    };
    show(
        a,
        &log,
        &core::kickoff(&log, &f, &r.root, &al),
        r.cfg.kickoff_tokens,
    )
}

fn show(a: &Args, log: &Log, rows: &[&Row], budget: usize) -> Result<(), String> {
    if rows.is_empty() {
        eprintln!("fael: no rows match");
    } else if a.has("json") {
        rows.iter().for_each(|r| println!("{}", r.to_line()));
    } else if a.has("full") {
        print!("{}", core::render_full(log, rows, budget));
    } else {
        print!("{}", core::render(log, rows, budget));
    }
    Ok(())
}

/// One row pulled by id: always the body (`render_full`), or the JSON line.
fn show_one(a: &Args, log: &Log, row: &Row) -> Result<(), String> {
    if a.has("json") {
        println!("{}", row.to_line());
    } else {
        print!("{}", core::render_full(log, &[row], 10_000));
    }
    Ok(())
}

pub(crate) fn keys(a: &Args, pattern: Option<&String>) -> Result<(), String> {
    let r = super::repo()?;
    let log = super::read(&r);
    let ks = core::keys(&log, pattern.map(String::as_str));
    if ks.is_empty() {
        eprintln!("fael: no keys yet");
    }
    for k in ks {
        if a.has("json") {
            println!(
                "{}",
                serde_json::json!({"key": k.key, "count": k.count, "last": k.last})
            );
        } else {
            println!(
                "- {} ×{} (last {})",
                k.key,
                k.count,
                k.last.get(..10).unwrap_or(&k.last)
            );
        }
    }
    Ok(())
}
