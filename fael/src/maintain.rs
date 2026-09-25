//! `fael doctor` · `fael compact` · `fael import` — the maintenance commands
//! (SPEC §6, §11). Thin adapters: the repo is resolved here, the rules live
//! in `fael-core` so a hosted server calls the same entry points.

use crate::{Args, core, repo};
use std::path::PathBuf;
use std::process::ExitCode;

pub fn doctor(a: &Args) -> Result<ExitCode, String> {
    let r = repo()?;
    let month = core::current_month();
    let source = crate::hook::ignore_source(&r.root);
    let excluded = source.as_deref().is_some_and(crate::hook::deliberate);
    let ignored = source.is_some() && !excluded;
    if a.has("fix") {
        let before = core::doctor_scan(&r.fael, &r.root, ignored, &month);
        for action in core::doctor_fix(&r.fael, &r.root, &before)? {
            println!("fixed: {action}");
        }
    }
    let mut rep = core::doctor_scan(&r.fael, &r.root, ignored, &month);
    if excluded {
        rep.problems.push(core::Problem {
            kind: core::ProblemKind::Ignored,
            severity: core::Severity::Info,
            fixable: false,
            file: None,
            detail: ".fael/log is kept local by .git/info/exclude — taken as deliberate; \
                     move the pattern to .gitignore if it is not"
                .into(),
        });
    }
    let log = crate::read(&r);
    // Gone is judged through the resolver: a file that was renamed still
    // exists under its new path, so its rows still push and are not gone.
    let al = crate::aliases::load(&r, &log, true);
    let gone: Vec<_> = core::find(&log, &core::Filter::default())
        .into_iter()
        .filter(|row| core::gone(&r.root, row, &al))
        .collect();
    if !gone.is_empty() {
        let w = core::abbrev(&log);
        let eg: Vec<String> = gone
            .iter()
            .take(5)
            .map(|row| {
                format!(
                    "{} → {}",
                    &row.id[..w.min(row.id.len())],
                    row.files.join(", ")
                )
            })
            .collect();
        rep.problems.push(core::Problem {
            kind: core::ProblemKind::Gone,
            severity: core::Severity::Info,
            fixable: false,
            file: None,
            detail: format!(
                "{} open row(s) name only files that no longer exist, so they never push — \
                 re-file them on the new path or `fael close` them (e.g. {})",
                gone.len(),
                eg.join("; ")
            ),
        });
    }
    show(&rep, a.has("json"));
    Ok(if rep.errors().count() > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn show(rep: &core::DoctorReport, json: bool) {
    if json {
        let ps: Vec<_> = rep
            .problems
            .iter()
            .map(|p| {
                serde_json::json!({
                    "kind": format!("{:?}", p.kind).to_lowercase(),
                    "severity": format!("{:?}", p.severity).to_lowercase(),
                    "fixable": p.fixable,
                    "detail": p.detail,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(ps));
        return;
    }
    if rep.problems.is_empty() {
        println!("fael doctor: clean");
        return;
    }
    let errs = rep.errors().count();
    println!(
        "fael doctor: {} problem(s) ({} error(s), {} note(s))",
        rep.problems.len(),
        errs,
        rep.problems.len() - errs
    );
    for p in &rep.problems {
        let sev = if p.severity == core::Severity::Error {
            "error"
        } else {
            "note"
        };
        let fix = if p.fixable { " [--fix]" } else { "" };
        println!("{sev} [{:?}]{fix}: {}", p.kind, p.detail);
    }
}

pub fn compact(a: &Args) -> Result<ExitCode, String> {
    let r = repo()?;
    if let Some(b) = a.one("before") {
        valid_month(&b)?;
    }
    let opts = core::CompactOpts {
        writer: a.one("writer"),
        before: a.one("before"),
        prune: a.has("prune"),
    };
    // prune judges "gone" through the resolver, so the aliases are loaded
    // the same way kickoff loads them (refresh = pick up latest renames)
    let al = crate::aliases::load(&r, &crate::read(&r), true);
    let rep = core::compact(&r.fael, &r.root, &opts, &core::current_month(), &al)?;
    if a.has("json") {
        let ws: Vec<_> = rep
            .writers
            .iter()
            .map(|w| {
                serde_json::json!({
                    "writer": w.writer, "rows": w.rows, "folded": w.folded,
                    "pruned": w.pruned, "carried": w.carried, "deleted": w.deleted,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(ws));
    } else {
        for w in &rep.writers {
            println!(
                "{}: {} row(s) compacted ({} close(s) folded, {} pruned, {} carried) — deleted {}",
                w.writer,
                w.rows,
                w.folded,
                w.pruned,
                w.carried,
                w.deleted.join(", ")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn valid_month(b: &str) -> Result<(), String> {
    let ok = b.len() == 7
        && b.as_bytes()[4] == b'-'
        && b.bytes()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(format!(
            "rejected: --before {b:?} is not yyyy-mm (e.g. 2026-08)"
        ))
    }
}

pub fn import(a: &Args, src: &str) -> Result<ExitCode, String> {
    let r = repo()?;
    let mut maps = vec![];
    for m in a.many("map") {
        let (old, new) = m
            .split_once('=')
            .filter(|(o, n)| !o.is_empty() && !n.is_empty())
            .ok_or_else(|| format!("rejected: --map {m:?} — write --map old/=new/"))?;
        maps.push((old.to_string(), new.to_string()));
    }
    let sp = PathBuf::from(src);
    let sp = if sp.is_absolute() { sp } else { r.cwd.join(sp) };
    let rep = core::import(&r.fael, &sp, &r.cfg.kinds, &core::ImportOpts { maps })?;
    for w in &rep.warnings {
        eprintln!("{w}");
    }
    if a.has("json") {
        println!(
            "{}",
            serde_json::json!({
                "adds": rep.adds, "folded": rep.folded, "carried": rep.carried,
                "skipped": rep.skipped,
                "paths": rep.paths.iter().map(|p| p.strip_prefix(&r.root).unwrap_or(p).display().to_string()).collect::<Vec<_>>(),
            })
        );
    } else {
        let paths = rep
            .paths
            .iter()
            .map(|p| p.strip_prefix(&r.root).unwrap_or(p).display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "imported {} row(s) ({} close(s) folded, {} carried, {} skipped) → {paths}",
            rep.adds, rep.folded, rep.carried, rep.skipped
        );
    }
    Ok(ExitCode::SUCCESS)
}
