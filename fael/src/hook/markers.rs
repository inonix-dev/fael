//! Bug markers (port of fapony's bug-markers.ts, std only): the phrases in
//! the assistant's text that make the stop hook ask for an issue row.

use crate::core;
use std::path::Path;

/// Free-text announcement phrases, never symptom words — a false fire costs a
/// Stop-hook block. Ported without a regex crate to keep the hook path std +
/// serde_json only; the behaviour matches on the common phrases.
const BUG_PHRASES_LATIN: [&str; 12] = [
    "found a bug",
    "found the bug",
    "found bug",
    "found a real bug",
    "found the real bug",
    "this is a bug",
    "that is a bug",
    "it is a bug",
    "it's a bug",
    "bug confirmed",
    "confirmed bug",
    "confirmed a bug",
];

/// Thai phrases are caseless — they match in the lowercased haystack too.
const BUG_PHRASES_THAI: [&str; 5] = ["เจอบั๊ก", "พบว่าเป็นบั๊ก", "เจอว่าเป็นบั๊ก", "บั๊กที่เจอ", "บั๊กที่พบ"];

const NEGATIONS: [&str; 7] = ["ไม่", "จะ", "ถ้า", "อาจ", "not", "no", "if"];

/// Problems short of a confirmed bug — something inconsistent, or expected to
/// break. Reporting these on the spot is the point (an agent has no reason to
/// stay quiet), so a false fire is the accepted cost: one block per session.
/// "might"/"อาจ" are the claim here, not a negation — only a flat denial
/// ("no mismatch", "ไม่มีความเสี่ยง") cancels.
const RISK_PHRASES: [&str; 18] = [
    "inconsistent",
    "inconsistency",
    "mismatch",
    "doesn't match",
    "does not match",
    "out of sync",
    "might break",
    "could break",
    "will break",
    "likely to break",
    "ไม่ตรงกัน",
    "ไม่สอดคล้อง",
    "ขัดแย้งกัน",
    "อาจพัง",
    "น่าจะพัง",
    "อาจมีปัญหา",
    "น่าจะมีปัญหา",
    "มีความเสี่ยง",
];

const RISK_NEGATIONS: [&str; 3] = ["ไม่", "not", "no"];

/// The matched marker phrase, or `None`. Every match is checked against a
/// negation window so "ไม่พบบั๊กใหม่ แต่เจอบั๊กที่ X" still fires on the second.
/// The search runs on the lowercased text throughout, so byte indices always
/// belong to the string they slice.
pub(crate) fn has_bug_marker(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    // phrase lists
    for p in BUG_PHRASES_LATIN.into_iter().chain(BUG_PHRASES_THAI) {
        if let Some(i) = lower.find(p)
            && !negated(&lower, i, &NEGATIONS)
        {
            return Some(lower[i..i + p.len()].to_string());
        }
    }
    for p in RISK_PHRASES {
        if let Some(i) = lower.find(p)
            && !negated(&lower, i, &RISK_NEGATIONS)
        {
            return Some(p.to_string());
        }
    }
    // `bug…:` — "**Bug (cause…):**" (same line, optional paren group)
    let mut from = 0;
    while let Some(i) = lower[from..].find("bug") {
        let i = from + i;
        if word_boundary(&lower, i, 3)
            && colon_after(&lower, i + 3)
            && !negated(&lower, i, &NEGATIONS)
        {
            return Some("bug:".to_string());
        }
        from = i + 3;
    }
    None
}

fn word_boundary(s: &str, i: usize, len: usize) -> bool {
    let b = s.as_bytes();
    let left = i == 0 || !b[i - 1].is_ascii_alphanumeric();
    let right = b.get(i + len).is_none_or(|c| !c.is_ascii_alphanumeric());
    left && right
}

fn colon_after(s: &str, mut i: usize) -> bool {
    let b = s.as_bytes();
    while b.get(i).is_some_and(|c| *c == b' ' || *c == b'\t') {
        i += 1;
    }
    if b.get(i) == Some(&b'(') {
        // skip to the closing paren on this line
        while let Some(c) = b.get(i) {
            i += 1;
            if *c == b')' {
                break;
            }
            if *c == b'\n' {
                return false;
            }
        }
        while b.get(i).is_some_and(|c| *c == b' ' || *c == b'\t') {
            i += 1;
        }
    }
    // a colon before the line ends
    s[i..]
        .split('\n')
        .next()
        .is_some_and(|l| l.trim_end().ends_with(':'))
}

/// The ~15 chars before the match end in a negation word.
fn negated(lower: &str, i: usize, negations: &[&str]) -> bool {
    let before: String = lower[..i]
        .chars()
        .rev()
        .take(15)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let t = before.trim_end().to_lowercase();
    negations.iter().any(|n| {
        t.ends_with(n)
            && (n.chars().all(|c| !c.is_ascii_alphabetic())
                || t[..t.len() - n.len()]
                    .chars()
                    .last()
                    .is_none_or(|c| !c.is_alphabetic()))
    })
}

/// Scan assistant text in a Claude transcript tail for a bug marker. Reads
/// only the last 200 KB (and nothing over 10 MB) — the hook must not stall
/// turn-end. Returns the first matched phrase.
pub(crate) fn bug_signal_from_transcript(path: &Path, since_ms: i64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    if size > 10 * 1024 * 1024 {
        return None;
    }
    let tail = size.min(200 * 1024);
    f.seek(SeekFrom::Start(size - tail)).ok()?;
    let mut buf = vec![0u8; tail as usize];
    f.read_exact(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split('\n').peekable();
    if tail < size {
        lines.next(); // first line is partial
    }
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let m = &v["message"];
        if m["role"] != "assistant" {
            continue;
        }
        // Claude Code stamps each line at the top level, not inside `message`
        if let Some(created) = v["timestamp"].as_str()
            && let Some(ms) = core::ts_ms(created)
            && ms < since_ms
        {
            continue;
        }
        for b in m["content"].as_array().into_iter().flatten() {
            if b["type"] != "text" {
                continue;
            }
            if let Some(t) = b["text"].as_str()
                && let Some(hit) = has_bug_marker(t)
            {
                return Some(hit);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::has_bug_marker;

    #[test]
    fn markers_catch_bugs_and_risks_not_denials() {
        for hit in [
            "I found a bug in login",
            "doc กับโค้ดไม่ตรงกัน",
            "config and schema are out of sync",
            "this might break the importer",
            "ตรงนี้อาจมีปัญหาตอน merge",
        ] {
            assert!(has_bug_marker(hit).is_some(), "{hit}");
        }
        for miss in [
            "no bug found",
            "no mismatch left",
            "ไม่มีความเสี่ยง",
            "ถ้าเจอบั๊กให้บอก",
            "all tests pass",
        ] {
            assert!(has_bug_marker(miss).is_none(), "{miss}");
        }
    }
}
