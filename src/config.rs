use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub auto: AutoConfig,
}

/// Background polling and automatic review behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoConfig {
    /// Poll GitHub and run reviews automatically. Off unless you ask for it
    /// with `ghpr -r 5m`, so ghpr never spends CPU or starts reviews behind
    /// your back by default.
    #[serde(default = "default_auto_enabled")]
    pub enabled: bool,

    /// How often to look for new or updated PRs (seconds, minimum 15).
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// How many AI reviews may run at the same time.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// Only review PRs where you are a requested reviewer or an assignee.
    /// PRs you already have a local draft for are always re-reviewed when they
    /// change, regardless of this setting.
    #[serde(default = "default_only_assigned")]
    pub only_assigned: bool,
}

fn default_auto_enabled() -> bool { false }
fn default_poll_interval() -> u64 { 60 }
fn default_max_concurrent() -> usize { 2 }
fn default_only_assigned() -> bool { true }

impl Default for AutoConfig {
    fn default() -> Self {
        Self {
            enabled: default_auto_enabled(),
            poll_interval_secs: default_poll_interval(),
            max_concurrent: default_max_concurrent(),
            only_assigned: default_only_assigned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Display name in UI (e.g. "Claude", "Copilot", "AI")
    #[serde(default = "default_ai_name")]
    pub name: String,

    /// Command to run (e.g. "claude", "gh copilot", "ollama")
    #[serde(default = "default_command")]
    pub command: String,

    /// Arguments. Use {pr_url} as placeholder for the PR URL.
    /// Use {system_prompt} as placeholder for the structured output instructions.
    #[serde(default = "default_args")]
    pub args: Vec<String>,

    /// Marker line in output that precedes the JSON comment array.
    #[serde(default = "default_json_marker")]
    pub json_marker: String,

    /// Output mode: "stream-json" (claude CLI) or "text" (collect all stdout)
    #[serde(default = "default_output_mode")]
    pub output_mode: String,

    /// Lowest severity worth showing: CRITICAL, HIGH, MEDIUM, LOW or INFO.
    /// Anything below it is dropped as noise. Use "INFO" to keep everything.
    #[serde(default = "default_min_severity")]
    pub min_severity: String,
}

/// Rank a severity name so thresholds can be compared. Returns `None` for a
/// missing or unrecognised severity — those are always kept, so an agent that
/// doesn't label its findings never has them silently thrown away.
pub fn severity_rank(severity: Option<&str>) -> Option<u8> {
    match severity?.trim().to_uppercase().as_str() {
        "INFO" => Some(0),
        "LOW" => Some(1),
        "MEDIUM" | "MED" => Some(2),
        "HIGH" => Some(3),
        "CRITICAL" | "CRIT" => Some(4),
        _ => None,
    }
}

fn default_ai_name() -> String { "AI".to_string() }
fn default_command() -> String { "claude".to_string() }
fn default_args() -> Vec<String> {
    vec![
        "-p".to_string(),
        "/auto-review-pr {pr_url}".to_string(),
        "--append-system-prompt".to_string(),
        "{system_prompt}".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--allowedTools".to_string(),
        "Bash(gh:*)".to_string(),
        "--allowedTools".to_string(),
        "Read".to_string(),
        "--allowedTools".to_string(),
        "Glob".to_string(),
        "--allowedTools".to_string(),
        "Grep".to_string(),
        "--allowedTools".to_string(),
        "WebFetch".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
        "--no-session-persistence".to_string(),
    ]
}
fn default_json_marker() -> String { "---GHPR_JSON---".to_string() }
fn default_output_mode() -> String { "stream-json".to_string() }
fn default_min_severity() -> String { "MEDIUM".to_string() }

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            name: default_ai_name(),
            command: default_command(),
            args: default_args(),
            json_marker: default_json_marker(),
            output_mode: default_output_mode(),
            min_severity: default_min_severity(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ai: AiConfig::default(),
            auto: AutoConfig::default(),
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ghpr")
            .join("config.toml")
    }

    /// Load config from disk. Returns None if file doesn't exist.
    pub fn load() -> Result<Option<Self>> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;
        Ok(Some(config))
    }

