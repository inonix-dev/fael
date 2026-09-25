//! The stop decision, written once for every adapter: block the session that
//! edited files or made commits (or announced a bug) without recording a mem row.
//! Edits are the primary signal — many agents are told never to commit.
//! Pure — git and transcript reads live in the adapter (`fael/src/hook.rs`).

use crate::Log;
use crate::id::ts_ms;

/// What the adapter learned about this turn.
pub struct StopFacts {
    /// The client already fired this hook once — letting through avoids a loop.
    pub stop_active: bool,
    /// Repo-relative files the edit hook saw this session, first-seen order.
    pub edits: Vec<String>,
    /// `git log --format=%h %s` since the session start — only filled when
    /// `edits` is empty (fallback for edits made outside the edit hook).
    pub commits: Vec<String>,
    /// Any add or close row stamped at or after the session start.
    pub new_row: bool,
    /// The repo has a `.fael/log` at all — a repo that never adopted fael
    /// fails open instead of blocking every turn.
    pub has_log: bool,
    /// A bug-announcement phrase from the transcript tail, if any.
    pub bug_signal: Option<String>,
    /// An `issue` row exists at or after the session start.
    pub bug_row_since: bool,
}

/// The block reason, or `None` to let the turn end. Never fails — every
/// unknown is an allow, because a memory tool must not break the session.
pub fn decide_stop(f: &StopFacts) -> Option<String> {
    if f.stop_active {
        return None;
    }
    // Bug rule first: independent of commits (a reported bug with no row is
    // lost when the room closes, whatever else the turn did).
    if let Some(marker) = &f.bug_signal
        && !f.bug_row_since
    {
        return Some(
            format!(
                "This turn reported a bug (\"{marker}\") but no issue row exists for this session.\n\
                 Record it before ending: fael add issue \"<what is broken>\" --files <files>\n\
                 Already filed, or not a bug? End the turn again — this fires once per session."
            ),
        );
    }
    // Work rule: files edited (or, failing that, commits) with nothing recorded.
    if (f.edits.is_empty() && f.commits.is_empty()) || !f.has_log || f.new_row {
        return None;
    }
    // a markdown checklist the agent can act on as-is, --files prefilled
    let (what, items, files) = if f.edits.is_empty() {
        (format!("{} commit(s)", f.commits.len()), &f.commits, "<files>".to_string())
    } else {
        (format!("{} file(s) edited", f.edits.len()), &f.edits, f.edits.join(","))
    };
    let mut out = vec![format!("{what} this session with no mem row for this work:")];
    out.extend(items.iter().take(10).map(|c| format!("- [ ] {c}")));
    if items.len() > 10 {
        out.push(format!("- … +{} more", items.len() - 10));
    }
    out.push(format!(
        "Record one before ending: fael add <decision|issue|note> \"<what happened>\" --files {files}"
    ));
    out.push("Nothing worth recording? End the turn again — this fires once per session.".into());
    Some(out.join("\n"))
}

/// True when any add or close row is stamped at or after `since_ms`. Numeric
/// (ms) on both sides — a whole-second string compare reads a row filed just
/// before the session start as newer whenever they share a second.
pub fn has_new_row(log: &Log, since_ms: i64) -> bool {
    log.rows
        .iter()
        .chain(log.closes.iter())
        .filter_map(|r| ts_ms(&r.ts))
        .any(|ms| ms >= since_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Row;

    fn facts() -> StopFacts {
        StopFacts {
            stop_active: false,
            edits: vec![],
            commits: vec!["abc123 fix login".into()],
            new_row: false,
            has_log: true,
            bug_signal: None,
            bug_row_since: false,
        }
    }

    #[test]
    fn allows_everything_unknown() {
        assert!(decide_stop(&StopFacts { stop_active: true, ..facts() }).is_none());
        assert!(
            decide_stop(&StopFacts {
                commits: vec![],
                ..facts()
            })
            .is_none()
        );
        assert!(
            decide_stop(&StopFacts {
                has_log: false,
                ..facts()
            })
            .is_none()
        );
        assert!(
            decide_stop(&StopFacts {
                new_row: true,
                ..facts()
            })
            .is_none()
        );
    }

    #[test]
    fn blocks_commit_without_row() {
        let r = decide_stop(&facts()).unwrap();
        assert!(r.contains("1 commit(s)"), "{r}");
        assert!(r.contains("fael add <decision|issue|note>"), "{r}");
    }

    #[test]
    fn edits_win_over_commits_and_prefill_files() {
        let r = decide_stop(&StopFacts {
            edits: vec!["src/a.rs".into(), "src/b.rs".into()],
            ..facts()
        })
        .unwrap();
        assert!(r.contains("2 file(s) edited") && r.contains("- [ ] src/a.rs"), "{r}");
        assert!(r.contains("--files src/a.rs,src/b.rs") && !r.contains("abc123"), "{r}");
        assert!(
            decide_stop(&StopFacts { edits: vec!["a".into()], commits: vec![], new_row: true, ..facts() })
                .is_none()
        );
    }

    #[test]
    fn bug_rule_needs_no_commit() {
        let r = decide_stop(&StopFacts {
            commits: vec![],
            bug_signal: Some("found a bug".into()),
            ..facts()
        })
        .unwrap();
        assert!(r.contains("fael add issue"), "{r}");
        assert!(
            decide_stop(&StopFacts {
                commits: vec![],
                bug_signal: Some("found a bug".into()),
                bug_row_since: true,
                ..facts()
            })
            .is_none()
        );
    }

    #[test]
    fn new_row_compares_ms_not_seconds() {
        let mut log = Log::default();
        let mut r = Row::new("t-0000", "note", "x", vec!["a.rs".into()]);
        r.ts = "2026-09-25T10:00:01Z".into();
        log.rows.push(r);
        assert!(!has_new_row(&log, ts_ms("2026-09-25T10:00:01.500Z").unwrap()));
        assert!(has_new_row(&log, ts_ms("2026-09-25T10:00:01Z").unwrap()));
    }
}
