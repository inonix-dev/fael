use crate::{CORE_KINDS, Config, KEY_MAX, ROW_BYTES_MAX, Row, anchor};
use std::path::Path;

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
    if let Some(t) = row.to_who()
        && t.trim().is_empty()
    {
        return Err("rejected: to is empty — name who has to answer, e.g. --to ploy".into());
    }
    if row.kind != "issue" && row.urgent_value().is_some() {
        return Err(
            "rejected: urgent is for issues — file it as kind issue or drop --urgent".into(),
        );
    }
    if row.urgent.is_some_and(|u| !u.is_finite()) {
        return Err("rejected: urgent must be a finite number".into());
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

/// Check an alias (`moved`) row before it is written — what `fael mv` appends.
pub fn validate_alias(row: &Row, cfg: &Config) -> Result<(), String> {
    if !row.kind.is_empty() {
        return Err(
            "rejected: an alias row carries no kind — it only says where a path moved".into(),
        );
    }
    if !row.files.is_empty() {
        return Err(
            "rejected: an alias row carries no files — it only says where a path moved".into(),
        );
    }
    if row.reference.is_some() {
        return Err(
            "rejected: an alias row closes nothing — it only says where a path moved".into(),
        );
    }
    let pair = row
        .extra
        .get("moved")
        .and_then(|m| m.as_object())
        .and_then(|m| Some((m.get("from")?.as_str()?, m.get("to")?.as_str()?)));
    let Some((from, to)) = pair else {
        return Err(
            "rejected: alias needs moved.from and moved.to strings — write it with `fael mv <old> <new>`"
                .into(),
        );
    };
    if from.trim().is_empty() || to.trim().is_empty() {
        return Err("rejected: alias from and to must both be named".into());
    }
    if from == to {
        return Err(format!(
            "rejected: {from:?} is already itself — `fael mv` needs two different paths"
        ));
    }
    for f in [from, to] {
        if !canonical(f) {
            return Err(format!(
                "rejected: alias entry {f:?} is not repo-relative — write it like src/auth.rs (no ./, .., absolute path or \\); `fael mv` normalises this for you"
            ));
        }
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

fn absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
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
