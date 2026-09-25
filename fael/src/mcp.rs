//! `fael mcp` — MCP over stdio: newline-delimited JSON-RPC 2.0, three tools (find · add · close).
//! Blocking std I/O, one request at a time — no async runtime on this path (PLAN §4).
//! Tool failures come back as `isError` results so the agent reads the fix; only protocol
//! faults are JSON-RPC errors.

use crate::{Filter, add_row, close_row, core, query, read, repo};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const VERSION: &str = "2025-06-18";

pub fn serve() -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| format!("stdin: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line) {
            writeln!(out, "{reply}")
                .and_then(|()| out.flush())
                .map_err(|e| format!("stdout: {e}"))?;
        }
    }
    Ok(())
}

/// One incoming line → the reply line, or None for a notification.
fn handle(line: &str) -> Option<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(error(Value::Null, -32700, "parse error"));
    };
    // no id = notification (notifications/initialized, cancelled, …) — never answered
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg["method"].as_str().unwrap_or("") {
        "initialize" => json!({
            // ponytail: echo the client's version — the three tools use nothing version-specific
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fael", "version": env!("CARGO_PKG_VERSION")},
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => call(&params),
        m => return Some(error(id, -32601, &format!("method not found: {m}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i32, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

fn call(p: &Value) -> Value {
    let args = &p["arguments"];
    let res = match p["name"].as_str().unwrap_or("") {
        "find" => find(args),
        "add" => add(args),
        "close" => close(args),
        n => Err(format!("unknown tool {n} — fael has find, add, close")),
    };
    let (text, is_error) = match res {
        Ok(t) => (t, false),
        Err(e) => (e, true),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn s(a: &Value, k: &str) -> Option<String> {
    a[k].as_str().filter(|v| !v.is_empty()).map(String::from)
}

fn need(a: &Value, k: &str) -> Result<String, String> {
    s(a, k).ok_or(format!("{k} is required"))
}

fn files(a: &Value) -> Vec<String> {
    a["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

fn find(a: &Value) -> Result<String, String> {
    let r = repo()?;
    let f = Filter {
        text: s(a, "text"),
        files: core::normalize_files(&files(a), &r.cwd, &r.root)?,
        key: s(a, "key"),
        kind: s(a, "kind"),
        since: s(a, "since"),
        ..Filter::default()
    };
    let log = read(&r);
    let (mut rows, budget) = query(&r, &log, &f);
    if let Some(n) = a["limit"].as_u64() {
        rows.truncate(n as usize);
    }
    Ok(if rows.is_empty() {
        "no rows match".into()
    } else {
        core::render(&log, &rows, budget)
    })
}

fn add(a: &Value) -> Result<String, String> {
    let r = repo()?;
    let (row, _, warns) = add_row(
        &r,
        &need(a, "kind")?,
        &need(a, "text")?,
        &files(a),
        s(a, "key"),
        s(a, "supersedes"),
    )?;
    Ok(done(&row.id, warns))
}

fn close(a: &Value) -> Result<String, String> {
    let r = repo()?;
    let (row, _, warns) = close_row(&r, &need(a, "id")?, &need(a, "text")?)?;
    Ok(done(&row.id, warns))
}

fn done(id: &str, warns: Vec<String>) -> String {
    std::iter::once(format!("recorded {id}"))
        .chain(warns)
        .collect::<Vec<_>>()
        .join("\n")
}

fn tools() -> Value {
    let str_ = |d: &str| json!({"type": "string", "description": d});
    let files = |d: &str| json!({"type": "array", "items": {"type": "string"}, "description": d});
    json!([
        {
            "name": "find",
            "description": "Read this project's memory: decisions, open issues and notes left by earlier sessions and teammates. \
    Call it at the start of a task and before touching a file. No arguments = the session brief.",
            "annotations": {"readOnlyHint": true},
            "inputSchema": {"type": "object", "properties": {
                "files": files("repo-relative paths, directories, globs, or anchors like doc:pricing — rows on any of them"),
                "text": str_("case-insensitive substring of the row text"),
                "key": str_("key glob, e.g. auth:*"),
                "kind": str_("decision | issue | note, or a kind the repo declares"),
                "since": str_("yyyy-mm or yyyy-mm-dd"),
                "limit": {"type": "integer", "minimum": 1},
            }},
        },
        {
            "name": "add",
            "description": "Record something the next session must know: a decision and why, a bug (kind issue), \
    or state a later session needs (note). One standalone sentence or two — it is read months later with no chat. \
    files must name what it is about; reuse a path or anchor that find already showed instead of inventing a new one.",
            "inputSchema": {"type": "object", "required": ["kind", "text", "files"], "properties": {
                "kind": str_("decision | issue | note, or a kind the repo declares"),
                "text": str_("what happened and why, standalone"),
                "files": {"type": "array", "items": {"type": "string"}, "minItems": 1,
                    "description": "repo-relative paths, or anchors scheme:ref (doc:pricing, customer:acme) for things that are not files"},
                "key": str_("optional colon key, e.g. auth:session"),
                "supersedes": str_("id of the row this one replaces"),
            }},
        },
        {
            "name": "close",
            "description": "Close a row that no longer holds — an issue that is fixed, a note that is done.",
            "inputSchema": {"type": "object", "required": ["id", "text"], "properties": {
                "id": str_("row id or a unique prefix, as find shows it"),
                "text": str_("why it is closed, e.g. fixed in <sha>"),
            }},
        },
    ])
}
