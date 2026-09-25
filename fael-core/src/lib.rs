//! fael-core — the log format (docs/format.md): row v1, validate, read, append under lock.
//! Knows the row format, never a client. The CLI, MCP and hooks sit on top of this.

mod compact;
mod doctor;
mod hook;
mod id;
mod import;
mod log;
mod query;

pub use compact::{Opts as CompactOpts, Report as CompactReport, WriterReport, compact};
pub use import::{Opts as ImportOpts, Report as ImportReport, import};

pub use doctor::{
    Kind as ProblemKind, Problem, Report as DoctorReport, Severity, current_month,
    fix as doctor_fix, scan as doctor_scan,
};

pub use hook::{StopFacts, decide_stop, last_row_ms};

pub use id::{now_ms, rfc3339, ts_ms, ulid, ulid_at, writer_id};
pub use log::{Log, MONTH_MAX, add, add_row, append, close, close_row, parse, read};
pub use query::{
    Filter, KeyUse, abbrev, brief, closed, est_tokens, find, glob, keys, push, query, render,
    resolve, superseded, warnings,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

/// Core kinds with fixed meaning; a repo adds more through `Config::kinds`.
pub const CORE_KINDS: [&str; 3] = ["decision", "issue", "note"];
/// Hard cap on one serialised row, in bytes (Thai is 3 bytes/char).
pub const ROW_BYTES_MAX: usize = 10 * 1024;
pub const KEY_MAX: usize = 64;

/// One line of a log file — an add row, or a close row (`ref` set, no kind/files).
/// Reading is lenient: every field defaults, and fields fael doesn't know are kept in `extra`
/// and written back untouched (forward-compat, format.md §Readers).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u64>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Row {
    /// A fresh v1 add row stamped with a ULID and the current UTC time.
    pub fn new(by: &str, kind: &str, text: &str, files: Vec<String>) -> Row {
        let ms = now_ms();
        Row {
            v: Some(1),
            id: ulid_at(ms),
            ts: rfc3339(ms),
            by: by.into(),
            kind: kind.into(),
            text: text.into(),
            files,
            ..Row::default()
        }
    }

    /// A fresh v1 close row pointing at `reference`.
    pub fn close(by: &str, reference: &str, text: &str) -> Row {
        let ms = now_ms();
        Row {
            v: Some(1),
            id: ulid_at(ms),
            ts: rfc3339(ms),
            by: by.into(),
            text: text.into(),
            reference: Some(reference.into()),
            ..Row::default()
        }
    }

    /// The row as one JSON line, without the trailing `\n`.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("Row always serialises")
    }
}

/// Who wrote a row and where the tree stood — the adapter fills it: git on a dev box,
/// the signed-in user (no branch/sha) on a server. Core never asks git itself.
#[derive(Debug, Clone, Default)]
pub struct Stamp {
    pub by: String,
    pub branch: Option<String>,
    pub sha: Option<String>,
}

impl Stamp {
    fn apply(&self, row: &mut Row) {
        if let Some(b) = &self.branch {
            row.extra.insert("branch".into(), b.clone().into());
        }
        if let Some(s) = &self.sha {
            row.extra.insert("sha".into(), s.clone().into());
        }
    }
}

/// Per-repo settings from `.fael/config.toml` (every field has a default; see `Config::from_toml`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Extra kinds this repo declares on top of `CORE_KINDS`.
    pub kinds: Vec<String>,
    /// Row size limit in bytes — clamped to `ROW_BYTES_MAX`.
    pub row_bytes: usize,
    /// Allowed first segments of `key`; empty = any. Outside it is a warning, never a reject.
    pub key_domains: Vec<String>,
    /// Token budget for the session brief (`kickoff`, `find` with no filter).
    pub kickoff_tokens: usize,
    /// Token budget for `find` output.
    pub find_tokens: usize,
    /// Token budget for the read/edit hook push.
    pub push_tokens: usize,
    /// Warn when a row's text is estimated over this many tokens.
    pub warn_row_tokens: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            kinds: vec![],
            row_bytes: ROW_BYTES_MAX,
            key_domains: vec![],
            kickoff_tokens: 800,
            find_tokens: 800,
            push_tokens: 800,
            warn_row_tokens: 400,
        }
    }
}

