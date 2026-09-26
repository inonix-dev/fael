//! find · brief · keys · render — what the CLI, MCP and hooks show, built on `read()`'s `Log`.
//! Deterministic: newest first by `id` (never `ts` — clocks differ across machines).
//!
//! Thin entry only — the filter type lives here, the verbs in `query/`:
//! `select` (find/brief/kickoff/push/gone over row sets), `matching` (path and
//! glob primitives), `render` (token-budgeted markdown), `lookup` (resolve,
//! keys, query, warnings).

mod lookup;
mod matching;
mod render;
mod select;

pub use lookup::{KeyUse, keys, query, resolve, warnings};
pub use matching::glob;
pub use render::{Cut, abbrev, est_tokens, render, render_full, render_full_page, render_page};
pub use select::{
    Urgent, UrgentChange, brief, closed, cmp_rows, find, fresh_ts, gone, kickoff, page, push,
    ranked, resolve_urgent, superseded,
};

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
    /// who the row routes to (`issue --to <who>`) — lowercased, matched like
    /// session start (`to_matches`): a full writer id or its name part, either way
    pub to: Option<String>,
    /// show closed and superseded rows too
    pub all: bool,
    /// Postgres-style paging, applied after ranking before render
    /// (`page()`): at most this many rows …
    pub limit: Option<usize>,
    /// … skipping this many ranked rows first
    pub offset: usize,
}

impl Filter {
    /// No narrowing at all — `find` then answers with the session brief.
    /// Paging is not narrowing: `find --limit 2` still briefs, just shorter.
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.files.is_empty()
            && self.key.is_none()
            && self.kind.is_none()
            && self.since.is_none()
            && self.by.is_none()
            && self.to.is_none()
    }
}