    /// Write default config to disk.
    pub fn write_default() -> Result<PathBuf> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let config = Config::default();
        let content = toml::to_string_pretty(&config)?;
        let full = format!("{}\n\n{}", CONFIG_HEADER, content);
        std::fs::write(&path, full)?;
        Ok(path)
    }

    /// Severity threshold for keeping a finding, or `None` to keep everything.
    pub fn min_severity_rank(&self) -> Option<u8> {
        severity_rank(Some(&self.ai.min_severity))
    }

    /// Severities at or above the configured threshold, highest first.
    fn allowed_severities(&self) -> Vec<&'static str> {
        const ALL: [(&str, u8); 5] = [
            ("CRITICAL", 4),
            ("HIGH", 3),
            ("MEDIUM", 2),
            ("LOW", 1),
            ("INFO", 0),
        ];
        let min = self.min_severity_rank().unwrap_or(0);
        ALL.iter()
            .filter(|(_, rank)| *rank >= min)
            .map(|(name, _)| *name)
            .collect()
    }

    /// Build the system prompt that instructs the AI to output structured JSON.
    pub fn system_prompt(&self) -> String {
        let allowed = self.allowed_severities();
        // Ask the agent not to generate what we would only throw away.
        let threshold = if allowed.len() < 5 {
            format!(
                "\nOnly report findings of severity {}. Do NOT report anything less important than {} — such findings must be omitted entirely.",
                allowed.join(", "),
                self.ai.min_severity.trim().to_uppercase(),
            )
        } else {
            String::new()
        };

        format!(
            r#"IMPORTANT: After your review, you MUST end your response with a JSON block on a new line starting with {marker} followed by a JSON array of all your file-specific comments. Each element must have:
- "filename" (full file path)
- "line" (line number in new file)
- "severity" (one of: {allowed})
- "comment" (your review comment text){threshold}
Example:
{marker}
[{{"filename":"src/app.ts","line":42,"severity":"HIGH","comment":"Consider handling the error"}}]
If no comments, output:
{marker}
[]"#,
            marker = self.ai.json_marker,
            allowed = allowed.join(", "),
            threshold = threshold,
        )
    }

    /// Expand args, replacing {pr_url} and {system_prompt} placeholders.
    pub fn expand_args(&self, pr_url: &str) -> Vec<String> {
        let sys_prompt = self.system_prompt();
        self.ai.args.iter().map(|a| {
            a.replace("{pr_url}", pr_url)
             .replace("{system_prompt}", &sys_prompt)
        }).collect()
    }
}

const CONFIG_HEADER: &str = r#"# ghpr AI agent configuration
#
# The AI agent is called to review pull requests. It must output
# structured comments in JSON format so ghpr can display them inline.
#
# Placeholders in args:
#   {pr_url}         - replaced with the GitHub PR URL
#   {system_prompt}  - replaced with instructions for JSON output format
#
# Output modes:
#   "stream-json"  - parse streaming JSON (claude CLI --output-format stream-json)
#   "text"         - collect all stdout, then extract JSON at the end
#
# Expected JSON output format (after the json_marker line):
#   [
#     {"filename": "src/main.rs", "line": 42, "severity": "HIGH", "comment": "Consider error handling here"},
#     {"filename": "src/lib.rs", "line": 10, "severity": "LOW", "comment": "This could be simplified"}
#   ]
#
# Fields:
#   filename  - full file path in the repo (also accepts "file")
#   line      - line number in the new version of the file
#   severity  - one of: CRITICAL, HIGH, MEDIUM, LOW, INFO (optional, shown color-coded)
#   comment   - the review comment text (also accepts "body")
#
# Noise filter:
#   min_severity  - lowest severity kept; anything below it is discarded.
#                   Defaults to "MEDIUM", so LOW and INFO findings are ignored.
#                   Set to "INFO" to keep everything, "HIGH" to be stricter.
#                   Findings with no severity at all are always kept.
#                   The threshold is also passed to the agent so it doesn't
#                   waste time producing findings that would be thrown away.
#
# Example configs:
#
# [ai]
# name = "Claude"
# command = "claude"
# args = ["-p", "/auto-review-pr {pr_url}", "--append-system-prompt", "{system_prompt}", "--output-format", "stream-json", "--allowedTools", "Bash(gh:*)", "--allowedTools", "Read", "--allowedTools", "Glob", "--allowedTools", "Grep", "--allowedTools", "WebFetch", "--verbose", "--include-partial-messages", "--no-session-persistence"]
# json_marker = "---GHPR_JSON---"
# output_mode = "stream-json"
# min_severity = "MEDIUM"
#
# [ai]
# name = "Custom Agent"
# command = "my-review-tool"
# args = ["review", "--pr", "{pr_url}", "--format", "json"]
# json_marker = "---GHPR_JSON---"
# output_mode = "text"
#
# Automatic background review. Off by default — turn it on for a session with
# `ghpr -r 5m`, or set enabled = true here to have it always on.
#
# [auto]
# enabled = false            # poll for new/updated PRs and review them
# poll_interval_secs = 300   # how often to check (minimum 15); -r overrides
# max_concurrent = 2         # parallel AI reviews
# only_assigned = true       # only PRs where you're a reviewer or assignee
#
# Draft PRs and your own PRs are never auto-reviewed. Results are stored in
# ~/.ghpr/drafts/ until you submit the review or the PR is closed.
"#;

/// Help text shown when no config exists.
#[allow(dead_code)]
pub fn setup_help() -> String {
    format!(
        r#"No AI agent configured.

Config file: {}

To get started, run ghpr once — a default config will be created.
Edit it to point to your AI review tool.

The AI agent must:
  1. Accept a PR URL as input
  2. Review the code
  3. Output a JSON array of comments after a marker line

Expected output format:
  ---GHPR_JSON---
  [
    {{"filename": "src/main.rs", "line": 42, "severity": "HIGH", "comment": "Your comment here"}}
  ]

Each comment needs:
  - filename: full file path in the repo (also accepts "file")
  - line:     line number in the new version of the file
  - severity: CRITICAL, HIGH, MEDIUM, LOW, or INFO (optional)
  - comment:  the review comment text (also accepts "body")

Press 'c' in diff view to trigger the AI review."#,
        Config::config_path().display()
    )
}
