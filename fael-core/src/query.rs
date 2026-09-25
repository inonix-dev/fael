//! find · brief · keys · render — what the CLI, MCP and hooks show, built on `read()`'s `Log`.
//! Deterministic: newest first by `id` (never `ts` — clocks differ across machines).

use crate::{Config, Log, Row, anchor};
use std::collections::{HashMap, HashSet};

/// What `find` narrows by. Every field is optional; `files` holds normalised refs.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// case-insensitive substring of `text`
    pub text: Option<String>,
    /// exact · dir prefix (a zone) · glob — any one matching any row file is a hit
    pub files: Vec<String>,
    /// Redis glob over `key`
    pub key: Option<String>,
    pub kind: Option<String>,
    /// lower bound on `ts`, as a prefix: `2026-09` or `2026-09-20`
    pub since: Option<String>,
    pub by: Option<String>,
    /// show closed and superseded rows too
    pub all: bool,
}

impl Filter {
    /// No narrowing at all — `find` then answers with the session brief.
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.files.is_empty()
            && self.key.is_none()
            && self.kind.is_none()
            && self.since.is_none()
            && self.by.is_none()
    }
}

/// Ids of closed rows (a close row, or a compact row's `closed` field).
pub fn closed(log: &Log) -> HashSet<&str> {
    let mut h: HashSet<&str> = log
        .closes
        .iter()
        .filter_map(|c| c.reference.as_deref())
        .collect();
    h.extend(
        log.rows
            .iter()
            .filter(|r| r.extra.contains_key("closed"))
            .map(|r| r.id.as_str()),
    );
    h
}

/// Ids some newer row names in `supersedes`.
pub fn superseded(log: &Log) -> HashSet<&str> {
    log.rows
        .iter()
        .filter_map(|r| r.supersedes.as_deref())
        .collect()
}

/// Rows matching `f`, newest first by id. Closed and superseded rows are hidden unless `f.all`.
pub fn find<'a>(log: &'a Log, f: &Filter) -> Vec<&'a Row> {
    let hide: HashSet<&str> = if f.all {
        HashSet::new()
    } else {
        closed(log).union(&superseded(log)).copied().collect()
    };
    let text = f.text.as_ref().map(|t| t.to_lowercase());
    let files: Vec<String> = f
        .files
        .iter()
        .map(|q| lenient(q).trim_end_matches('/').to_string())
        .collect();
    let mut out: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| {
            !hide.contains(r.id.as_str())
                && f.kind.as_ref().is_none_or(|k| &r.kind == k)
                && f.by.as_ref().is_none_or(|b| &r.by == b)
                && f.since.as_ref().is_none_or(|s| r.ts.as_str() >= s.as_str())
                && f.key
                    .as_ref()
                    .is_none_or(|g| r.key.as_deref().is_some_and(|k| glob(g, k)))
                && text
                    .as_ref()
                    .is_none_or(|t| r.text.to_lowercase().contains(t))
                && (files.is_empty()
                    || r.files.iter().any(|rf| {
                        let rf = lenient(rf);
                        files.iter().any(|q| file_match(q, &rf))
                    }))
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// The session brief (kickoff, and `find` with no filter): open issues, then decisions, then
/// notes, then repo kinds — newest first inside each.
pub fn brief<'a>(log: &'a Log, f: &Filter) -> Vec<&'a Row> {
    let mut rows = find(log, f);
    // stable sort keeps newest-first inside each kind
    rows.sort_by_key(|r| match r.kind.as_str() {
        "issue" => 0,
        "decision" => 1,
        "note" => 2,
        _ => 3,
    });
    rows
}

