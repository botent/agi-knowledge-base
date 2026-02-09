//! File-backed background agent recipes loaded from `$MEMINI_HOME/agents`.
//!
//! Recipe files are Markdown with optional front matter:
//!
//! ```text
//! ---
//! name: repo-digest
//! description: summarize repo activity
//! interval_secs: 1800
//! auto_start: false
//! trigger_events: VariableUpdate
//! trigger_variables: deploy.request,ci.*
//! tools: local
//! persona: You are a repo digest agent.
//! ---
//! Summarize recent repository changes and propose next actions.
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;

use crate::constants::DEFAULT_AGENT_INTERVAL_SECS;

#[derive(Clone, Debug)]
pub struct AgentRecipe {
    pub name: String,
    pub description: String,
    pub interval_secs: u64,
    pub auto_start: bool,
    pub trigger_events: Vec<String>,
    pub trigger_variables: Vec<String>,
    pub tools: Vec<String>,
    pub persona: String,
    pub instructions: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct NewAgentRecipe {
    pub name: String,
    pub description: String,
    pub interval_secs: u64,
    pub auto_start: bool,
    pub tools: Vec<String>,
    pub persona: String,
    pub instructions: String,
}

#[derive(Clone, Debug)]
pub struct RecipeTemplate {
    pub id: &'static str,
    pub description: &'static str,
    pub interval_secs: u64,
    pub tools: &'static [&'static str],
    pub persona: &'static str,
    pub instructions: &'static str,
}

impl AgentRecipe {
    pub fn has_trigger(&self) -> bool {
        !self.trigger_events.is_empty() || !self.trigger_variables.is_empty()
    }

    pub fn trigger_summary(&self) -> Option<String> {
        if !self.has_trigger() {
            return None;
        }
        let events = if self.trigger_events.is_empty() {
            "VariableUpdate".to_string()
        } else {
            self.trigger_events.join("|")
        };
        let vars = if self.trigger_variables.is_empty() {
            "*".to_string()
        } else {
            self.trigger_variables.join(",")
        };
        Some(format!("{events}:{vars}"))
    }

    pub fn matches_trigger(&self, event_type: &str, variable_name: Option<&str>) -> bool {
        if !self.has_trigger() {
            return false;
        }

        let event_match = if self.trigger_events.is_empty() {
            event_type.eq_ignore_ascii_case("VariableUpdate")
        } else {
            self.trigger_events
                .iter()
                .any(|pattern| pattern.eq_ignore_ascii_case(event_type))
        };
        if !event_match {
            return false;
        }

        if self.trigger_variables.is_empty() {
            return true;
        }

        let Some(name) = variable_name else {
            return false;
        };
        let candidate = name.to_ascii_lowercase();

        self.trigger_variables.iter().any(|pattern| {
            let normalized = pattern.trim().to_ascii_lowercase();
            if normalized.is_empty() {
                return false;
            }
            if normalized == "*" {
                return true;
            }
            if let Some(prefix) = normalized.strip_suffix('*') {
                return candidate.starts_with(prefix);
            }
            candidate == normalized
        })
    }
}

const RECIPE_TEMPLATES: &[RecipeTemplate] = &[
    RecipeTemplate {
        id: "repo-watch",
        description: "Track repo state, test failures, and unfinished work.",
        interval_secs: 1800,
        tools: &[
            "workspace_list_files",
            "workspace_read_file",
            "workspace_run_command",
        ],
        persona: "You are a repository watchdog agent. Focus on risky changes, broken tests, and unfinished tasks.",
        instructions: "Inspect the repository, run quick verification commands, and summarize what changed, what failed, and what to do next.",
    },
    RecipeTemplate {
        id: "release-notes",
        description: "Draft concise release notes from recent source changes.",
        interval_secs: 3600,
        tools: &["workspace_read_file", "workspace_run_command"],
        persona: "You are a release-notes agent. Produce concise, accurate, developer-facing notes.",
        instructions: "Analyze recent project changes and draft release notes with sections: Added, Changed, Fixed, and Follow-ups.",
    },
    RecipeTemplate {
        id: "cleanup",
        description: "Find stale files and suggest safe cleanup actions.",
        interval_secs: 7200,
        tools: &[
            "workspace_list_files",
            "workspace_read_file",
            "workspace_run_command",
        ],
        persona: "You are a codebase cleanup agent. Prefer safe, incremental improvements.",
        instructions: "Scan for stale artifacts, dead scripts, and obvious cleanup opportunities. Propose a prioritized cleanup plan.",
    },
];

