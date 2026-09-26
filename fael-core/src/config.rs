use crate::ROW_BYTES_MAX;
use serde::Deserialize;

/// Per-repo settings from `.fael/config.toml` (every field has a default; see `Config::from_toml`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Extra kinds this repo declares on top of `CORE_KINDS`.
    pub kinds: Vec<String>,
    /// Row size limit in bytes — clamped to `ROW_BYTES_MAX`.
    pub row_bytes: usize,
    /// Allowed first segments of `key`; empty = any. Outside it is a warning, never a reject.
    pub key_domains: Vec<String>,
    /// Token budget for the session brief (`kickoff`, `find` with no filter).
    pub kickoff_tokens: usize,
    /// Token budget for `find` output.
    pub find_tokens: usize,
    /// Token budget for the read/edit hook push.
    pub push_tokens: usize,
    /// How many of the freshest open decisions session-start lists above the
    /// count line (PLAN-fael-direction chunk 1). 0 = count line only.
    pub session_decisions: usize,
    /// Warn when a row's text is estimated over this many tokens.
    pub warn_row_tokens: usize,
    /// Resolve renamed paths through the L2 alias set (`git log -M` + `fael mv`
    /// rows). `false` returns to pre-resolver matching — the escape hatch.
    pub resolve: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            kinds: vec![],
            row_bytes: ROW_BYTES_MAX,
            key_domains: vec![],
            kickoff_tokens: 800,
            find_tokens: 800,
            push_tokens: 800,
            session_decisions: 0,
            warn_row_tokens: 400,
            resolve: true,
        }
    }
}

impl Config {
    /// Parse `.fael/config.toml` text — every field optional. The caller reads the bytes
    /// from wherever the repo lives; a missing file is `Config::default()`, not this.
    pub fn from_toml(s: &str) -> Result<Config, String> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct File {
            kinds: Vec<String>,
            key_domains: Vec<String>,
            resolve: Option<bool>,
            budget: Budget,
            warn: Warn,
            limit: Limit,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Budget {
            kickoff_tokens: Option<usize>,
            find_tokens: Option<usize>,
            push_tokens: Option<usize>,
            session_decisions: Option<usize>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Warn {
            row_tokens: Option<usize>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Limit {
            row_bytes: Option<usize>,
        }
        let f: File = toml::from_str(s).map_err(|e| e.to_string())?;
        let d = Config::default();
        Ok(Config {
            kinds: f.kinds,
            key_domains: f.key_domains,
            row_bytes: f.limit.row_bytes.unwrap_or(d.row_bytes),
            kickoff_tokens: f.budget.kickoff_tokens.unwrap_or(d.kickoff_tokens),
            find_tokens: f.budget.find_tokens.unwrap_or(d.find_tokens),
            push_tokens: f.budget.push_tokens.unwrap_or(d.push_tokens),
            session_decisions: f.budget.session_decisions.unwrap_or(d.session_decisions),
            warn_row_tokens: f.warn.row_tokens.unwrap_or(d.warn_row_tokens),
            resolve: f.resolve.unwrap_or(d.resolve),
        })
    }
}
