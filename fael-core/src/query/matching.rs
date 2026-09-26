use crate::anchor;

/// Exact or under the directory (a zone) — an anchor's ref is opaque, never a zone.
pub(super) fn zone(q: &str, f: &str) -> bool {
    f == q || (anchor(q).is_none() && f.starts_with(q) && f.as_bytes().get(q.len()) == Some(&b'/'))
}

/// Same directory: both are paths (never anchors) with equal parent dirs.
/// Markdown never counts: a dir of plans/docs is a pile of unrelated
/// documents, a dir of code is a module (PLAN-fael-direction chunk 6) — so
/// editing one plan does not push rows filed against another.
pub(super) fn same_dir(q: &str, f: &str) -> bool {
    if anchor(q).is_some() || anchor(f).is_some() {
        return false;
    }
    if is_md(q) || is_md(f) {
        return false;
    }
    fn dir(s: &str) -> &str {
        s.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
    }
    dir(q) == dir(f)
}

/// Markdown by extension, case-insensitive (`README.MD` counts). Compares
/// bytes: a `str` slice at `len - 3` panics inside a multi-byte char.
pub(super) fn is_md(s: &str) -> bool {
    s.as_bytes()
        .get(s.len().saturating_sub(3)..)
        .is_some_and(|e| e.eq_ignore_ascii_case(b".md"))
}

/// Readers accept legacy spellings: `\` separators and a leading `./`.
pub(super) fn lenient(f: &str) -> String {
    let mut s = f.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

pub(super) fn file_match(q: &str, f: &str) -> bool {
    if q.contains(['*', '?', '[']) {
        return glob(q, f);
    }
    // an anchor's ref is opaque — `/` in it is not a directory
    f == q || (anchor(q).is_none() && f.starts_with(q) && f.as_bytes().get(q.len()) == Some(&b'/'))
}

/// Redis `KEYS` glob: `*` any run (including `:` and `/`), `?` one char, `[abc]` `[a-z]` `[^a]`, `\x` literal.
// ponytail: backtracking matcher, exponential on many `*` — keys are ≤ 64 chars so it never matters
pub fn glob(pattern: &str, s: &str) -> bool {
    fn m(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|i| m(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && m(&p[1..], &s[1..]),
            Some('[') if p.len() > 2 && p[2..].contains(&']') && !s.is_empty() => {
                let end = 2 + p[2..].iter().position(|&c| c == ']').unwrap();
                let (neg, set) = match p[1] {
                    '^' => (true, &p[2..end]),
                    _ => (false, &p[1..end]),
                };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= (set[i]..=set[i + 2]).contains(&s[0]);
                        i += 3;
                    } else {
                        hit |= set[i] == s[0];
                        i += 1;
                    }
                }
                hit != neg && m(&p[end + 1..], &s[1..])
            }
            Some('\\') if p.len() > 1 => !s.is_empty() && s[0] == p[1] && m(&p[2..], &s[1..]),
            Some(&c) => !s.is_empty() && s[0] == c && m(&p[1..], &s[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = s.chars().collect();
    m(&p, &s)
}
