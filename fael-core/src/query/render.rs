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

/// One markdown line per row — `- [id] kind #key text → files` — stopping once `budget`
/// estimated tokens are used (the first row always shows). A last line counts what was cut.
pub fn render(log: &Log, rows: &[&Row], budget: usize) -> String {
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
        let text = r.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = format!(
            "- [{id}] {}{mark}{key} {text} → {}\n",
            r.kind,
            r.files.join(", ")
        );
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
