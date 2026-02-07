//! AI chat flow — memory recall, OpenAI tool loops, and Rice trace commits.

use std::env;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::constants::OPENAI_KEY_VAR;
use crate::mcp;
use crate::openai::{
    ToolCall, extract_output_items, extract_output_text, extract_tool_calls,
    tool_loop_limit_reached,
};
use crate::rice::{agent_id_for, format_memories, system_prompt};

use super::App;
use super::daemon;
use super::log_src;
use super::logging::{LogLevel, mask_key};

impl App {
    /// Run a full chat turn: recall → LLM → tool loops → commit → persist thread.
    pub(crate) fn handle_chat_message(&mut self, message: &str, require_mcp: bool) {
        let key = match self.ensure_openai_key() {
            Ok(k) => k,
            Err(err) => {
                log_src!(self, LogLevel::Error, format!("OpenAI key missing: {err}"));
                self.log(
                    LogLevel::Info,
                    "Use /openai set <key> or /key <key> to configure.".to_string(),
                );
                return;
            }
        };

        if require_mcp && self.mcp_connections.is_empty() {
            log_src!(self, LogLevel::Warn, "No MCP connections.".to_string());
            return;
        }

        // Focus Rice on the current message.
        if let Err(err) = self.runtime.block_on(self.rice.focus(message)) {
            log_src!(self, LogLevel::Warn, format!("Rice focus failed: {err:#}"));
        }

        // Recall relevant memories (Rice computes embeddings server-side).
        let memories =
            match self
                .runtime
                .block_on(self.rice.reminisce(vec![], self.memory_limit, message))
            {
                Ok(traces) => traces,
                Err(err) => {
                    log_src!(self, LogLevel::Warn, format!("Rice recall failed: {err:#}"));
                    Vec::new()
                }
            };

        if !memories.is_empty() {
            self.log(
                LogLevel::Info,
                format!("Rice recalled {} related memory(ies).", memories.len()),
            );
        }

        // Build LLM input: system prompt + memories + conversation thread + new message.
        let memory_context = format_memories(&memories);
        let mut input = Vec::new();
        input.push(json!({"role": "system", "content": system_prompt(&self.active_agent.persona, require_mcp)}));
        if !memory_context.is_empty() {
            input.push(json!({"role": "system", "content": memory_context}));
        }

        // Include conversation thread (previous turns give multi-turn context).
        for msg in &self.conversation_thread {
            input.push(msg.clone());
        }

        // New user message.
        input.push(json!({"role": "user", "content": message}));

        let mut tools = match self.openai_tools_for_mcp(require_mcp) {
            Ok(t) => t,
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Error,
                    format!("Failed to load MCP tools: {err:#}")
                );
                return;
            }
        };

        // Always inject the built-in spawn_agent tool so the LLM can
        // delegate sub-tasks to parallel agent windows.
        let spawn_tool = json!({
            "type": "function",
            "name": "spawn_agent",
            "description": "Spawn an independent sub-agent that runs in its own window in the user's grid layout. Use this to delegate sub-tasks that can run in parallel. Each agent gets its own memory context and streams output in real time.",
            "parameters": {
                "type": "object",
                "properties": {
                    "label": {
                        "type": "string",
                        "description": "Short name for the agent window (e.g. 'Research', 'Code Review', 'Summarizer')"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The detailed task prompt for the sub-agent. Be specific about what it should do."
                    }
                },
                "required": ["label", "prompt"]
            }
        });
        tools.get_or_insert_with(Vec::new).push(spawn_tool);

        // Initial LLM request.
        let mut response =
            match self
                .runtime
                .block_on(self.openai.response(&key, &input, tools.as_deref()))
            {
                Ok(r) => r,
                Err(err) => {
                    log_src!(
                        self,
                        LogLevel::Error,
                        format!("OpenAI request failed: {err:#}")
                    );
                    return;
                }
            };

        let mut output_items = extract_output_items(&response);
        if !output_items.is_empty() {
            input.extend(output_items.clone());
        }
        let mut output_text = extract_output_text(&output_items);
        let mut tool_calls = extract_tool_calls(&output_items);
        let mut tool_loops = 0usize;

        // Tool-call loop.
        while !tool_calls.is_empty() {
            if tool_loop_limit_reached(tool_loops) {
                log_src!(self, LogLevel::Warn, "Tool loop limit reached.".to_string());
                break;
            }
            tool_loops += 1;

            for call in tool_calls {
                self.log(LogLevel::Info, format!("Calling tool: {}", call.name));
                let tool_output = if call.name == "spawn_agent" {
                    self.handle_spawn_agent_tool(&call)
                } else {
                    match self.call_mcp_tool_value(&call.name, call.arguments) {
                        Ok(value) => {
                            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
                        }
                        Err(err) => format!("{{\"error\":\"{err}\"}}"),
                    }
                };
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call.call_id,
                    "output": tool_output
                }));
            }

            response =
                match self
                    .runtime
                    .block_on(self.openai.response(&key, &input, tools.as_deref()))
                {
                    Ok(r) => r,
                    Err(err) => {
                        log_src!(
                            self,
                            LogLevel::Error,
                            format!("OpenAI request failed: {err:#}")
                        );
                        break;
                    }
                };
            output_items = extract_output_items(&response);
            if !output_items.is_empty() {
                input.extend(output_items.clone());
            }
            output_text = extract_output_text(&output_items);
            tool_calls = extract_tool_calls(&output_items);
        }

        // Display result.
        if output_text.is_empty() {
            log_src!(self, LogLevel::Warn, "No response received.".to_string());
        } else {
            let label = format!("{}", self.active_agent.name);
            self.log_markdown(label, output_text.clone());
        }

        // Update conversation thread with this turn.
        self.conversation_thread
            .push(json!({"role": "user", "content": message}));
        if !output_text.is_empty() {
            self.conversation_thread
                .push(json!({"role": "assistant", "content": output_text}));
        }

        // Trim thread if over limit.
        let max = crate::constants::MAX_THREAD_MESSAGES;
        while self.conversation_thread.len() > max {
            self.conversation_thread.drain(0..2);
        }

        // Persist thread to Rice.
        if let Err(err) = self
            .runtime
            .block_on(self.rice.save_thread(&self.conversation_thread))
        {
            log_src!(self, LogLevel::Warn, format!("Thread save failed: {err:#}"));
        }

        // Commit trace to Rice long-term memory.
        let aid = agent_id_for(&self.active_agent.name);
        if let Err(err) = self.runtime.block_on(self.rice.commit_trace(
            message,
            &output_text,
            "chat",
            vec![],
            &aid,
        )) {
            log_src!(self, LogLevel::Warn, format!("Rice commit failed: {err:#}"));
        }
    }

    /// Build the OpenAI-compatible tool definitions from the active MCP connection.
    fn openai_tools_for_mcp(&mut self, require_mcp: bool) -> Result<Option<Vec<Value>>> {
        if self.mcp_connections.is_empty() {
            if require_mcp {
                return Err(anyhow!("No active MCP connection"));
            }
            return Ok(None);
        }

        let mut tool_warnings = Vec::new();
        let server_ids: Vec<String> = self.mcp_connections.keys().cloned().collect();
        for id in &server_ids {
            let refresh_result = {
                let Some(connection) = self.mcp_connections.get_mut(id) else {
                    continue;
                };
                if connection.tool_cache.is_empty() {
                    Some(self.runtime.block_on(mcp::refresh_tools(connection)))
                } else {
                    None
                }
            };
            if let Some(Err(err)) = refresh_result {
                tool_warnings.push(format!("MCP tools refresh failed for {id}: {err:#}"));
            }
        }
        for line in tool_warnings {
            log_src!(self, LogLevel::Warn, line);
        }

        let mut openai_tools = Vec::new();
        for id in server_ids {
            let Some(connection) = self.mcp_connections.get(&id) else {
                continue;
            };
            if connection.tool_cache.is_empty() {
                continue;
            }
            openai_tools.extend(mcp::tools_to_openai_namespaced(
                &connection.server,
                &connection.tool_cache,
            )?);
        }

        if openai_tools.is_empty() {
            if require_mcp {
                return Err(anyhow!("MCP connected but no tools available"));
            }
            return Ok(None);
        }

        Ok(Some(openai_tools))
    }

    /// Ensure an OpenAI API key is available, loading from Rice or env if needed.
    fn ensure_openai_key(&mut self) -> Result<String> {
        if let Some(key) = &self.openai_key {
            return Ok(key.clone());
        }

        if let Ok(Some(Value::String(key))) = self
            .runtime
            .block_on(self.rice.get_variable(OPENAI_KEY_VAR))
        {
            self.openai_key_hint = Some(mask_key(&key));
            self.openai_key = Some(key.clone());
            return Ok(key);
        }

        if let Ok(key) = env::var("OPENAI_API_KEY") {
            self.persist_openai_key(&key);
            return Ok(key);
        }

        Err(anyhow!("OpenAI key not configured"))
    }

    /// Handle the built-in `spawn_agent` tool call from the LLM.
    /// Creates an agent window and spawns the background task, returning
    /// a JSON status string to feed back to the model.
    fn handle_spawn_agent_tool(&mut self, call: &ToolCall) -> String {
        let label = call
            .arguments
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("Sub-Agent")
            .to_string();
        let prompt = call
            .arguments
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if prompt.is_empty() {
            return r#"{"error":"prompt is required"}"#.to_string();
        }

        let window_id = self.next_window_id;
        self.next_window_id += 1;

        // Create the window in Thinking state so it appears in the grid.
        let window = daemon::AgentWindow {
            id: window_id,
            label: label.clone(),
            prompt: prompt.clone(),
            status: daemon::AgentWindowStatus::Thinking,
            output_lines: Vec::new(),
            pending_question: None,
            scroll: 0,
        };
        self.agent_windows.push(window);

        // Select the new agent in the grid.
        let idx = self.agent_windows.len().saturating_sub(1);
        self.grid_selected = idx;

        // Spawn the background task.
        let tx = self.daemon_tx.clone();
        let openai = self.openai.clone();
        let key = self.openai_key.clone();
        let rice_handle = self.runtime.spawn(crate::rice::RiceStore::connect());
        let persona = self.active_agent.persona.clone();

        daemon::spawn_agent_window(
            window_id,
            persona,
            prompt,
            tx,
            openai,
            key,
            rice_handle,
            self.runtime.handle().clone(),
        );

        self.log(
            LogLevel::Info,
            format!("LLM spawned agent: {label} (window #{window_id})"),
        );

        format!(
            r#"{{"status":"spawned","window_id":{window_id},"label":"{}"}}"#,
            label
        )
    }
}
