//! Shared prompt builders for orchestrator + worker agents.
//!
//! Prompt templates are sourced from Markdown files in this order:
//! 1. `$MEMINI_PROMPTS_DIR/*.md` (if set)
//! 2. `$MEMINI_HOME/prompts/*.md` (defaults to `~/Memini/prompts`)
//! 3. Bundled repository defaults in `./prompts/*.md`

use std::env;
use std::fs;
use std::path::PathBuf;

use directories::BaseDirs;

const DEFAULT_MEMINI_PERSONA_MD: &str = include_str!("../prompts/default_memini_persona.md");
const EXECUTION_STYLE_MD: &str = include_str!("../prompts/execution_style.md");
const ORCHESTRATION_RULES_MD: &str = include_str!("../prompts/orchestration_rules.md");
const NEEDS_INPUT_RULE_MD: &str = include_str!("../prompts/needs_input_rule.md");
const DAEMON_BRIEFING_PERSONA_MD: &str = include_str!("../prompts/daemon_briefing_persona.md");
const DAEMON_BRIEFING_PROMPT_MD: &str = include_str!("../prompts/daemon_briefing_prompt.md");
const DAEMON_DIGEST_PERSONA_MD: &str = include_str!("../prompts/daemon_digest_persona.md");
const DAEMON_DIGEST_PROMPT_MD: &str = include_str!("../prompts/daemon_digest_prompt.md");

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

fn prompt_override_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(value) = env::var("MEMINI_PROMPTS_DIR") {
        if !value.trim().is_empty() {
            dirs.push(PathBuf::from(value));
        }
    }
    dirs.push(memini_home().join("prompts"));
    dirs
}

fn load_prompt(file_name: &str, bundled: &str) -> String {
    for dir in prompt_override_dirs() {
        let path = dir.join(file_name);
        if !path.is_file() {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    bundled.trim().to_string()
}

pub fn default_memini_persona() -> String {
    load_prompt("default_memini_persona.md", DEFAULT_MEMINI_PERSONA_MD)
}

pub fn daemon_briefing_persona() -> String {
    load_prompt("daemon_briefing_persona.md", DAEMON_BRIEFING_PERSONA_MD)
}

pub fn daemon_briefing_prompt() -> String {
    load_prompt("daemon_briefing_prompt.md", DAEMON_BRIEFING_PROMPT_MD)
}

pub fn daemon_digest_persona() -> String {
    load_prompt("daemon_digest_persona.md", DAEMON_DIGEST_PERSONA_MD)
}

pub fn daemon_digest_prompt() -> String {
    load_prompt("daemon_digest_prompt.md", DAEMON_DIGEST_PROMPT_MD)
}

pub fn custom_persona(name: &str, description: &str) -> String {
    format!(
        "You are {name}, a specialized execution-first AI assistant. \
         {description} You have long-term memory and should deliver concrete, \
         actionable outcomes with minimal back-and-forth."
    )
}

pub fn main_chat_system_prompt(persona: &str, now: &str, require_mcp: bool) -> String {
    let tools_line = if require_mcp {
        "Use connected tools through delegated agents whenever tools are needed."
    } else {
        "Use memory context and delegated agents to complete tasks autonomously."
    };
    let execution_style = load_prompt("execution_style.md", EXECUTION_STYLE_MD);
    let orchestration_rules = load_prompt("orchestration_rules.md", ORCHESTRATION_RULES_MD);

    format!(
        "{persona}\nCurrent date and time: {now}.\n{tools_line}\n\n{execution_style}\n\n{orchestration_rules}"
    )
}

pub fn worker_system_prompt(persona: &str, now: &str, has_tools: bool) -> String {
    let tools_line = if has_tools {
        "You have tool access. Use tools proactively to complete the task fully."
    } else {
        "You may have limited tool access. Still produce final, ready-to-apply outputs."
    };
    let execution_style = load_prompt("execution_style.md", EXECUTION_STYLE_MD);
    let needs_input_rule = load_prompt("needs_input_rule.md", NEEDS_INPUT_RULE_MD);

    format!(
        "{persona}\nCurrent date and time: {now}.\nYou are a delegated worker agent in a CLI workflow.\n{tools_line}\n\n{execution_style}\n\n{needs_input_rule}"
    )
}
