//! fael-core — the log format (docs/format.md): row v1, validate, read, append under lock.
//! Knows the row format, never a client. The CLI, MCP and hooks sit on top of this.

mod id;
mod log;

pub use id::{now_ms, rfc3339, ulid, ulid_at, writer_id};
pub use log::{Log, MONTH_MAX, add, append, close, parse, read};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

/// Per-repo settings from `.fael/config.toml` (loaded by the CLI; every field has a default).
#[derive(Debug, Clone)]
pub struct Config {
    /// Extra kinds this repo declares on top of `CORE_KINDS`.
    pub kinds: Vec<String>,
    /// Row size limit in bytes — clamped to `ROW_BYTES_MAX`.
    pub row_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            kinds: vec![],
            row_bytes: ROW_BYTES_MAX,
        }
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