pub fn recipe_templates() -> &'static [RecipeTemplate] {
    RECIPE_TEMPLATES
}

pub fn recipe_template_by_id(id: &str) -> Option<&'static RecipeTemplate> {
    RECIPE_TEMPLATES
        .iter()
        .find(|template| template.id.eq_ignore_ascii_case(id.trim()))
}

pub fn load_agent_recipes() -> Result<Vec<AgentRecipe>> {
    let dir = ensure_agents_dir()?;
    let mut recipes = Vec::new();

    for entry in fs::read_dir(&dir).with_context(|| format!("Read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let is_md = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !is_md {
            continue;
        }

        let raw = fs::read_to_string(&path).with_context(|| format!("Read {}", path.display()))?;
        let recipe = parse_recipe_file(&path, &raw)
            .with_context(|| format!("Parse agent recipe {}", path.display()))?;
        recipes.push(recipe);
    }

    recipes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(recipes)
}

pub fn write_recipe_file(spec: &NewAgentRecipe) -> Result<PathBuf> {
    let name = sanitize_name(&spec.name)?;
    if spec.instructions.trim().is_empty() {
        bail!("instructions cannot be empty");
    }

    let dir = ensure_agents_dir()?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        bail!("Agent recipe already exists: {}", path.display());
    }

    let content = render_recipe_markdown(
        &name,
        &spec.description,
        spec.interval_secs,
        spec.auto_start,
        &spec.tools,
        &spec.persona,
        &spec.instructions,
    );
    fs::write(&path, content).with_context(|| format!("Write {}", path.display()))?;
    Ok(path)
}

pub fn remove_recipe_file(name: &str) -> Result<Option<PathBuf>> {
    let name = sanitize_name(name)?;
    let path = agents_dir().join(format!("{name}.md"));
    if !path.exists() {
        return Ok(None);
    }
    fs::remove_file(&path).with_context(|| format!("Remove {}", path.display()))?;
    Ok(Some(path))
}

pub fn ensure_agents_dir() -> Result<PathBuf> {
    let dir = agents_dir();
    fs::create_dir_all(&dir).with_context(|| format!("Create {}", dir.display()))?;
    Ok(dir)
}

pub fn agents_dir() -> PathBuf {
    memini_home().join("agents")
}

