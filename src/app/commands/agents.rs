//! `/agent`, `/thread`, and `/memory` command handlers — persona
//! management, conversation threads, and memory search.

use super::super::App;
use super::super::agents::Agent;
use super::super::log_src;
use super::super::logging::LogLevel;

// ── /agent ───────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_agent_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.list_agents();
            return;
        }
        match args[0] {
            "use" | "switch" => {
                if let Some(name) = args.get(1) {
                    self.switch_agent(name);
                } else {
                    log_src!(self, LogLevel::Warn, "Usage: /agent use <name>".to_string());
                }
            }
            "create" | "new" => {
                if args.len() >= 3 {
                    let name = args[1];
                    let description = args[2..].join(" ");
                    self.create_agent(name, &description);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /agent create <name> <description>".to_string()
                    );
                }
            }
            "delete" | "remove" => {
                if let Some(name) = args.get(1) {
                    self.delete_agent(name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /agent delete <name>".to_string()
                    );
                }
            }
            "info" => self.show_agent_info(),
            _ => self.list_agents(),
        }
    }

    fn list_agents(&mut self) {
        self.log(LogLevel::Info, "Available personas:".to_string());
        let default = Agent::default();
        let marker = if self.active_agent.name == "memini" {
            " (active)"
        } else {
            ""
        };
        self.log(
            LogLevel::Info,
            format!("  {} -- {}{marker}", default.name, default.description),
        );
        let agents = self.custom_agents.clone();
        let active_name = self.active_agent.name.clone();
        for agent in &agents {
            let marker = if agent.name == active_name {
                " (active)"
            } else {
                ""
            };
            self.log(
                LogLevel::Info,
                format!("  {} -- {}{marker}", agent.name, agent.description),
            );
        }
    }

    fn switch_agent(&mut self, name: &str) {
        let agent = if name == "memini" {
            Agent::default()
        } else if let Some(a) = self.custom_agents.iter().find(|a| a.name == name).cloned() {
            a
        } else {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Unknown agent: {name}. Use /agent to see available agents.")
            );
            return;
        };

        // Clear conversation thread when switching agents.
        self.conversation_thread.clear();
        if let Err(err) = self.runtime.block_on(self.rice.clear_thread()) {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Thread clear failed: {err:#}")
            );
        }

        self.active_agent = agent.clone();
        if let Err(err) = self
            .runtime
            .block_on(self.rice.save_active_agent_name(&agent.name))
        {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Failed to persist agent: {err:#}")
            );
        }

        self.log(
            LogLevel::Info,
            format!(
                "Switched to persona: {} -- {}",
                agent.name, agent.description
            ),
        );
    }

    fn create_agent(&mut self, name: &str, description: &str) {
        if name == "memini" {
            log_src!(
                self,
                LogLevel::Warn,
                "Cannot override the built-in 'memini' agent.".to_string()
            );
            return;
        }
        if self.custom_agents.iter().any(|a| a.name == name) {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Agent '{name}' already exists. Delete it first.")
            );
            return;
        }

        let persona = crate::prompts::custom_persona(name, description);
        let agent = Agent {
            name: name.to_string(),
            description: description.to_string(),
            persona,
        };
        self.custom_agents.push(agent);

        let agents_json =
            serde_json::to_value(&self.custom_agents).unwrap_or(serde_json::Value::Array(vec![]));
        if let Err(err) = self
            .runtime
            .block_on(self.rice.save_custom_agents(agents_json))
        {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Failed to save agents: {err:#}")
            );
        }

        self.log(
            LogLevel::Info,
            format!("\u{2728} Agent '{name}' created! Use /agent use {name} to switch."),
        );
    }

    fn delete_agent(&mut self, name: &str) {
        if name == "memini" {
            log_src!(
                self,
                LogLevel::Warn,
                "Cannot delete the built-in 'memini' agent.".to_string()
            );
            return;
        }

        let before = self.custom_agents.len();
        self.custom_agents.retain(|a| a.name != name);
        if self.custom_agents.len() == before {
            log_src!(self, LogLevel::Warn, format!("Agent '{name}' not found."));
            return;
        }

        // If deleting the active agent, switch back to default.
        if self.active_agent.name == name {
            self.active_agent = Agent::default();
            self.conversation_thread.clear();
            let _ = self.runtime.block_on(self.rice.clear_thread());
            let _ = self
                .runtime
                .block_on(self.rice.save_active_agent_name("memini"));
        }

        let agents_json =
            serde_json::to_value(&self.custom_agents).unwrap_or(serde_json::Value::Array(vec![]));
        if let Err(err) = self
            .runtime
            .block_on(self.rice.save_custom_agents(agents_json))
        {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Failed to save agents: {err:#}")
            );
        }

        self.log(LogLevel::Info, format!("Persona '{name}' deleted."));
    }

    fn show_agent_info(&mut self) {
        let name = self.active_agent.name.clone();
        let description = self.active_agent.description.clone();
        let thread_len = self.conversation_thread.len();
        self.log(LogLevel::Info, format!("Active persona: {name}"));
        self.log(LogLevel::Info, format!("   Description: {description}"));
        self.log(LogLevel::Info, format!("   Thread: {thread_len} messages"));
    }
}

// ── /thread ──────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_thread_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_thread_info();
            return;
        }
        match args[0] {
            "clear" | "reset" => self.clear_thread(),
            _ => self.show_thread_info(),
        }
    }

    fn show_thread_info(&mut self) {
        let count = self.conversation_thread.len();
        let turns = count / 2;
        self.log(
            LogLevel::Info,
            format!(
                "Conversation: {count} messages ({turns} turns) | Persona: {}",
                self.active_agent.name
            ),
        );
        if count == 0 {
            self.log(
                LogLevel::Info,
                "   Thread is empty. Start chatting to build context.".to_string(),
            );
        }
    }

    fn clear_thread(&mut self) {
        self.conversation_thread.clear();
        if let Err(err) = self.runtime.block_on(self.rice.clear_thread()) {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Thread clear failed: {err:#}")
            );
        }
        self.log(LogLevel::Info, "Conversation cleared.".to_string());
    }
}

// ── /memory ──────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_memory_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.log(LogLevel::Info, "Usage: /memory <search query>".to_string());
            return;
        }
        let query = args.join(" ");
        self.search_memory(&query);
    }

    fn search_memory(&mut self, query: &str) {
        let memories =
            match self
                .runtime
                .block_on(self.rice.reminisce(vec![], self.memory_limit, query))
            {
                Ok(traces) => traces,
                Err(err) => {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        format!("Memory search failed: {err:#}")
                    );
                    return;
                }
            };

        if memories.is_empty() {
            self.log(LogLevel::Info, "No matching memories found.".to_string());
            return;
        }

        self.log(
            LogLevel::Info,
            format!("Found {} memory(ies):", memories.len()),
        );
        for trace in &memories {
            let input = trace.input.trim();
            let outcome = trace.outcome.trim();
            let action = trace.action.trim();
            if input.is_empty() && outcome.is_empty() {
                continue;
            }
            if action.is_empty() {
                self.log(
                    LogLevel::Info,
                    format!("  \u{21B3} {input} \u{2192} {outcome}"),
                );
            } else {
                self.log(
                    LogLevel::Info,
                    format!("  \u{21B3} [{action}] {input} \u{2192} {outcome}"),
                );
            }
        }
    }
}