/// The read/edit push: rows about `files`, ranked so the most actionable comes
/// first — exact file, then same directory, then rows sharing a key with an
/// exact hit. Open `issue` before `decision` before the rest, newest first by
/// `id` inside each. Closed and superseded rows never push. Deterministic: the
/// same log and query give the same order on any machine. The caller cuts the
/// result to the push budget with `render`.
pub fn push<'a>(log: &'a Log, files: &[String]) -> Vec<&'a Row> {
    let hide: HashSet<&str> = closed(log).union(&superseded(log)).copied().collect();
    let queries: Vec<String> = files
        .iter()
        .map(|q| lenient(q).trim_end_matches('/').to_string())
        .collect();
    if queries.is_empty() {
        return vec![];
    }
    // keys of the exact hits — tier 2 shares one of these
    let mut hit_keys: HashSet<&str> = HashSet::new();
    for r in &log.rows {
        if hide.contains(r.id.as_str()) {
            continue;
        }
        let rf: Vec<String> = r.files.iter().map(|f| lenient(f)).collect();
        if queries.iter().any(|q| rf.iter().any(|f| zone(q, f))) {
            hit_keys.extend(r.key.as_deref());
        }
    }
    let tier = |r: &Row| {
        let rf: Vec<String> = r.files.iter().map(|f| lenient(f)).collect();
        if queries.iter().any(|q| rf.iter().any(|f| zone(q, f))) {
            return 0;
        }
        if queries.iter().any(|q| rf.iter().any(|f| same_dir(q, f))) {
            return 1;
        }
        if r.key.as_deref().is_some_and(|k| hit_keys.contains(k)) {
            return 2;
        }
        3
    };
    let mut out: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| !hide.contains(r.id.as_str()) && tier(r) < 3)
        .collect();
    // newest first, then stable sort keeps it inside each rank
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out.sort_by_key(|r| {
        (
            tier(r),
            match r.kind.as_str() {
                "issue" => 0,
                "decision" => 1,
                _ => 2,
            },
        )
    });
    out
}

/// Exact or under the directory (a zone) — an anchor's ref is opaque, never a zone.
fn zone(q: &str, f: &str) -> bool {
    f == q || (anchor(q).is_none() && f.starts_with(q) && f.as_bytes().get(q.len()) == Some(&b'/'))
}

/// Same directory: both are paths (never anchors) with equal parent dirs.
fn same_dir(q: &str, f: &str) -> bool {
    if anchor(q).is_some() || anchor(f).is_some() {
        return false;
    }
    fn dir(s: &str) -> &str {
        s.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
    }
    dir(q) == dir(f)
}

/// Readers accept legacy spellings: `\` separators and a leading `./`.
fn lenient(f: &str) -> String {
    let mut s = f.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

fn file_match(q: &str, f: &str) -> bool {
    if q.contains(['*', '?', '[']) {
        return glob(q, f);
    }
    // an anchor's ref is opaque — `/` in it is not a directory
    f == q || (anchor(q).is_none() && f.starts_with(q) && f.as_bytes().get(q.len()) == Some(&b'/'))
}

/// Redis `KEYS` glob: `*` any run (including `:` and `/`), `?` one char, `[abc]` `[a-z]` `[^a]`, `\x` literal.
// ponytail: backtracking matcher, exponential on many `*` — keys are ≤ 64 chars so it never matters
pub fn glob(pattern: &str, s: &str) -> bool {
    fn m(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|i| m(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && m(&p[1..], &s[1..]),
            Some('[') if p.len() > 2 && p[2..].contains(&']') && !s.is_empty() => {
                let end = 2 + p[2..].iter().position(|&c| c == ']').unwrap();
                let (neg, set) = match p[1] {
                    '^' => (true, &p[2..end]),
                    _ => (false, &p[1..end]),
                };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= (set[i]..=set[i + 2]).contains(&s[0]);
                        i += 3;
                    } else {
                        hit |= set[i] == s[0];
                        i += 1;
                    }
                }
                hit != neg && m(&p[end + 1..], &s[1..])
            }
            Some('\\') if p.len() > 1 => !s.is_empty() && s[0] == p[1] && m(&p[2..], &s[1..]),
            Some(&c) => !s.is_empty() && s[0] == c && m(&p[1..], &s[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = s.chars().collect();
    m(&p, &s)
}

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
        (brief(log, f), cfg.kickoff_tokens)
    } else {
        (find(log, f), cfg.find_tokens)
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
