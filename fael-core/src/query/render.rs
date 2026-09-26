use super::{closed, superseded};
use crate::{Log, Row};

/// Estimated tokens — derived at read time, never stored (every model's tokenizer differs).
// ponytail: uncalibrated — ASCII ≈ 4 bytes/token, anything else (Thai) ≈ 1 char/token;
// calibrate against o200k + Claude count_tokens (SPEC §8, §12) and publish the error
pub fn est_tokens(s: &str) -> usize {
    let (ascii, other) = s.chars().fold((0usize, 0usize), |(a, o), c| {
        if c.is_ascii() { (a + 1, o) } else { (a, o + 1) }
    });
    ascii.div_ceil(4) + other
}

/// Shortest id prefix (≥ 8) that is still unique across the log — what `render` prints.
pub fn abbrev(log: &Log) -> usize {
    let mut ids: Vec<&str> = log.rows.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    ids.windows(2)
        .map(|w| {
            w[0].bytes()
                .zip(w[1].bytes())
                .take_while(|(a, b)| a == b)
                .count()
                + 1
        })
        .max()
        .unwrap_or(0)
        .max(8)
}

/// One markdown line per row — `- [id] kind #key title → files` — stopping once `budget`
/// estimated tokens are used (the first row always shows). A last line counts what was cut.
/// The line shows the title (`Row::display_title`), never the body — bodies come
/// back via `render_full` (`find <id>`, `--full`).
pub fn render(log: &Log, rows: &[&Row], budget: usize) -> String {
    render_inner(log, rows, budget, false)
}

/// The same rows with bodies: each title line plus the full `text` indented
/// below it — what `find <id>` and `--full` show (skim titles, read on demand).
pub fn render_full(log: &Log, rows: &[&Row], budget: usize) -> String {
    render_inner(log, rows, budget, true)
}

fn render_inner(log: &Log, rows: &[&Row], budget: usize, full: bool) -> String {
    let width = abbrev(log);
    let (closed, superseded) = (closed(log), superseded(log));
    let mut out = String::new();
    let mut used = 0;
    for (i, r) in rows.iter().enumerate() {
        let id = r.id.get(..width).unwrap_or(&r.id);
        let mark = if closed.contains(r.id.as_str()) {
            " (closed)"
        } else if superseded.contains(r.id.as_str()) {
            " (superseded)"
        } else {
            ""
        };
        let key = r.key.as_ref().map(|k| format!(" #{k}")).unwrap_or_default();
        // `(urgent 1, to: ploy)` — whichever of the two is set, urgent first
        let route = match (r.urgent_value().map(|u| format!("urgent {u}")), r.to_who()) {
            (Some(u), Some(t)) => format!(" ({u}, to: {t})"),
            (Some(u), None) => format!(" ({u})"),
            (None, Some(t)) => format!(" (to: {t})"),
            (None, None) => String::new(),
        };
        let text = r.display_title();
        let line = format!(
            "- [{id}] {}{mark}{key} {text}{route} → {}\n",
            r.kind,
            r.files.join(", ")
        );
        // bodies read on demand only: the indented full text under its title line
        let line = if full {
            let body = r.text.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("{line}  {body}\n")
        } else {
            line
        };
        used += est_tokens(&line);
        if i > 0 && used > budget {
            out.push_str(&format!(
                "… +{} more over the {budget}-token budget — narrow the filter\n",
                rows.len() - i
            ));
            break;
        }
        out.push_str(&line);
    }
    out
}
