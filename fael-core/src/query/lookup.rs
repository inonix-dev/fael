use super::Filter;
use super::{est_tokens, glob};
use crate::{Config, Log, Row};
use std::collections::HashMap;

/// A row by exact id or a unique prefix (like a git sha, case-insensitive).
pub fn resolve<'a>(log: &'a Log, prefix: &str) -> Result<&'a Row, String> {
    if let Some(r) = log.rows.iter().find(|r| r.id == prefix) {
        return Ok(r);
    }
    let hits: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| {
            !prefix.is_empty()
                && r.id
                    .get(..prefix.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
        })
        .collect();
    match hits.as_slice() {
        [r] => Ok(r),
        [] => Err(format!(
            "rejected: no row with id {prefix:?} — copy the id from fael find"
        )),
        _ => Err(format!(
            "rejected: id {prefix:?} matches {} rows ({}) — use more characters",
            hits.len(),
            hits.iter()
                .take(5)
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// One key in use: how many rows carry it and the `ts` of the newest.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyUse {
    pub key: String,
    pub count: usize,
    pub last: String,
}

/// Every key matching `pattern` (all when `None`), most used first — so agents reuse one.
pub fn keys(log: &Log, pattern: Option<&str>) -> Vec<KeyUse> {
    let mut by: HashMap<&str, (usize, &Row)> = HashMap::new();
    for r in &log.rows {
        let Some(k) = r.key.as_deref() else { continue };
        if pattern.is_some_and(|p| !glob(p, k)) {
            continue;
        }
        let e = by.entry(k).or_insert((0, r));
        e.0 += 1;
        if r.id > e.1.id {
            e.1 = r;
        }
    }
    let mut out: Vec<KeyUse> = by
        .into_iter()
        .map(|(k, (count, r))| KeyUse {
            key: k.into(),
            count,
            last: r.ts.clone(),
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    out
}

/// No filter = the session brief under the kickoff budget; otherwise find under the find budget.
pub fn query<'a>(log: &'a Log, f: &Filter, cfg: &Config) -> (Vec<&'a Row>, usize) {
    if f.is_empty() && !f.all {
        (super::brief(log, f), cfg.kickoff_tokens)
    } else {
        (super::find(log, f), cfg.find_tokens)
    }
}

/// Warnings for a row about to be added — never a reject: a key domain the repo did not declare,
/// a new key close to an existing one, text over `warn.row_tokens`.
pub fn warnings(row: &Row, log: &Log, cfg: &Config) -> Vec<String> {
    let mut w = vec![];
    if let Some(k) = &row.key {
        let domain = k.split(':').next().unwrap_or(k);
        if !cfg.key_domains.is_empty() && !cfg.key_domains.iter().any(|d| d == domain) {
            w.push(format!(
                "warning: key domain {domain:?} is not in config key_domains ({}) — reuse one if it fits",
                cfg.key_domains.join(", ")
            ));
        }
        let used = keys(log, None);
        if !used.iter().any(|u| &u.key == k) {
            let parent = |s: &str| s.rsplit_once(':').map(|(p, _)| p.to_string());
            let similar: Vec<&str> = used
                .iter()
                .map(|u| u.key.as_str())
                .filter(|u| (parent(k).is_some() && parent(u) == parent(k)) || distance(u, k) <= 2)
                .take(5)
                .collect();
            if !similar.is_empty() {
                w.push(format!(
                    "warning: new key {k:?}, similar keys exist: {} — reuse one if it means the same",
                    similar.join(", ")
                ));
            }
        }
    }
    let t = est_tokens(&row.text);
    if t > cfg.warn_row_tokens {
        w.push(format!(
            "warning: text is ~{t} tokens (warn at {}) — every push of this row costs that",
            cfg.warn_row_tokens
        ));
    }
    w
}

/// Levenshtein distance over chars.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != *cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}
