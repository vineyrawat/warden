// OWNER: analysis
//! `.warden.yml` (repo root) + `AppSettings` (SQLite `settings` table, key `app`).
//!
//! Struct definitions were created by scaffold because many modules use them; the analysis agent owns
//! this file and implements the `todo!()` logic (loading, merging, duration parsing).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Result, WardenError};
use crate::types::{PostingMode, Severity};

// ---------------------------------------------------------------------------------------------
// .warden.yml
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct WardenConfig {
    pub version: u32,
    pub review: ReviewConfig,
    pub rules: Vec<RuleConfig>,
    pub ci: CiConfig,
    pub posting: PostingConfig,
}

impl Default for WardenConfig {
    /// Used when the repo has no `.warden.yml`: built-in Frappe rule set, no CI recipes, Draft mode.
    fn default() -> Self {
        Self {
            version: 1,
            review: ReviewConfig::default(),
            rules: default_rules(),
            ci: CiConfig::default(),
            posting: PostingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct ReviewConfig {
    pub focus: Vec<String>,
    pub ignore_paths: Vec<String>,
    pub max_notes: u32,
    pub min_confidence: f32,
    pub severity_floor: Severity,
    pub nits: bool,
    /// Free text appended to the system prompt.
    pub extra_instructions: String,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            focus: vec![
                "security".into(),
                "correctness".into(),
                "frappe-permissions".into(),
                "performance".into(),
            ],
            ignore_paths: vec![
                "yarn.lock".into(),
                "**/*.min.js".into(),
                "**/dist/**".into(),
            ],
            max_notes: 10,
            min_confidence: 0.7,
            severity_floor: Severity::Minor,
            nits: false,
            extra_instructions: String::new(),
        }
    }
}

/// How a rule is enforced, derived from which field is set (see [`RuleConfig::kind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum RuleKind {
    /// `instruction` → goes into Claude's prompt.
    Claude,
    /// `pattern` → deterministic regex over ADDED lines.
    Regex,
    /// `semgrep` → run semgrep with that rule file if installed, else skip with a notice.
    Semgrep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct RuleConfig {
    pub id: String,
    pub title: String,
    /// Optional glob limiting the files the rule applies to.
    #[serde(default)]
    pub applies_to: Option<String>,
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub semgrep: Option<String>,
    #[serde(default = "default_rule_severity")]
    pub severity: Severity,
}

fn default_rule_severity() -> Severity {
    Severity::Major
}

impl RuleConfig {
    /// Precedence: `semgrep` > `pattern` > `instruction` (a rule with nothing set is a Claude rule
    /// whose instruction is its title).
    pub fn kind(&self) -> RuleKind {
        if self.semgrep.is_some() {
            RuleKind::Semgrep
        } else if self.pattern.is_some() {
            RuleKind::Regex
        } else {
            RuleKind::Claude
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct CiConfig {
    pub recipes: Vec<RecipeConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct RecipeConfig {
    pub name: String,
    /// Shell command template; placeholders `{changed_files}`, `{changed_py}`, `{changed_ts}`,
    /// `{base_sha}`, `{head_sha}` (see `ci::render_command`).
    #[serde(default)]
    pub run: Option<String>,
    /// Adapter instead of `run`, e.g. `gitlab-ci-local`.
    #[serde(default, rename = "use")]
    pub use_: Option<String>,
    /// Jobs for the adapter (`gitlab-ci-local`).
    #[serde(default)]
    pub jobs: Vec<String>,
    /// Relative to the worktree; default ".".
    #[serde(default)]
    pub cwd: Option<String>,
    /// `30s` / `5m` / `1h` (see [`parse_duration`]).
    #[serde(default)]
    pub timeout: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct PostingConfig {
    pub mode: PostingMode,
    pub auto_post: AutoPost,
    pub summary_note: bool,
    pub max_notes_per_hour: u32,
}

impl Default for PostingConfig {
    fn default() -> Self {
        Self {
            mode: PostingMode::Draft,
            auto_post: AutoPost::default(),
            summary_note: true,
            max_notes_per_hour: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct AutoPost {
    pub severities: Vec<Severity>,
    pub min_confidence: f32,
}

impl Default for AutoPost {
    fn default() -> Self {
        Self {
            severities: vec![Severity::Blocker, Severity::Major],
            min_confidence: 0.85,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AppSettings (JSON in SQLite settings table, key "app")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct AppSettings {
    /// Explicit path to the `claude` binary; `None` = look up on PATH (and common install dirs).
    pub claude_cli_path: Option<String>,
    pub model: String,
    pub max_turns: u32,
    #[ts(type = "number")]
    pub review_timeout_secs: u64,
    pub max_reviews_per_hour: u32,
    pub posting_paused: bool,
    pub mcp_port: u16,
    /// `~` is expanded by the consumer.
    pub worktree_root: String,
    /// Holds `warden.db` and `logs/`. `~` is expanded by the consumer.
    pub data_dir: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            claude_cli_path: None,
            model: "opus".into(),
            max_turns: 30,
            review_timeout_secs: 600,
            max_reviews_per_hour: 6,
            posting_paused: false,
            mcp_port: 7811,
            worktree_root: "~/.warden/worktrees".into(),
            data_dir: "~/.warden".into(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Functions (analysis implements)
// ---------------------------------------------------------------------------------------------

/// File names looked up (in order) in the worktree root.
pub const REPO_CONFIG_FILES: [&str; 2] = [".warden.yml", ".warden.yaml"];

/// Reads `<worktree>/.warden.yml` (or `.warden.yaml`). Missing file => `WardenConfig::default()`.
/// An empty (or comment-only) file also yields the defaults. Invalid YAML, unknown enum values,
/// bad regexes/globs/durations and out-of-range numbers are `WardenError::Config`.
pub fn load_repo_config(worktree: &Path) -> Result<WardenConfig> {
    for name in REPO_CONFIG_FILES {
        let path = worktree.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return parse_repo_config(&text).map_err(|e| {
                    WardenError::Config(format!("{}: {}", name, strip_config_prefix(&e)))
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(WardenError::Config(format!("cannot read {name}: {e}"))),
        }
    }
    Ok(WardenConfig::default())
}

fn strip_config_prefix(e: &WardenError) -> String {
    match e {
        WardenError::Config(m) => m.clone(),
        other => other.to_string(),
    }
}

/// Parses the text of a `.warden.yml` and validates it.
pub fn parse_repo_config(text: &str) -> Result<WardenConfig> {
    let has_content = text
        .lines()
        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#') && l.trim() != "---");
    if !has_content {
        return Ok(WardenConfig::default());
    }
    let value: serde_yaml::Value = serde_yaml::from_str(text)?;
    if value.is_null() {
        return Ok(WardenConfig::default());
    }
    if !value.is_mapping() {
        return Err(WardenError::Config(
            "yaml: top level of .warden.yml must be a mapping".into(),
        ));
    }
    let cfg: WardenConfig = serde_yaml::from_value(value)?;
    cfg.validate()?;
    Ok(cfg)
}

/// Deep-merges per-project overrides (AppSettings/project_overrides JSON) over the repo config.
///
/// Semantics are JSON merge patch (RFC 7386): objects merge recursively, scalars and arrays replace,
/// `null` removes a key (so it falls back to its default). One extension: `rules` may be an
/// **object keyed by rule id** instead of an array — each entry is merged into the rule with that
/// id (`null` removes the rule, an unknown id adds a new rule built from the entry).
/// `null` / `{}` overrides return `base` unchanged.
pub fn merge_overrides(base: WardenConfig, overrides: &serde_json::Value) -> Result<WardenConfig> {
    use serde_json::Value;
    match overrides {
        Value::Null => return Ok(base),
        Value::Object(m) if m.is_empty() => return Ok(base),
        Value::Object(_) => {}
        other => {
            return Err(WardenError::Config(format!(
                "overrides must be a JSON object, got {}",
                json_kind(other)
            )))
        }
    }
    let mut merged = serde_json::to_value(&base)?;
    let mut overrides = overrides.clone();

    // rules keyed by id
    let rules_by_id = match overrides.get_mut("rules") {
        Some(v @ Value::Object(_)) => Some(std::mem::take(v)),
        _ => None,
    };
    if let Some(Value::Object(by_id)) = rules_by_id {
        let mut rules: Vec<Value> = merged
            .get("rules")
            .and_then(|r| r.as_array().cloned())
            .unwrap_or_default();
        for (id, patch) in by_id {
            let pos = rules
                .iter()
                .position(|r| r.get("id").and_then(|v| v.as_str()) == Some(id.as_str()));
            match (pos, patch) {
                (Some(i), Value::Null) => {
                    rules.remove(i);
                }
                (None, Value::Null) => {}
                (Some(i), p) => json_merge(&mut rules[i], &p),
                (None, mut p) => {
                    if let Value::Object(o) = &mut p {
                        o.entry("id").or_insert_with(|| Value::String(id.clone()));
                        o.entry("title")
                            .or_insert_with(|| Value::String(id.clone()));
                    }
                    rules.push(p);
                }
            }
        }
        if let Value::Object(o) = &mut overrides {
            o.insert("rules".into(), Value::Array(rules));
        }
    }

    json_merge(&mut merged, &overrides);
    let cfg: WardenConfig = serde_json::from_value(merged)
        .map_err(|e| WardenError::Config(format!("invalid overrides: {e}")))?;
    cfg.validate()?;
    Ok(cfg)
}

fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// RFC 7386 merge patch.
fn json_merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    json_merge(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Accepts `30s`, `5m`, `1h` (and bare seconds). Also `500ms`, `2d`, compound forms like `1h30m`,
/// and long unit names (`10 sec`, `5 minutes`). Case-insensitive; surrounding spaces ignored.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let err = || WardenError::Config(format!("invalid duration {s:?} (use e.g. 30s, 5m, 1h)"));
    let t = s.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Err(err());
    }
    if let Ok(secs) = t.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    let mut total_ms: u128 = 0;
    let mut rest = t.as_str();
    let mut parts = 0;
    while !rest.is_empty() {
        rest = rest.trim_start();
        let num_len = rest
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        if num_len == 0 {
            return Err(err());
        }
        let num: f64 = rest[..num_len].parse().map_err(|_| err())?;
        rest = rest[num_len..].trim_start();
        let unit_len = rest
            .char_indices()
            .find(|(_, c)| !c.is_ascii_alphabetic())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        let factor_ms: f64 = match unit {
            "ms" | "msec" | "millis" | "millisecond" | "milliseconds" => 1.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1_000.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60_000.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000.0,
            "d" | "day" | "days" => 86_400_000.0,
            "" if parts == 0 && rest.trim().is_empty() => 1_000.0,
            _ => return Err(err()),
        };
        let ms = num * factor_ms;
        if !ms.is_finite() || !(0.0..=1.0e15).contains(&ms) {
            return Err(err());
        }
        total_ms += ms.round() as u128;
        parts += 1;
        rest = rest.trim_start_matches([' ', ',']);
    }
    let ms = u64::try_from(total_ms).map_err(|_| err())?;
    Ok(Duration::from_millis(ms))
}

impl WardenConfig {
    /// Checks everything serde cannot: version, unique non-empty rule ids, compilable regexes and
    /// globs, recipe shape and durations, numeric ranges.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(WardenError::Config(m));
        if self.version != 1 {
            return bad(format!("unsupported version {} (expected 1)", self.version));
        }
        let r = &self.review;
        if !(0.0..=1.0).contains(&r.min_confidence) || r.min_confidence.is_nan() {
            return bad(format!(
                "review.min_confidence must be between 0 and 1, got {}",
                r.min_confidence
            ));
        }
        for g in &r.ignore_paths {
            globset::Glob::new(g.trim()).map_err(|e| {
                WardenError::Config(format!("review.ignore_paths: bad glob {g:?}: {e}"))
            })?;
        }
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            if rule.id.trim().is_empty() {
                return bad("rules: every rule needs a non-empty id".into());
            }
            if !seen.insert(rule.id.as_str()) {
                return bad(format!("rules: duplicate rule id {}", rule.id));
            }
            if rule.title.trim().is_empty() {
                return bad(format!("rules: rule {} needs a title", rule.id));
            }
            if let Some(p) = &rule.pattern {
                regex::Regex::new(p).map_err(|e| {
                    WardenError::Config(format!("rules: {}: invalid pattern: {e}", rule.id))
                })?;
            }
            if let Some(g) = &rule.applies_to {
                globset::Glob::new(g.trim()).map_err(|e| {
                    WardenError::Config(format!("rules: {}: invalid applies_to glob: {e}", rule.id))
                })?;
            }
            if matches!(&rule.semgrep, Some(s) if s.trim().is_empty()) {
                return bad(format!("rules: {}: semgrep path is empty", rule.id));
            }
        }
        let mut names = std::collections::HashSet::new();
        for recipe in &self.ci.recipes {
            if recipe.name.trim().is_empty() {
                return bad("ci.recipes: every recipe needs a name".into());
            }
            if !names.insert(recipe.name.as_str()) {
                return bad(format!("ci.recipes: duplicate recipe name {}", recipe.name));
            }
            let has_run = recipe.run.as_deref().is_some_and(|s| !s.trim().is_empty());
            let has_use = recipe.use_.as_deref().is_some_and(|s| !s.trim().is_empty());
            if has_run == has_use {
                return bad(format!(
                    "ci.recipes: {} needs exactly one of `run` or `use`",
                    recipe.name
                ));
            }
            if let Some(t) = &recipe.timeout {
                parse_duration(t).map_err(|_| {
                    WardenError::Config(format!(
                        "ci.recipes: {}: invalid timeout {t:?} (use e.g. 30s, 5m, 1h)",
                        recipe.name
                    ))
                })?;
            }
        }
        let a = &self.posting.auto_post;
        if !(0.0..=1.0).contains(&a.min_confidence) || a.min_confidence.is_nan() {
            return bad(format!(
                "posting.auto_post.min_confidence must be between 0 and 1, got {}",
                a.min_confidence
            ));
        }
        Ok(())
    }

    /// Rules whose instruction goes into Claude's prompt.
    pub fn claude_rules(&self) -> Vec<&RuleConfig> {
        self.rules
            .iter()
            .filter(|r| r.kind() == RuleKind::Claude)
            .collect()
    }

    /// Looks up a rule by id.
    pub fn rule(&self, id: &str) -> Option<&RuleConfig> {
        self.rules.iter().find(|r| r.id == id)
    }
}

impl RuleConfig {
    /// Text to show Claude for this rule: the instruction, or the title when there is none.
    pub fn instruction_text(&self) -> &str {
        self.instruction
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.title)
    }
}

impl RecipeConfig {
    /// Parsed `timeout`, if set and valid.
    pub fn timeout_duration(&self) -> Option<Duration> {
        self.timeout.as_deref().and_then(|t| parse_duration(t).ok())
    }
}

/// Built-in Frappe rule set used when the repo has no `.warden.yml`.
///
/// FR-001 / FR-004 / SEC-001 are Claude rules (SEC-001 is also enforced by the deterministic secret
/// scanner, see `checks::secrets_scan`); FR-002 / FR-007 are regex rules over added lines.
pub fn default_rules() -> Vec<RuleConfig> {
    let rule = |id: &str, title: &str, sev: Severity| RuleConfig {
        id: id.into(),
        title: title.into(),
        applies_to: Some("**/*.py".into()),
        instruction: None,
        pattern: None,
        semgrep: None,
        severity: sev,
    };
    vec![
        RuleConfig {
            instruction: Some(
                "Every @frappe.whitelist() method must enforce permissions (frappe.has_permission, \
                 doc.check_permission, frappe.only_for, or a permission-checked frappe.get_doc/get_list) \
                 before reading or mutating data. Flag whitelisted methods that skip this, especially \
                 allow_guest=True endpoints and ones that take a doctype/name from the request."
                    .into(),
            ),
            ..rule("FR-001", "Whitelisted methods must check permissions", Severity::Major)
        },
        RuleConfig {
            // f-strings (f"", rf"", fr""), "...".format(...) and "..." % x passed straight into
            // frappe.db.sql. Parameterised calls (`"... %s", (x,)`) do not match.
            pattern: Some(
                r#"frappe\.db\.sql\(\s*(?:[rR]?[fF][rR]?["']|(?:"""[^"]*"""|'''[^']*'''|"[^"]*"|'[^']*')\s*(?:%|\.format\())"#
                    .into(),
            ),
            instruction: Some(
                "Never build SQL with f-strings, .format() or % interpolation; pass values as query \
                 parameters (frappe.db.sql(query, values)) or use frappe.qb / frappe.db.get_value / \
                 frappe.db.set_value."
                    .into(),
            ),
            ..rule("FR-002", "No string-formatted SQL", Severity::Blocker)
        },
        RuleConfig {
            instruction: Some(
                "ignore_permissions=True (or flags.ignore_permissions = True) must be accompanied by a \
                 comment explaining why bypassing permissions is safe. Flag uses without one."
                    .into(),
            ),
            ..rule("FR-004", "ignore_permissions=True needs a justification comment", Severity::Major)
        },
        RuleConfig {
            pattern: Some(r"frappe\.db\.commit\(\s*\)".into()),
            instruction: Some(
                "Frappe commits at the end of every request; an explicit frappe.db.commit() inside a \
                 whitelisted/request handler breaks atomicity (partial writes survive later errors). \
                 Remove it unless this is a background job that deliberately commits in batches."
                    .into(),
            ),
            ..rule("FR-007", "No frappe.db.commit() in whitelisted functions", Severity::Major)
        },
        RuleConfig {
            applies_to: None,
            instruction: Some(
                "Flag hardcoded secrets (API keys, tokens, passwords, private keys) in added code. \
                 The deterministic secret scanner also reports these with rule_id SEC-001."
                    .into(),
            ),
            ..rule("SEC-001", "Hardcoded secret", Severity::Blocker)
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let s = AppSettings::default();
        assert_eq!(s.model, "opus");
        assert_eq!(s.max_turns, 30);
        assert_eq!(s.review_timeout_secs, 600);
        assert_eq!(s.max_reviews_per_hour, 6);
        assert_eq!(s.mcp_port, 7811);
        let c = WardenConfig::default();
        assert_eq!(c.review.max_notes, 10);
        let kinds: Vec<_> = c.rules.iter().map(|r| (r.id.as_str(), r.kind())).collect();
        assert!(kinds.contains(&("FR-002", RuleKind::Regex)));
        assert!(kinds.contains(&("FR-001", RuleKind::Claude)));
    }

    #[test]
    fn app_settings_partial_json_fills_defaults() {
        let s: AppSettings = serde_json::from_str(r#"{"model":"sonnet"}"#).unwrap();
        assert_eq!(s.model, "sonnet");
        assert_eq!(s.mcp_port, 7811);
    }

    #[test]
    fn recipe_use_field_renamed() {
        let r: RecipeConfig =
            serde_yaml::from_str("name: gitlab-ci\nuse: gitlab-ci-local\njobs: [build]\n").unwrap();
        assert_eq!(r.use_.as_deref(), Some("gitlab-ci-local"));
        assert_eq!(r.jobs, vec!["build".to_string()]);
    }
}