impl Config {
    /// Parse `.fael/config.toml` text — every field optional. The caller reads the bytes
    /// from wherever the repo lives; a missing file is `Config::default()`, not this.
    pub fn from_toml(s: &str) -> Result<Config, String> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct File {
            kinds: Vec<String>,
            key_domains: Vec<String>,
            budget: Budget,
            warn: Warn,
            limit: Limit,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Budget {
            kickoff_tokens: Option<usize>,
            find_tokens: Option<usize>,
            push_tokens: Option<usize>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Warn {
            row_tokens: Option<usize>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Limit {
            row_bytes: Option<usize>,
        }
        let f: File = toml::from_str(s).map_err(|e| e.to_string())?;
        let d = Config::default();
        Ok(Config {
            kinds: f.kinds,
            key_domains: f.key_domains,
            row_bytes: f.limit.row_bytes.unwrap_or(d.row_bytes),
            kickoff_tokens: f.budget.kickoff_tokens.unwrap_or(d.kickoff_tokens),
            find_tokens: f.budget.find_tokens.unwrap_or(d.find_tokens),
            push_tokens: f.budget.push_tokens.unwrap_or(d.push_tokens),
            warn_row_tokens: f.warn.row_tokens.unwrap_or(d.warn_row_tokens),
        })
    }
}

/// Check an add row before it is written. `Err` is the message shown to the agent:
/// it says what is wrong and how to fix the call.
pub fn validate(row: &Row, cfg: &Config) -> Result<(), String> {
    if !CORE_KINDS.contains(&row.kind.as_str()) && !cfg.kinds.contains(&row.kind) {
        let mut all: Vec<&str> = CORE_KINDS.to_vec();
        all.extend(cfg.kinds.iter().map(String::as_str));
        return Err(format!(
            "rejected: kind must be one of {} (+ config.kinds) — got {:?}",
            all.join("|"),
            row.kind
        ));
    }
    if row.files.is_empty() || row.files.iter().any(|f| f.trim().is_empty()) {
        return Err("rejected: files is required — name the file(s) this is about".into());
    }
    if let Some(f) = row.files.iter().find(|f| !canonical(f)) {
        return Err(format!(
            "rejected: files entry {f:?} is not repo-relative — write it like src/auth.rs (no ./, .., absolute path or \\); run it through normalize_files first"
        ));
    }
    if let Some(k) = &row.key {
        valid_key(k)?;
    }
    check_common(row, cfg)
}

/// Check a close row before it is written.
pub fn validate_close(row: &Row, cfg: &Config) -> Result<(), String> {
    if row.reference.as_deref().is_none_or(|r| r.trim().is_empty()) {
        return Err("rejected: close needs the id of the row it closes".into());
    }
    check_common(row, cfg)
}

fn check_common(row: &Row, cfg: &Config) -> Result<(), String> {
    if row.text.trim().is_empty() {
        return Err("rejected: text is required — write it so it stands alone months later".into());
    }
    let line = row.to_line();
    let limit = cfg.row_bytes.min(ROW_BYTES_MAX);
    if line.len() > limit {
        return Err(format!(
            "rejected: row is {} bytes, limit is {limit} — trim the text or split it into rows",
            line.len()
        ));
    }
    if let Some(what) = secret(&line) {
        return Err(format!(
            "rejected: looks like a secret ({what}) — remove it, the log lives in git"
        ));
    }
    Ok(())
}

/// `scheme:ref` anchor (`doc:pricing`, `issue:#12`): a scheme of ≥ 2 chars `[a-z0-9+.-]`, starting
/// with a letter, before the first `:` and before any `/`. Two chars minimum so `C:` stays a drive.
/// Returns the ref — opaque to fael (`/` in it is not a path separator); it must be non-empty.
// ponytail: a root-level file named like `notes:v2.md` reads as an anchor — rare, rename the file
pub(crate) fn anchor(f: &str) -> Option<&str> {
    f.split_once(':')
        .filter(|(s, _)| {
            s.len() >= 2
                && s.as_bytes()[0].is_ascii_lowercase()
                && s.bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'+' | b'.' | b'-'))
        })
        .map(|(_, r)| r)
}

