//! `/openai`, `/key`, and `/rice` command handlers, plus bootstrap
//! loaders that restore persisted keys on startup.

use std::env;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::constants::{
    ACTIVE_MCP_VAR, OPENAI_KEY_VAR, OPENAI_MODEL_VAR, OPENAI_REASONING_EFFORT_VAR,
};
use crate::mcp::config::McpServer;
use crate::openai::parse_reasoning_setting;
use crate::rice::RiceStatus;

use super::super::App;
use super::super::log_src;
use super::super::logging::{LogLevel, mask_key};

// ── /openai ──────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_openai_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_openai_status();
            return;
        }

        match args[0] {
            "set" | "key" => {
                if let Some(key) = args.get(1) {
                    self.persist_openai_key(key);
                } else {
                    log_src!(self, LogLevel::Warn, "Usage: /openai set <key>".to_string());
                }
            }
            "clear" => self.clear_openai_key(),
            "import-env" => self.import_openai_env(),
            other => log_src!(
                self,
                LogLevel::Warn,
                format!("Unknown /openai command: {other}")
            ),
        }
    }

    pub(crate) fn handle_key_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_openai_status();
            return;
        }
        self.persist_openai_key(args[0]);
    }

    fn show_openai_status(&mut self) {
        match &self.openai_key_hint {
            Some(hint) => self.log(LogLevel::Info, format!("OpenAI key stored ({hint}).")),
            None => self.log(LogLevel::Info, "OpenAI key not set.".to_string()),
        }
    }

    /// Store an OpenAI key in Rice and update local state.
    pub(crate) fn persist_openai_key(&mut self, key: &str) {
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            OPENAI_KEY_VAR,
            Value::String(key.to_string()),
            "explicit",
        )) {
            log_src!(
                self,
                LogLevel::Error,
                format!("Failed to store OpenAI key: {err:#}")
            );
            return;
        }

        self.openai_key = Some(key.to_string());
        self.openai_key_hint = Some(mask_key(key));
        self.log(LogLevel::Info, "OpenAI key stored in Rice.".to_string());
    }

    fn clear_openai_key(&mut self) {
        if let Err(err) = self
            .runtime
            .block_on(self.rice.delete_variable(OPENAI_KEY_VAR))
        {
            log_src!(
                self,
                LogLevel::Error,
                format!("Failed to delete key: {err:#}")
            );
            return;
        }

        self.openai_key = None;
        self.openai_key_hint = None;
        self.log(LogLevel::Info, "OpenAI key removed.".to_string());
    }

    fn import_openai_env(&mut self) {
        match env::var("OPENAI_API_KEY") {
            Ok(key) => self.persist_openai_key(&key),
            Err(_) => log_src!(self, LogLevel::Warn, "OPENAI_API_KEY not set.".to_string()),
        }
    }

    pub(crate) fn handle_model_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_model_status();
            return;
        }

        match args[0] {
            "list" => self.show_model_guide(),
            "set" | "use" => {
                if let Some(model) = args.get(1) {
                    self.persist_openai_model(model);
                } else {
                    log_src!(self, LogLevel::Warn, "Usage: /model set <name>".to_string());
                }
            }
            "thinking" => {
                if let Some(mode) = args.get(1) {
                    self.persist_thinking_setting(mode);
                } else {
                    self.show_model_status();
                    self.log(
                        LogLevel::Info,
                        "Usage: /model thinking <on|off|low|medium|high>".to_string(),
                    );
                }
            }
            "help" => self.show_model_help(),
            maybe_model => {
                // Shortcut: `/model gpt-5-mini`
                self.persist_openai_model(maybe_model);
            }
        }
    }

    fn show_model_status(&mut self) {
        let thinking = self
            .openai
            .reasoning_effort
            .as_deref()
            .map(|effort| format!("on ({effort})"))
            .unwrap_or_else(|| "off".to_string());
        self.log(
            LogLevel::Info,
            format!(
                "Model: {} | Thinking: {thinking}",
                self.openai.model.as_str()
            ),
        );
        self.log(
            LogLevel::Info,
            "Use /model list for guidance, /model set <name>, /model thinking <mode>.".to_string(),
        );
    }

    fn show_model_guide(&mut self) {
        self.log(
            LogLevel::Info,
            "Model picks (starting points; tune by your latency/cost needs):".to_string(),
        );
        self.log(
            LogLevel::Info,
            "  gpt-5         -- best quality for complex planning/coding".to_string(),
        );
        self.log(
            LogLevel::Info,
            "  gpt-5-mini    -- strong quality with faster turn-around".to_string(),
        );
        self.log(
            LogLevel::Info,
            "  gpt-4o        -- versatile multimodal balance".to_string(),
        );
        self.log(
            LogLevel::Info,
            "  gpt-4o-mini   -- lowest-cost/faster iterative runs".to_string(),
        );
        self.log(
            LogLevel::Info,
            "Thinking mode: /model thinking on|off|low|medium|high".to_string(),
        );
        self.log(
            LogLevel::Info,
            "Examples: /model set gpt-5-mini  |  /model thinking medium".to_string(),
        );
    }

    fn show_model_help(&mut self) {
        self.log(LogLevel::Info, "Model commands:".to_string());
        self.log(LogLevel::Info, "  /model".to_string());
        self.log(LogLevel::Info, "  /model list".to_string());
        self.log(LogLevel::Info, "  /model set <name>".to_string());
        self.log(
            LogLevel::Info,
            "  /model thinking <on|off|low|medium|high>".to_string(),
        );
        self.log(
            LogLevel::Info,
            "  /model <name>   (shortcut for set)".to_string(),
        );
    }

    fn persist_openai_model(&mut self, model: &str) {
        let model = model.trim();
        if model.is_empty() {
            log_src!(
                self,
                LogLevel::Warn,
                "Model name cannot be empty.".to_string()
            );
            return;
        }

        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            OPENAI_MODEL_VAR,
            Value::String(model.to_string()),
            "explicit",
        )) {
            log_src!(
                self,
                LogLevel::Error,
                format!("Failed to store model preference: {err:#}")
            );
            return;
        }

        self.openai.model = model.to_string();
        self.log(
            LogLevel::Info,
            format!("Active model set to '{}'.", self.openai.model),
        );
    }

    fn persist_thinking_setting(&mut self, raw: &str) {
        let parsed = parse_reasoning_setting(raw);
        let Some(setting) = parsed else {
            log_src!(
                self,
                LogLevel::Warn,
                "Invalid thinking mode. Use on|off|low|medium|high.".to_string()
            );
            return;
        };

        match setting {
            Some(effort) => {
                if let Err(err) = self.runtime.block_on(self.rice.set_variable(
                    OPENAI_REASONING_EFFORT_VAR,
                    Value::String(effort.clone()),
                    "explicit",
                )) {
                    log_src!(
                        self,
                        LogLevel::Error,
                        format!("Failed to store thinking mode: {err:#}")
                    );
                    return;
                }
                self.openai.reasoning_effort = Some(effort.clone());
                self.log(
                    LogLevel::Info,
                    format!("Thinking enabled (effort: {effort})."),
                );
            }
            None => {
                if let Err(err) = self
                    .runtime
                    .block_on(self.rice.delete_variable(OPENAI_REASONING_EFFORT_VAR))
                {
                    log_src!(
                        self,
                        LogLevel::Error,
                        format!("Failed to clear thinking mode: {err:#}")
                    );
                    return;
                }
                self.openai.reasoning_effort = None;
                self.log(LogLevel::Info, "Thinking disabled.".to_string());
            }
        }
    }
}

