//! Usage accounting (SPEC §8): every injection into context, per machine —
//! never in git. `fael stats` reads it back.

use super::state::{now_rfc3339, state_dir};
use crate::core;
use std::collections::HashMap;
use std::path::Path;

/// Every injection into context, per machine — never in git. Fails open:
/// a usage write never fails the command it rode along with.
pub(crate) fn record_usage(client: &str, event: &str, repo: &Path, text: &str, ids: &[String]) {
    let row = serde_json::json!({
        "ts": now_rfc3339().unwrap_or_default(),
        "repo": repo.to_string_lossy(),
        "client": client,
        "event": event,
        "bytes": text.len(),
        "est_tokens": core::est_tokens(text),
        "ids": ids,
    });
    let path = state_dir().join("usage.jsonl");
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{row}"));
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "predates the lint — split, then drop"
)]
pub fn stats(json: bool) -> Result<(), String> {
    let path = state_dir().join("usage.jsonl");
    let Ok(s) = std::fs::read_to_string(&path) else {
        println!("fael: no usage recorded yet");
        return Ok(());
    };
    let mut n = 0usize;
    let (mut bytes, mut toks) = (0usize, 0usize);
    let mut by_event: HashMap<String, (usize, usize)> = HashMap::new();
    let mut by_client: HashMap<String, (usize, usize)> = HashMap::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    // (repo, "stop-work"|"stop-bug", ms) — checked against each repo's log below
    let mut blocks: Vec<(String, String, i64)> = vec![];
    // benchmark/test repos live in the OS temp dir and would swamp real usage
    // (01M3CRR6A) — skipped, unless the state dir is scratch too: that run
    // is a test or bench reading its own usage back
    let tmp = [
        std::env::temp_dir(),
        std::env::temp_dir().canonicalize().unwrap_or_default(),
    ];
    let in_tmp = |p: &Path| {
        tmp.iter()
            .any(|t| !t.as_os_str().is_empty() && p.starts_with(t))
    };
    let keep_tmp = in_tmp(&path);
    let mut skipped = 0usize;
    for line in s.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !keep_tmp && v["repo"].as_str().is_some_and(|r| in_tmp(Path::new(r))) {
            skipped += 1;
            continue;
        }
        n += 1;
        let b = v["bytes"].as_u64().unwrap_or(0) as usize;
        let t = v["est_tokens"].as_u64().unwrap_or(0) as usize;
        bytes += b;
        toks += t;
        let ev = v["event"].as_str().unwrap_or("?").to_string();
        let cl = v["client"].as_str().unwrap_or("?").to_string();
        if ev.starts_with("stop-")
            && let (Some(repo), Some(ms)) =
                (v["repo"].as_str(), v["ts"].as_str().and_then(core::ts_ms))
        {
            blocks.push((repo.to_string(), ev.clone(), ms));
        }
        by_event
            .entry(ev)
            .and_modify(|e| {
                e.0 += 1;
                e.1 += t;
            })
            .or_insert((1, t));
        by_client
            .entry(cl)
            .and_modify(|e| {
                e.0 += 1;
                e.1 += t;
            })
            .or_insert((1, t));
        for id in v["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i.as_str())
        {
            *by_id.entry(id.into()).or_insert(0) += 1;
        }
    }
    if n == 0 {
        println!("fael: no usage recorded yet ({skipped} from temp repos skipped)");
        return Ok(());
    }
    // did a row follow each block? work: any add/close · bug: an issue row.
    // ponytail: rows from anyone count — per-session attribution needs `session` on rows
    let mut logs: HashMap<String, core::Log> = HashMap::new();
    let mut outcome: HashMap<String, (usize, usize)> = HashMap::new();
    for (repo, ev, ms) in &blocks {
        let log = logs
            .entry(repo.clone())
            .or_insert_with(|| core::read(&Path::new(repo).join(".fael")));
        let followed = if ev == "stop-bug" {
            log.rows
                .iter()
                .any(|r| r.kind == "issue" && core::ts_ms(&r.ts).is_some_and(|t| t >= *ms))
        } else {
            core::last_row_ms(log, *ms).is_some()
        };
        let e = outcome.entry(ev.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 += followed as usize;
    }
    if json {
        let top: Vec<_> = {
            let mut v: Vec<_> = by_id.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            v.into_iter()
                .take(10)
                .map(|(id, c)| serde_json::json!({"id": id, "pushes": c}))
                .collect()
        };
        println!(
            "{}",
            serde_json::json!({
                "events": n, "bytes": bytes, "est_tokens": toks, "skipped_temp": skipped,
                "by_event": by_event.iter().map(|(k, (c, t))| (k.clone(), serde_json::json!({"events": c, "est_tokens": t}))).collect::<serde_json::Map<String,_>>(),
                "by_client": by_client.iter().map(|(k, (c, t))| (k.clone(), serde_json::json!({"events": c, "est_tokens": t}))).collect::<serde_json::Map<String,_>>(),
                "top_rows": top,
                "stop_blocks": outcome.iter().map(|(k, (b, f))| (k.clone(), serde_json::json!({"blocks": b, "followed_by_row": f}))).collect::<serde_json::Map<String,_>>(),
            })
        );
        return Ok(());
    }
    println!(
        "fael usage ({}): {} injections · {} bytes · ~{} tokens into context",
        path.display(),
        n,
        bytes,
        toks
    );
    if skipped > 0 {
        println!("  skipped ×{skipped} from temp repos (benchmarks, tests)");
    }
    let mut ev: Vec<_> = by_event.iter().collect();
    ev.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    for (k, (c, t)) in ev {
        println!("  {k}: ×{c} (~{t} tokens)");
    }
    let mut cl: Vec<_> = by_client.iter().collect();
    cl.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    for (k, (c, t)) in cl {
        println!("  client {k}: ×{c} (~{t} tokens)");
    }
    let mut ids: Vec<_> = by_id.iter().collect();
    ids.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (id, c) in ids.into_iter().take(10) {
        println!("  row {id}: pushed ×{c}");
    }
    let mut oc: Vec<_> = outcome.iter().collect();
    oc.sort();
    for (k, (b, f)) in oc {
        println!("  {k}: {b} block(s) → {f} followed by a row");
    }
    Ok(())
}
