//! Reading and appending `.fael/log/**` (format.md §Layout, §Writers, §Readers).
//! Reads never fail and take no lock; appends hold `.fael/.lock` and write one whole line.

use crate::{Config, Row, Stamp, closed, resolve, validate, validate_close, warnings};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// A month file this big refuses appends — run `fael compact` (well under GitHub's 50 MiB warning is the point).
pub const MONTH_MAX: u64 = 50 * 1024 * 1024;

/// Everything under `.fael/log/`, deduped by id (first by file order wins).
#[derive(Debug, Default)]
pub struct Log {
    pub rows: Vec<Row>,
    pub closes: Vec<Row>,
    /// `file:line: what` for every line that was skipped — never fatal.
    pub warnings: Vec<String>,
}

/// Read every log file under `<fael>/log/`. Missing dir = empty log. Never errors.
pub fn read(fael: &Path) -> Log {
    let mut files = vec![];
    walk(&fael.join("log"), &mut files);
    files.sort();
    let mut log = Log::default();
    for f in &files {
        let name = f.to_string_lossy();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(bytes) = fs::read(f) else {
            log.warnings.push(format!("{name}: unreadable — skipped"));
            continue;
        };
        let out = if name.ends_with(".close.jsonl") {
            &mut log.closes
        } else {
            &mut log.rows
        };
        parse(&bytes, &name, out, &mut log.warnings);
    }
    dedupe(&mut log.rows);
    dedupe(&mut log.closes);
    log
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out)
        } else {
            out.push(p)
        }
    }
}

/// Parse one file's bytes into rows. BOM, CRLF and invalid UTF-8 are normalised in memory;
/// merge-conflict markers are skipped (both sides' rows kept); a torn last line (no `\n`) is ignored.
pub fn parse(bytes: &[u8], file: &str, out: &mut Vec<Row>, warnings: &mut Vec<String>) {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut lines: Vec<&str> = text.split('\n').collect();
    let tail = lines.pop().unwrap_or("");
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty()
            || ["<<<<<<<", "=======", ">>>>>>>", "|||||||"]
                .iter()
                .any(|m| line.starts_with(m))
        {
            continue;
        }
        match serde_json::from_str::<Row>(line) {
            Ok(r) => out.push(r),
            Err(e) => warnings.push(format!("{file}:{}: broken line skipped ({e})", i + 1)),
        }
    }
    if !tail.trim().is_empty() {
        warnings.push(format!(
            "{file}:{}: torn last line (no \\n) ignored",
            lines.len() + 1
        ));
    }
}

fn dedupe(rows: &mut Vec<Row>) {
    let mut seen = HashSet::new();
    rows.retain(|r| r.id.is_empty() || seen.insert(r.id.clone()));
}

/// Validate an add row, then append it to `<fael>/log/<by>/<yyyy-mm>.jsonl`.
pub fn add(fael: &Path, row: &Row, cfg: &Config) -> Result<PathBuf, String> {
    validate(row, cfg)?;
    append(fael, row, false)
}

/// Validate a close row, then append it to `<fael>/log/<by>/<yyyy-mm>.close.jsonl`.
pub fn close(fael: &Path, row: &Row, cfg: &Config) -> Result<PathBuf, String> {
    validate_close(row, cfg)?;
    append(fael, row, true)
}

/// Resolve `supersedes`, stamp, validate, append — the one add path every adapter (CLI, MCP,
/// a server) goes through. `row.files` must already be normalised. Returns the non-fatal warnings.
pub fn add_row(
    fael: &Path,
    log: &Log,
    cfg: &Config,
    stamp: &Stamp,
    mut row: Row,
    supersedes: Option<&str>,
) -> Result<(Row, PathBuf, Vec<String>), String> {
    if let Some(s) = supersedes {
        row.supersedes = Some(resolve(log, s)?.id.clone());
    }
    stamp.apply(&mut row);
    let path = add(fael, &row, cfg)?;
    let warns = warnings(&row, log, cfg);
    Ok((row, path, warns))
}

/// Resolve `id`, stamp, validate, append a close row. Closing twice is a warning, not a reject.
pub fn close_row(
    fael: &Path,
    log: &Log,
    cfg: &Config,
    stamp: &Stamp,
    id: &str,
    why: &str,
) -> Result<(Row, PathBuf, Vec<String>), String> {
    let target = resolve(log, id)?;
    let mut warns = vec![];
    if closed(log).contains(target.id.as_str()) {
        warns.push(format!(
            "fael: {} is already closed — closing it again",
            target.id
        ));
    }
    let mut row = Row::close(&stamp.by, &target.id, why);
    stamp.apply(&mut row);
    let path = close(fael, &row, cfg)?;
    Ok((row, path, warns))
}

/// Append without validating (import/compact write already-checked rows through here).
/// lock → seal a torn tail with `\n` → one `write_all` of the whole line → unlock on drop.
pub fn append(fael: &Path, row: &Row, is_close: bool) -> Result<PathBuf, String> {
    // `by` and the month become path parts — refuse anything that could leave the writer folder
    let by = &row.by;
    if by.is_empty() || by.starts_with(['_', '.']) || by.contains(['/', '\\']) {
        return Err(format!("rejected: writer id {by:?} is not a folder name"));
    }
    let month = row.ts.get(..7).filter(|m| {
        let b = m.as_bytes();
        b[4] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || c.is_ascii_digit())
    });
    let Some(month) = month else {
        return Err(format!("rejected: ts {:?} is not RFC 3339", row.ts));
    };
    let dir = fael.join("log").join(by);
    let path = dir.join(format!(
        "{month}{}.jsonl",
        if is_close { ".close" } else { "" }
    ));
    let io = |e: std::io::Error| format!("{}: {e}", path.display());

    fs::create_dir_all(&dir).map_err(io)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(fael.join(".lock"))
        .map_err(io)?;
    lock.lock().map_err(io)?;

    let mut f = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .map_err(io)?;
    let len = f.metadata().map_err(io)?.len();
    if len >= MONTH_MAX {
        return Err(format!(
            "rejected: {} is ≥ 50 MiB — run `fael compact` first",
            path.display()
        ));
    }
    let line = row.to_line();
    let mut buf = Vec::with_capacity(line.len() + 2);
    if len > 0 {
        let mut last = [0u8];
        f.seek(SeekFrom::End(-1))
            .and_then(|_| f.read_exact(&mut last))
            .map_err(io)?;
        if last[0] != b'\n' {
            buf.push(b'\n'); // seal the torn line off; readers then skip it as a broken line
        }
    }
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    f.write_all(&buf).map_err(io)?;
    Ok(path)
}