// ── /rice ────────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_rice_command(&mut self, args: Vec<&str>) {
        if !args.is_empty() && args[0] == "setup" {
            self.start_rice_setup();
            return;
        }

        match &self.rice.status {
            RiceStatus::Connected => {
                self.log(LogLevel::Info, "🟢 Rice is connected.".to_string());
                self.log(
                    LogLevel::Info,
                    format!("   Run ID: {}", self.rice.active_run_id()),
                );
            }
            RiceStatus::Disabled(reason) => {
                log_src!(self, LogLevel::Warn, format!("Rice disabled: {reason}"));
                self.log(
                    LogLevel::Info,
                    "Run /rice setup to configure Rice interactively.".to_string(),
                );
            }
        };
    }
}

// ── Bootstrap loaders ────────────────────────────────────────────────

impl App {
    /// Load the persisted OpenAI key from Rice (or fall back to env).
    pub(crate) fn load_openai_from_rice(&mut self) -> Result<()> {
        let value = self
            .runtime
            .block_on(self.rice.get_variable(OPENAI_KEY_VAR))?;
        if let Some(Value::String(key)) = value {
            self.openai_key = Some(key.clone());
            self.openai_key_hint = Some(mask_key(&key));
            return Ok(());
        }

        if let Ok(key) = env::var("OPENAI_API_KEY") {
            self.log(
                LogLevel::Info,
                "OPENAI_API_KEY found in env; storing in Rice.".to_string(),
            );
            self.persist_openai_key(&key);
        }

        Ok(())
    }

    /// Restore the last-used MCP server from Rice.
    pub(crate) fn load_active_mcp_from_rice(&mut self) -> Result<()> {
        let value = self
            .runtime
            .block_on(self.rice.get_variable(ACTIVE_MCP_VAR))?;
        if let Some(value) = value {
            let server: McpServer =
                serde_json::from_value(value).context("decode active MCP from Rice")?;
            self.active_mcp = Some(server);
        }
        Ok(())
    }

    /// Restore persisted model and thinking settings from Rice.
    pub(crate) fn load_openai_model_settings_from_rice(&mut self) -> Result<()> {
        let model_value = self
            .runtime
            .block_on(self.rice.get_variable(OPENAI_MODEL_VAR))?;
        if let Some(Value::String(model)) = model_value {
            let model = model.trim();
            if !model.is_empty() {
                self.openai.model = model.to_string();
                self.log(
                    LogLevel::Info,
                    format!("Loaded model preference: {}", self.openai.model),
                );
            }
        }

        let thinking_value = self
            .runtime
            .block_on(self.rice.get_variable(OPENAI_REASONING_EFFORT_VAR))?;
        if let Some(Value::String(raw)) = thinking_value {
            match parse_reasoning_setting(&raw) {
                Some(setting) => {
                    self.openai.reasoning_effort = setting;
                }
                None => {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        format!(
                            "Ignored invalid stored thinking mode '{raw}'. Use /model thinking ..."
                        )
                    );
                }
            }
        }

        Ok(())
    }
}