fn absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
}

/// The canonical form `validate` accepts: an anchor, or `/`-separated segments with no
/// empty, `.` or `..` segment, no `\\` and no leading `/` or drive.
fn canonical(f: &str) -> bool {
    anchor(f).map_or_else(
        || {
            !f.contains('\\')
                && !absolute(f)
                && f.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
        },
        |r| !r.trim().is_empty(),
    )
}

/// Turn what a client sent into repo-relative paths — the one normalisation every adapter uses
/// before `add`, so CLI, MCP and hooks can't drift apart. `cwd` and `root` are absolute;
/// a relative entry is read from `cwd`. Anchors pass through. Outside the repo = `Err`.
// ponytail: lexical only — a symlinked root (/tmp vs /private/tmp) must be passed already resolved
pub fn normalize_files(files: &[String], cwd: &Path, root: &Path) -> Result<Vec<String>, String> {
    let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let root_s = slash(root);
    let root_segs: Vec<&str> = root_s.split('/').filter(|s| !s.is_empty()).collect();
    let cwd_s = slash(cwd);
    files
        .iter()
        .map(|f| {
            let f = f.trim();
            if let Some(r) = anchor(f) {
                return if r.trim().is_empty() {
                    Err(format!(
                        "rejected: anchor {f:?} has no ref — write scheme:ref, e.g. doc:pricing"
                    ))
                } else {
                    Ok(f.to_string())
                };
            }
            let p = f.replace('\\', "/");
            let full = if absolute(&p) {
                p
            } else {
                format!("{cwd_s}/{p}")
            };
            let mut segs: Vec<&str> = vec![];
            for s in full.split('/') {
                match s {
                    "" | "." => {}
                    ".." => {
                        segs.pop();
                    }
                    s => segs.push(s),
                }
            }
            match segs.strip_prefix(root_segs.as_slice()) {
                Some(rel) if !rel.is_empty() => Ok(rel.join("/")),
                _ => Err(format!(
                    "rejected: {f:?} is outside the repo ({root_s}) — name a file inside it"
                )),
            }
        })
        .collect()
}

/// Redis-style key: `:`-separated segments of `[a-z0-9._-]+`, ≤ 64 chars, lowercase only.
pub fn valid_key(k: &str) -> Result<(), String> {
    let ok = k.len() <= KEY_MAX
        && k.split(':').all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
        });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "rejected: key {k:?} — use lowercase segments [a-z0-9._-] joined by ':' (e.g. auth:session), ≤ {KEY_MAX} chars"
        ))
    }
}

/// Name of the first secret-looking token in `s`.
// ponytail: fixed prefix list, catches the common pasted tokens only; swap for a real scanner if one slips through
fn secret(s: &str) -> Option<&'static str> {
    const PREFIXES: [(&str, &str); 8] = [
        ("-----BEGIN", "private key block"),
        ("AKIA", "AWS access key"),
        ("ghp_", "GitHub token"),
        ("github_pat_", "GitHub token"),
        ("sk-ant-", "Anthropic key"),
        ("sk-proj-", "OpenAI key"),
        ("xoxb-", "Slack token"),
        ("xoxp-", "Slack token"),
    ];
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .find_map(|tok| {
            PREFIXES.iter().find(|(p, _)| {
                // a bare prefix word ("AKIA", "sk-") is prose, a long token after it is a secret
                tok.starts_with(p) && (tok.len() >= p.len() + 16 || *p == "-----BEGIN")
            })
        })
        .map(|(_, what)| *what)
}