fn parse_recipe_file(path: &Path, raw: &str) -> Result<AgentRecipe> {
    let (front_matter, body) = split_front_matter(raw);

    let fallback_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("agent");
    let name = front_matter
        .get("name")
        .map(|value| sanitize_name(value))
        .transpose()?
        .unwrap_or_else(|| fallback_name.to_string());

    let description = front_matter
        .get("description")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();

    let interval_secs = front_matter
        .get("interval_secs")
        .or_else(|| front_matter.get("interval"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_AGENT_INTERVAL_SECS);

    let auto_start = front_matter
        .get("auto_start")
        .or_else(|| front_matter.get("autostart"))
        .and_then(|value| parse_bool(value))
        .unwrap_or(false);

    let trigger_events = front_matter
        .get("trigger_events")
        .or_else(|| front_matter.get("events"))
        .map(|value| parse_csv(value))
        .unwrap_or_default();

    let trigger_variables = front_matter
        .get("trigger_variables")
        .or_else(|| front_matter.get("trigger_vars"))
        .or_else(|| front_matter.get("trigger_keys"))
        .map(|value| parse_csv(value))
        .unwrap_or_default();

    let tools = front_matter
        .get("tools")
        .map(|value| parse_csv(value))
        .unwrap_or_default();

    let persona = front_matter.get("persona").cloned().unwrap_or_else(|| {
        format!(
            "You are a background autonomous agent named '{name}'. \
                 Be concise, action-oriented, and explicit about changes."
        )
    });

    let instructions = if !body.trim().is_empty() {
        body.trim().to_string()
    } else if let Some(value) = front_matter
        .get("instructions")
        .or_else(|| front_matter.get("prompt"))
    {
        value.trim().to_string()
    } else {
        String::new()
    };

    if instructions.is_empty() {
        return Err(anyhow!(
            "missing instructions: add markdown body or front matter `instructions`"
        ));
    }

    Ok(AgentRecipe {
        name,
        description,
        interval_secs,
        auto_start,
        trigger_events,
        trigger_variables,
        tools,
        persona,
        instructions,
        path: path.to_path_buf(),
    })
}

fn split_front_matter(raw: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut map = std::collections::HashMap::new();
    let mut lines = raw.lines();
    let Some(first) = lines.next() else {
        return (map, String::new());
    };
    if first.trim() != "---" {
        return (map, raw.to_string());
    }

    let mut consumed = first.len() + 1;
    for line in lines.by_ref() {
        consumed += line.len() + 1;
        if line.trim() == "---" {
            let body = raw.get(consumed..).unwrap_or("").to_string();
            return (map, body);
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            map.insert(
                key.trim().to_ascii_lowercase(),
                strip_quotes(value.trim()).to_string(),
            );
        }
    }

    // No closing front matter fence; treat whole text as body.
    (std::collections::HashMap::new(), raw.to_string())
}

fn render_recipe_markdown(
    name: &str,
    description: &str,
    interval_secs: u64,
    auto_start: bool,
    tools: &[String],
    persona: &str,
    instructions: &str,
) -> String {
    let description = if description.trim().is_empty() {
        "Custom auto-agent"
    } else {
        description.trim()
    };
    let tool_line = if tools.is_empty() {
        "local".to_string()
    } else {
        tools.join(",")
    };

    format!(
        "---\nname: {name}\ndescription: {}\ninterval_secs: {interval_secs}\nauto_start: {auto_start}\ntools: {}\npersona: {}\n---\n{}\n",
        yaml_quote(description),
        yaml_quote(&tool_line),
        yaml_quote(persona.trim()),
        instructions.trim(),
    )
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

fn strip_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

fn sanitize_name(raw: &str) -> Result<String> {
    let candidate = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if candidate.is_empty() {
        bail!("name cannot be empty");
    }
    Ok(candidate)
}

fn memini_home() -> PathBuf {
    if let Ok(value) = env::var("MEMINI_HOME") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Some(base_dirs) = BaseDirs::new() {
        return base_dirs.home_dir().join("Memini");
    }
    PathBuf::from("Memini")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recipe_front_matter_and_body() {
        let raw = r#"---
name: repo-watch
description: repo status
interval_secs: 120
auto_start: true
trigger_events: VariableUpdate,Commit
trigger_variables: deploy.request,ci.*
tools: workspace_read_file,workspace_run_command
persona: You are a repo agent.
---
Check git status and summarize changes.
"#;
        let parsed = parse_recipe_file(Path::new("repo-watch.md"), raw).expect("parse recipe");
        assert_eq!(parsed.name, "repo-watch");
        assert_eq!(parsed.description, "repo status");
        assert_eq!(parsed.interval_secs, 120);
        assert!(parsed.auto_start);
        assert_eq!(parsed.trigger_events, vec!["VariableUpdate", "Commit"]);
        assert_eq!(parsed.trigger_variables, vec!["deploy.request", "ci.*"]);
        assert_eq!(
            parsed.tools,
            vec!["workspace_read_file", "workspace_run_command"]
        );
        assert_eq!(parsed.persona, "You are a repo agent.");
        assert_eq!(
            parsed.instructions,
            "Check git status and summarize changes."
        );
    }

    #[test]
    fn parse_recipe_without_front_matter_uses_body() {
        let raw = "Summarize unfinished tasks.";
        let parsed = parse_recipe_file(Path::new("quick-check.md"), raw).expect("parse recipe");
        assert_eq!(parsed.name, "quick-check");
        assert_eq!(parsed.instructions, "Summarize unfinished tasks.");
    }

    #[test]
    fn trigger_matching_supports_exact_and_prefix() {
        let raw = r#"---
name: trigger-agent
auto_start: true
trigger_events: VariableUpdate
trigger_variables: deploy.request,ci.*
---
React to deployment-related updates.
"#;
        let parsed = parse_recipe_file(Path::new("trigger-agent.md"), raw).expect("parse recipe");
        assert!(parsed.matches_trigger("VariableUpdate", Some("deploy.request")));
        assert!(parsed.matches_trigger("VariableUpdate", Some("ci.build.42")));
        assert!(!parsed.matches_trigger("Commit", Some("deploy.request")));
        assert!(!parsed.matches_trigger("VariableUpdate", Some("other.key")));
    }
}
