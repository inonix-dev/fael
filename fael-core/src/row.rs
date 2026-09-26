use crate::{now_ms, rfc3339, ulid_at};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    /// Who has to answer (PLAN-fael-direction chunk 2): an `issue --to <who>`
    /// routes a question to who must answer. Optional, stored lowercase.
    /// A top-level field (not `extra`) so select/render read it without
    /// parsing — old readers keep it in `extra` and stay compatible, no `v` bump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Row {
    /// Who this row routes to: the `to` field, falling back to a
    /// hand-written `to` in `extra` (forward-compat read).
    pub fn to_who(&self) -> Option<&str> {
        self.to
            .as_deref()
            .or_else(|| self.extra.get("to").and_then(|v| v.as_str()))
    }

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

    /// A fresh v1 alias row recording `from → to` (`fael mv`). Carries no
    /// kind and no files — it only says where a path lives now. Readers that
    /// don't know `moved` skip the row; `text` is human-readable and ignored.
    pub fn moved(by: &str, from: &str, to: &str) -> Row {
        let ms = now_ms();
        Row {
            v: Some(1),
            id: ulid_at(ms),
            ts: rfc3339(ms),
            by: by.into(),
            text: format!("{from} → {to}"),
            extra: Map::from_iter([(
                "moved".to_string(),
                serde_json::json!({"from": from, "to": to}),
            )]),
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
    pub(crate) fn apply(&self, row: &mut Row) {
        if let Some(b) = &self.branch {
            row.extra.insert("branch".into(), b.clone().into());
        }
        if let Some(s) = &self.sha {
            row.extra.insert("sha".into(), s.clone().into());
        }
    }
}
