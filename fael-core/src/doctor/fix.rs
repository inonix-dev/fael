use super::{Kind, Report};
use crate::log::{is_marker, lock, tmp_rename};
use crate::{Row, now_ms, rfc3339, ulid};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Repair every fixable problem in `report`, under the `.fael/.lock`.
/// Returns one line per action taken. Report-only problems are left alone.
pub fn fix(fael: &Path, root: &Path, report: &Report) -> Result<Vec<String>, String> {
    let _guard = lock(fael)?;

    let mut done = vec![];
    // group content repairs per file so each file is rewritten once
    let mut files: HashSet<PathBuf> = HashSet::new();
    for p in &report.problems {
        match p.kind {
            Kind::Broken | Kind::Torn | Kind::Conflict | Kind::Encoding => {
                if let Some(f) = p.file.clone().filter(|f| f.is_file()) {
                    files.insert(f);
                }
            }
            Kind::Union => {
                let attrs = root.join(".gitattributes");
                let mut s = std::fs::read_to_string(&attrs).unwrap_or_default();
                if !s.lines().any(|l| l.contains("merge=union")) {
                    if !s.is_empty() && !s.ends_with('\n') {
                        s.push('\n');
                    }
                    s.push_str(".fael/log/**/*.jsonl merge=union\n");
                    tmp_rename(&attrs, s.as_bytes())?;
                    done.push(format!("{}: added the merge=union line", attrs.display()));
                }
            }
            _ => {}
        }
    }
    let mut files: Vec<PathBuf> = files.into_iter().collect();
    files.sort();
    for f in files {
        if let Some(action) = repair_file(fael, &f)? {
            done.push(action);
        }
    }
    Ok(done)
}

/// Rewrite one log file without its broken lines and marker lines, with
/// encoding normalised; the removed bytes move to `quarantine/` (the torn
/// tail too). Returns the action line, or None when nothing changed.
fn repair_file(fael: &Path, path: &Path) -> Result<Option<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(&text)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines: Vec<&str> = text.split('\n').collect();
    let tail = lines.pop().unwrap_or("");
    // re-derive what's broken/markers post-normalisation (same rules as scan)
    let mut broken: HashSet<usize> = HashSet::new();
    let mut markers = 0usize;
    let mut kept = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || is_marker(t) {
            if is_marker(t) {
                markers += 1;
            }
            continue;
        }
        if serde_json::from_str::<Row>(line).is_err() {
            broken.insert(i + 1);
        } else {
            kept += 1;
        }
    }
    if broken.is_empty() && markers == 0 && tail.trim().is_empty() && is_clean(&bytes) {
        return Ok(None);
    }
    let mut out = String::new();
    let mut moved = vec![];
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || is_marker(t) {
            continue;
        }
        if broken.contains(&(i + 1)) {
            moved.push(line.to_string());
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !tail.trim().is_empty() {
        moved.push(tail.trim().to_string());
    }
    if !moved.is_empty() {
        let q = quarantine(fael, path)?;
        let mut body = moved.join("\n");
        body.push('\n');
        std::fs::write(&q, body).map_err(|e| format!("{}: {e}", q.display()))?;
    }
    tmp_rename(path, out.as_bytes())?;
    Ok(Some(format!(
        "{}: {} line(s) to quarantine, {} marker(s) stripped, {} row(s) kept",
        path.display(),
        moved.len(),
        markers,
        kept
    )))
}

fn is_clean(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"\xef\xbb\xbf")
        && !bytes.windows(2).any(|w| w == b"\r\n")
        && String::from_utf8(bytes.to_vec()).is_ok()
}

/// `.fael/quarantine/<log.path.with.dots>.<ULID>.jsonl` — removed bytes are
/// never deleted, and quarantine never lives under `log/` so reads ignore it.
fn quarantine(fael: &Path, path: &Path) -> Result<PathBuf, String> {
    let dir = fael.join("quarantine");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let rel = path
        .strip_prefix(fael)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(['/', '\\'], ".");
    let q = dir.join(format!("{rel}.{}.jsonl", ulid()));
    Ok(q)
}

/// The current UTC month — what the CLI passes as `scan`'s `month`.
pub fn current_month() -> String {
    rfc3339(now_ms())[..7].to_string()
}
