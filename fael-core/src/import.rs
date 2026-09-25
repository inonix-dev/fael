//! `fael import` — merge another log in, without dropping a row (SPEC §6).
//!
//! The source is a `.fael/log` dir, a repo root (its `.fael/log`), or a
//! fapony `.memory` dir — detected by shape, not by name. Output lands in
//! immutable `log/_import/<ULID>.jsonl`, with a `.close.jsonl` companion when
//! closes name no row here. Dedupe by id makes importing twice safe.
//!
//! Fapony legacy mapping: `agent`→`by` · `bug`→`issue` · `close` rows fold
//! into their target's `closed` (or ride the companion file when the target
//! isn't here) · cut kinds → `note` with the original in `legacy_kind` ·
//! missing id → `legacy-<sha1 8 hex of the line>` · no `files` but a
//! path-shaped `spec` (the plan the row was filed against) → `files = [spec]`,
//! so kickoff on that plan finds it (rows with empty text excepted). Everything else — `spec` itself, `key`,
//! unknown fields — round-trips through `Row::extra` untouched.

use crate::compact::fold;
use crate::log::{collect_files, dedupe_ids, is_marker, lock, tmp_rename};
use crate::{CORE_KINDS, Row, anchor, ulid};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Opts {
    /// `(old_prefix, new_prefix)` path rewrites for `files` (moving into a
    /// monorepo subfolder); first match wins, anchors never match.
    pub maps: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub adds: usize,
    pub folded: usize,
    pub carried: usize,
    pub skipped: usize,
    pub paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Merge `src` into this repo's `.fael/log/_import/`. `allowed` is the repo's
/// extra kinds (`Config::kinds`) — a legacy kind outside core + allowed
/// becomes a `note` keeping the original in `legacy_kind`.
pub fn import(fael: &Path, src: &Path, allowed: &[String], opts: &Opts) -> Result<Report, String> {
    let _guard = lock(fael)?;
    let files = sources(src)?;
    let (mut adds, mut closes) = (vec![], vec![]);
    let mut skipped = 0usize;
    let mut warnings = vec![];
    for f in &files {
        let name = f.to_string_lossy().into_owned();
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", name))?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut lines: Vec<&str> = text.split('\n').collect();
        lines.pop(); // the torn tail belongs to a live writer, not to us
        let is_close_file = name.ends_with(".close.jsonl");
        for (i, line) in lines.iter().enumerate() {
            let line = line.trim();
            if line.is_empty() || is_marker(line) {
                continue;
            }
            match convert(line, is_close_file, allowed, opts) {
                Ok(Converted::Add(r)) => adds.push(r),
                Ok(Converted::Close(r)) => closes.push(r),
                Err(w) => {
                    skipped += 1;
                    if warnings.len() < 5 {
                        warnings.push(format!("{name}:{}: skipped ({w})", i + 1));
                    }
                }
            }
        }
    }
    if adds.is_empty() && closes.is_empty() {
        return Err(format!(
            "{}: no rows to import ({} line(s) skipped{})",
            src.display(),
            skipped,
            warnings
                .first()
                .map(|w| format!(" — first: {w}"))
                .unwrap_or_default()
        ));
    }
    dedupe_ids(&mut adds);
    dedupe_ids(&mut closes);
    let folded = fold(&mut adds, &closes);
    let resolved: std::collections::HashSet<&str> =
        folded.iter().map(|(_, c)| c.id.as_str()).collect();
    let carried: Vec<Row> = closes
        .into_iter()
        .filter(|c| !resolved.contains(c.id.as_str()))
        .collect();

    let dir = fael.join("log").join("_import");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = ulid();
    let mut paths = vec![];
    let mut body = String::new();
    for r in &adds {
        body.push_str(&r.to_line());
        body.push('\n');
    }
    let out = dir.join(format!("{stamp}.jsonl"));
    tmp_rename(&out, body.as_bytes())?;
    paths.push(out);
    let carried_n = carried.len();
    if !carried.is_empty() {
        let mut cbody = String::new();
        for c in &carried {
            cbody.push_str(&c.to_line());
            cbody.push('\n');
        }
        let cout = dir.join(format!("{stamp}.close.jsonl"));
        tmp_rename(&cout, cbody.as_bytes())?;
        paths.push(cout);
    }
    Ok(Report {
        adds: adds.len(),
        folded: folded.len(),
        carried: carried_n,
        skipped,
        paths,
        warnings,
    })
}

/// `src` is a file (one log), a `.fael/log` dir, a repo root, or a legacy
/// dir — whichever holds jsonl wins, in that order.
fn sources(src: &Path) -> Result<Vec<PathBuf>, String> {
    if src.is_file() {
        return Ok(vec![src.to_path_buf()]);
    }
    if !src.is_dir() {
        return Err(format!(
            "rejected: {} is not a file or directory",
            src.display()
        ));
    }
    let nested = src.join(".fael/log");
    let dir = if nested.is_dir() {
        nested
    } else {
        src.to_path_buf()
    };
    let files: Vec<PathBuf> = collect_files(&dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    if files.is_empty() {
        return Err(format!("rejected: no .jsonl under {}", src.display()));
    }
    Ok(files)
}

enum Converted {
    Add(Row),
    Close(Row),
}

/// One source line → a fael row. Fapony-shaped lines (an `agent` field, no
/// `by`) go through the legacy mapping; fael-shaped lines pass through with
/// `--map` applied. Anything unparseable is an `Err` the caller counts.
fn convert(
    line: &str,
    is_close_file: bool,
    allowed: &[String],
    opts: &Opts,
) -> Result<Converted, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let Value::Object(mut m) = v else {
        return Err("not a JSON object".into());
    };
    // `by` is required in fael; early fapony rows lack `agent` too
    if !m.contains_key("by") {
        return legacy(m, line, allowed, opts);
    }
    if m.get("id")
        .and_then(Value::as_str)
        .is_none_or(|s| s.is_empty())
    {
        m.insert("id".into(), Value::String(legacy_id(line)));
    }
    let mut row: Row = serde_json::from_value(Value::Object(m)).map_err(|e| e.to_string())?;
    apply_maps(&mut row.files, opts);
    if is_close_file
        || row
            .reference
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    {
        Ok(Converted::Close(row))
    } else {
        Ok(Converted::Add(row))
    }
}

/// A fapony row → a fael row, per the mapping in the module docs.
fn legacy(
    mut m: Map<String, Value>,
    line: &str,
    allowed: &[String],
    opts: &Opts,
) -> Result<Converted, String> {
    // some fapony closes name their target in `id`, not `ref`
    let no_ref = m
        .get("ref")
        .and_then(Value::as_str)
        .is_none_or(|r| r.trim().is_empty());
    if m.get("kind").and_then(Value::as_str) == Some("close")
        && no_ref
        && let Some(target) = m.remove("id")
    {
        m.insert("ref".into(), target);
    }
    let id = m
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| legacy_id(line));
    let by = m
        .get("agent")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "legacy".into());
    let kind = m
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("note")
        .to_string();
    m.remove("agent");
    m.insert("v".into(), Value::from(1));
    m.insert("id".into(), Value::String(id));
    m.insert("by".into(), Value::String(by));
    if kind == "close" {
        m.remove("kind");
        m.insert("legacy_kind".into(), Value::String(kind));
        let mut row: Row = serde_json::from_value(Value::Object(m)).map_err(|e| e.to_string())?;
        apply_maps(&mut row.files, opts);
        return Ok(Converted::Close(row));
    }
    let mapped = if kind == "bug" {
        "issue".to_string()
    } else if CORE_KINDS.contains(&kind.as_str()) || allowed.iter().any(|k| k == &kind) {
        kind.clone()
    } else {
        m.insert("legacy_kind".into(), Value::String(kind));
        "note".to_string()
    };
    m.insert("kind".into(), Value::String(mapped));
    // ponytail: "path-shaped" = one token, no whitespace; free-text specs stay extra-only.
    // Empty-text rows (fapony `synced` bookkeeping) stay unanchored so kickoff isn't flooded.
    let no_files = m
        .get("files")
        .and_then(Value::as_array)
        .is_none_or(|a| a.is_empty())
        && m.get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.trim().is_empty());
    let spec = m
        .get("spec")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .map(str::to_string);
    if let (true, Some(spec)) = (no_files, spec) {
        m.insert("files".into(), Value::from(vec![spec]));
    }
    let mut row: Row = serde_json::from_value(Value::Object(m)).map_err(|e| e.to_string())?;
    apply_maps(&mut row.files, opts);
    Ok(Converted::Add(row))
}

/// `legacy-<first 8 hex of sha1(line)>` — deterministic across machines, so
/// two hosts importing the same legacy log dedupe to the same rows.
fn legacy_id(line: &str) -> String {
    let mut h = Sha1::new();
    h.update(line.trim().as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("legacy-{}", &hex[..8])
}

/// Rewrite path prefixes in `files`; anchors are opaque and never match.
fn apply_maps(files: &mut [String], opts: &Opts) {
    if opts.maps.is_empty() {
        return;
    }
    for f in files.iter_mut() {
        if anchor(f).is_some() {
            continue;
        }
        if let Some((old, new)) = opts.maps.iter().find(|(old, _)| f.starts_with(old)) {
            *f = format!("{new}{}", &f[old.len()..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::legacy_id;

    #[test]
    fn legacy_id_is_sha1() {
        // FIPS 180-1 vector: sha1("abc") = a9993e36…
        assert_eq!(legacy_id("abc"), "legacy-a9993e36");
        assert_eq!(legacy_id("  abc  "), "legacy-a9993e36");
    }
}
