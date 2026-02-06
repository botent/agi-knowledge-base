use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Local;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use directories::ProjectDirs;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::constants::{ACTIVE_MCP_VAR, APP_NAME, DEFAULT_MEMORY_LIMIT, MAX_LOGS, OPENAI_KEY_VAR};
use crate::mcp::config::{McpConfig, McpServer, McpSource};
use crate::mcp::{self, McpConnection};
use crate::openai::{
    OpenAiClient, extract_output_items, extract_output_text, extract_tool_calls, format_json,
    tool_loop_limit_reached,
};
use crate::rice::{RiceStatus, RiceStore, agent_id, format_memories, system_prompt};
use crate::util::env_first;

pub struct App {
    runtime: Runtime,
    input: String,
    cursor: usize,
    logs: Vec<LogLine>,
    mcp_config: McpConfig,
    mcp_source: McpSource,
    active_mcp: Option<McpServer>,
    mcp_connection: Option<McpConnection>,
    local_mcp_store: LocalMcpStore,
    rice: RiceStore,
    openai_key_hint: Option<String>,
    openai_key: Option<String>,
    openai: OpenAiClient,
    memory_limit: u64,
    pending_oauth: Option<(String, mcp::oauth::PendingOAuth)>,
    should_quit: bool,
}

impl App {
    pub fn new() -> Result<Self> {
        let runtime = Runtime::new().context("create tokio runtime")?;
        let (mcp_config, mcp_source) = McpConfig::load()?;
        let local_mcp_store = load_local_mcp_store();
        let rice = runtime.block_on(RiceStore::connect());
        let memory_limit = env_first(&["MEMINI_MEMORY_LIMIT"])
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MEMORY_LIMIT);

        let mut app = App {
            runtime,
            input: String::new(),
            cursor: 0,
            logs: Vec::new(),
            mcp_config,
            mcp_source,
            active_mcp: None,
            mcp_connection: None,
            local_mcp_store,
            rice,
            openai_key_hint: None,
            openai_key: None,
            openai: OpenAiClient::new(),
            memory_limit,
            pending_oauth: None,
            should_quit: false,
        };

        app.log(
            LogLevel::Info,
            format!(
                "Loaded {} MCP server(s) from {}.",
                app.mcp_config.servers.len(),
                app.mcp_source.label()
            ),
        );

        app.log(
            LogLevel::Info,
            "Type /help for commands. /mcp shows available servers.".to_string(),
        );

        app.bootstrap();
        Ok(app)
    }

    fn bootstrap(&mut self) {
        if let Err(err) = self.load_openai_from_rice() {
            self.log(LogLevel::Warn, format!("OpenAI key load skipped: {err}"));
        }

        if let Err(err) = self.load_active_mcp_from_rice() {
            self.log(LogLevel::Warn, format!("Active MCP load skipped: {err}"));
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(frame.size());

        let header_line = Line::from(vec![
            Span::styled("MCP: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.mcp_status_label(),
                Style::default().fg(self.mcp_status_color()),
            ),
            Span::styled("  OpenAI: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.openai_status_label(),
                Style::default().fg(self.openai_status_color()),
            ),
            Span::styled("  Rice: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.rice.status_label(),
                Style::default().fg(self.rice_status_color()),
            ),
        ]);

        let header = Paragraph::new(header_line);
        frame.render_widget(header, chunks[0]);

        let log_height = chunks[1].height as usize;
        let start = self.logs.len().saturating_sub(log_height);
        let lines: Vec<Line> = self.logs[start..]
            .iter()
            .map(|line| line.render())
            .collect();
        let log_text = Text::from(lines);

        let log_panel = Paragraph::new(log_text)
            .block(Block::default().borders(Borders::ALL).title("Activity"))
            .wrap(Wrap { trim: true });
        frame.render_widget(log_panel, chunks[1]);

        let input_panel = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Command"));
        frame.render_widget(input_panel, chunks[2]);

        let input_width = chunks[2].width.saturating_sub(2) as usize;
        let cursor = self.cursor.min(input_width);
        let cursor_x = chunks[2].x + 1 + cursor as u16;
        let cursor_y = chunks[2].y + 1;
        frame.set_cursor(cursor_x, cursor_y);
    }

    pub fn handle_event(&mut self, event: Event) -> Result<()> {
        if let Event::Key(key) = event {
            self.handle_key(key)?;
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        match key {
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.should_quit = true;
            }
            KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.logs.clear();
            }
            KeyEvent { code, .. } => match code {
                KeyCode::Char(ch) => self.insert_char(ch),
                KeyCode::Backspace => self.backspace(),
                KeyCode::Delete => self.delete(),
                KeyCode::Left => self.move_cursor_left(),
                KeyCode::Right => self.move_cursor_right(),
                KeyCode::Home => self.move_cursor_home(),
                KeyCode::End => self.move_cursor_end(),
                KeyCode::Enter => self.submit_input()?,
                KeyCode::Esc => self.should_quit = true,
                _ => {}
            },
        }
        Ok(())
    }

    fn insert_char(&mut self, ch: char) {
        if !ch.is_ascii() {
            return;
        }
        self.input.insert(self.cursor, ch);
        self.cursor = (self.cursor + 1).min(self.input.len());
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        self.input.remove(self.cursor);
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        self.input.remove(self.cursor);
    }

    fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    fn submit_input(&mut self) -> Result<()> {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;

        if line.is_empty() {
            return Ok(());
        }

        if line.starts_with('/') {
            self.handle_command(&line)?;
        } else {
            self.handle_chat_message(&line, false);
        }

        Ok(())
    }

    fn handle_command(&mut self, line: &str) -> Result<()> {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");

        match cmd {
            "/help" => self.show_help(),
            "/quit" | "/exit" => self.should_quit = true,
            "/clear" => self.logs.clear(),
            "/mcp" => self.handle_mcp_command(parts.collect::<Vec<_>>()),
            "/openai" => self.handle_openai_command(parts.collect::<Vec<_>>()),
            "/key" => self.handle_key_command(parts.collect::<Vec<_>>()),
            "/rice" => self.handle_rice_command(),
            _ => self.log(LogLevel::Warn, format!("Unknown command: {cmd}")),
        }

        Ok(())
    }

    fn show_help(&mut self) {
        self.log(LogLevel::Info, "Commands:".to_string());
        self.log(
            LogLevel::Info,
            "(no slash)            Chat with OpenAI".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp                   List MCP servers".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp connect <id>      Set active MCP".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp auth <id>         Run OAuth flow".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp auth-code <id> <url-or-code>  Complete OAuth manually".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp status            Show active MCP".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp tools             List tools on active MCP".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp call <tool> <json>Call MCP tool with JSON args".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp ask <prompt>      Chat using MCP tools".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp disconnect        Close MCP connection".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp token <id> <tok>  Store bearer token in Rice".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp token-clear <id>  Remove stored bearer token".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/mcp reload            Reload MCP config".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/openai                Show OpenAI key status".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/openai set <key>      Persist OpenAI key in Rice".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/openai key <key>      Alias for set".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/key <key>             Set OpenAI key".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/openai clear          Remove OpenAI key".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/openai import-env     Store $OPENAI_API_KEY".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/rice                  Show Rice connection status".to_string(),
        );
        self.log(
            LogLevel::Info,
            "/clear                 Clear activity log".to_string(),
        );
        self.log(LogLevel::Info, "/quit                  Exit".to_string());
    }

    fn handle_mcp_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.list_mcp_servers();
            return;
        }

        match args[0] {
            "connect" | "use" => {
                if let Some(target) = args.get(1) {
                    self.connect_mcp(target);
                } else {
                    self.log(LogLevel::Warn, "Usage: /mcp connect <id>".to_string());
                }
            }
            "disconnect" => self.disconnect_mcp(),
            "status" => self.show_mcp_status(),
            "tools" => self.list_mcp_tools(),
            "call" => {
                let tool = args.get(1).copied();
                let rest = if args.len() > 2 { &args[2..] } else { &[] };
                self.call_mcp_tool(tool, rest);
            }
            "ask" => {
                if args.len() > 1 {
                    let prompt = args[1..].join(" ");
                    self.handle_chat_message(&prompt, true);
                } else {
                    self.log(LogLevel::Warn, "Usage: /mcp ask <prompt>".to_string());
                }
            }
            "auth" => {
                if let Some(target) = args.get(1) {
                    self.authenticate_mcp(target);
                } else {
                    self.log(LogLevel::Warn, "Usage: /mcp auth <id>".to_string());
                }
            }
            "auth-code" => {
                if args.len() >= 3 {
                    let id = args[1].to_string();
                    let code_or_url = args[2..].join(" ");
                    self.complete_oauth_manual(&id, &code_or_url);
                } else {
                    self.log(
                        LogLevel::Warn,
                        "Usage: /mcp auth-code <id> <url-or-code>".to_string(),
                    );
                }
            }
            "token" => {
                if args.len() >= 3 {
                    let id = args[1];
                    let token = args[2];
                    self.store_mcp_token(id, token);
                } else {
                    self.log(LogLevel::Warn, "Usage: /mcp token <id> <token>".to_string());
                }
            }
            "token-clear" => {
                if let Some(target) = args.get(1) {
                    self.clear_mcp_token(target);
                } else {
                    self.log(LogLevel::Warn, "Usage: /mcp token-clear <id>".to_string());
                }
            }
            "reload" => self.reload_mcp_config(),
            other => {
                self.log(LogLevel::Warn, format!("Unknown /mcp command: {other}"));
            }
        }
    }

    fn handle_openai_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_openai_status();
            return;
        }

        match args[0] {
            "set" | "key" => {
                if let Some(key) = args.get(1) {
                    self.persist_openai_key(key);
                } else {
                    self.log(LogLevel::Warn, "Usage: /openai set <key>".to_string());
                }
            }
            "clear" => self.clear_openai_key(),
            "import-env" => self.import_openai_env(),
            other => self.log(LogLevel::Warn, format!("Unknown /openai command: {other}")),
        }
    }

    fn handle_key_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_openai_status();
            return;
        }
        self.persist_openai_key(args[0]);
    }

    fn handle_rice_command(&mut self) {
        match &self.rice.status {
            RiceStatus::Connected => {
                self.log(LogLevel::Info, "Rice is connected.".to_string());
            }
            RiceStatus::Disabled(reason) => {
                self.log(LogLevel::Warn, format!("Rice disabled: {reason}"));
                self.log(
                    LogLevel::Info,
                    "Set STATE_INSTANCE_URL/STATE_AUTH_TOKEN in .env to enable Rice State."
                        .to_string(),
                );
            }
        };
    }

    fn list_mcp_servers(&mut self) {
        if self.mcp_config.servers.is_empty() {
            self.log(LogLevel::Warn, "No MCP servers configured.".to_string());
            return;
        }

        self.log(LogLevel::Info, "Available MCP servers:".to_string());
        let servers = self.mcp_config.servers.clone();
        for server in servers {
            let transport = server.transport.as_deref().unwrap_or("http");
            let auth = server
                .auth
                .as_ref()
                .map(|auth| auth.auth_type.as_str())
                .unwrap_or("none");
            self.log(
                LogLevel::Info,
                format!(
                    "- {} ({}) [transport: {transport}, auth: {auth}]",
                    server.display_name(),
                    server.url
                ),
            );
        }
    }

    fn connect_mcp(&mut self, target: &str) {
        let Some(server) = self.mcp_config.find_by_id_or_name(target) else {
            self.log(LogLevel::Warn, format!("Unknown MCP server: {target}"));
            return;
        };

        let transport = server.transport.as_deref().unwrap_or("http");
        if transport != "http" {
            self.log(
                LogLevel::Warn,
                format!("Transport '{transport}' not supported yet."),
            );
            return;
        }

        let bearer = self.resolve_mcp_token(&server);
        if bearer.is_none() {
            if let Some(auth) = &server.auth {
                if auth.auth_type == "oauth_browser" {
                    self.log(
                        LogLevel::Warn,
                        "No bearer token found. Run /mcp auth <id> for OAuth.".to_string(),
                    );
                }
            }
        }

        let connect_result = self
            .runtime
            .block_on(mcp::connect_http(&server, bearer.clone()));

        match connect_result {
            Ok(connection) => {
                self.active_mcp = Some(server.clone());
                self.mcp_connection = Some(connection);

                let store_result = self.runtime.block_on(self.rice.set_variable(
                    ACTIVE_MCP_VAR,
                    serde_json::to_value(&server).unwrap_or(Value::Null),
                    "explicit",
                ));

                if let Err(err) = store_result {
                    self.log(LogLevel::Warn, format!("Failed to persist MCP: {err:#}"));
                }

                let token_hint = bearer.as_ref().map(|token| mask_key(token));
                match token_hint {
                    Some(hint) => self.log(
                        LogLevel::Info,
                        format!("Connected to {} (auth {hint}).", server.display_name()),
                    ),
                    None => self.log(
                        LogLevel::Info,
                        format!("Connected to {}.", server.display_name()),
                    ),
                }

                self.list_mcp_tools();
            }
            Err(err) => {
                self.log(LogLevel::Error, format!("Failed to connect MCP: {err:#}"));
            }
        }
    }

    fn disconnect_mcp(&mut self) {
        if self.mcp_connection.is_some() {
            self.mcp_connection = None;
            self.log(LogLevel::Info, "MCP connection closed.".to_string());
        } else {
            self.log(LogLevel::Info, "No MCP connection to close.".to_string());
        }
    }

    fn list_mcp_tools(&mut self) {
        let tools_result = {
            let Some(connection) = self.mcp_connection.as_mut() else {
                self.log(LogLevel::Warn, "No active MCP connection.".to_string());
                return;
            };
            self.runtime.block_on(mcp::refresh_tools(connection))
        };

        match tools_result {
            Ok(tools) => {
                if tools.is_empty() {
                    self.log(LogLevel::Info, "No tools reported by MCP.".to_string());
                } else {
                    self.log(LogLevel::Info, "MCP tools:".to_string());
                    for tool in tools {
                        self.log(LogLevel::Info, format!("- {}", tool.name));
                    }
                }
            }
            Err(err) => {
                self.log(LogLevel::Error, format!("Failed to list tools: {err:#}"));
            }
        }
    }

    fn call_mcp_tool(&mut self, tool: Option<&str>, args: &[&str]) {
        let Some(tool) = tool else {
            self.log(LogLevel::Warn, "Usage: /mcp call <tool> <json>".to_string());
            return;
        };

        let tool_cache = match self.mcp_connection.as_ref() {
            Some(connection) => connection.tool_cache.clone(),
            None => {
                self.log(LogLevel::Warn, "No active MCP connection.".to_string());
                return;
            }
        };

        if !tool_cache.is_empty() && !tool_cache.iter().any(|t| t.name == tool) {
            self.log(
                LogLevel::Warn,
                format!("Tool '{tool}' not in cached list; attempting anyway."),
            );
        }

        let arg_value = if args.is_empty() {
            json!({})
        } else {
            let raw = args.join(" ");
            match serde_json::from_str::<Value>(&raw) {
                Ok(value) => value,
                Err(err) => {
                    self.log(LogLevel::Error, format!("Invalid JSON args: {err}"));
                    return;
                }
            }
        };

        match self.call_mcp_tool_value(tool, arg_value) {
            Ok(value) => {
                let rendered = format_json(value);
                self.log(LogLevel::Info, format!("Tool {tool} result:"));
                self.log(LogLevel::Info, rendered);
            }
            Err(err) => {
                self.log(LogLevel::Error, format!("Tool call failed: {err:#}"));
            }
        }
    }

    fn call_mcp_tool_value(&mut self, tool: &str, arg_value: Value) -> Result<Value> {
        let connection = self
            .mcp_connection
            .as_ref()
            .ok_or_else(|| anyhow!("No active MCP connection"))?;
        let result = self
            .runtime
            .block_on(mcp::call_tool(connection, tool, arg_value))?;
        Ok(result)
    }

    fn show_mcp_status(&mut self) {
        if let Some(connection) = &self.mcp_connection {
            let tool_count = connection.tool_cache.len();
            self.log(
                LogLevel::Info,
                format!(
                    "Connected MCP: {} ({} tools)",
                    connection.server.display_name(),
                    tool_count
                ),
            );
        } else if let Some(server) = &self.active_mcp {
            self.log(
                LogLevel::Info,
                format!(
                    "Active MCP (saved): {} ({})",
                    server.display_name(),
                    server.url
                ),
            );
        } else {
            self.log(LogLevel::Info, "No active MCP.".to_string());
        }
    }

    fn authenticate_mcp(&mut self, target: &str) {
        let Some(server) = self.mcp_config.find_by_id_or_name(target) else {
            self.log(LogLevel::Warn, format!("Unknown MCP server: {target}"));
            return;
        };

        let Some(auth) = &server.auth else {
            self.log(LogLevel::Warn, "No auth config for server.".to_string());
            return;
        };

        if auth.auth_type != "oauth_browser" {
            self.log(
                LogLevel::Warn,
                "Auth flow only supports oauth_browser.".to_string(),
            );
            return;
        }

        let client_id = self.resolve_mcp_client_id(&server, auth);
        let client_secret = self.resolve_mcp_client_secret(auth);
        let mut oauth_logs = Vec::new();

        let http_client = reqwest::Client::new();

        // First, prepare the OAuth flow (discover endpoints, register, build URL).
        let prepare_result = self.runtime.block_on(mcp::oauth::prepare_auth(
            &http_client,
            &server,
            auth,
            client_id,
            client_secret,
            |line| oauth_logs.push(line),
        ));

        for line in oauth_logs.drain(..) {
            self.log(LogLevel::Info, line);
        }

        let (auth_url, pending) = match prepare_result {
            Ok(result) => result,
            Err(err) => {
                self.log(
                    LogLevel::Error,
                    format!("OAuth preparation failed: {err:#}"),
                );
                return;
            }
        };

        // Store pending state so /mcp auth-code can use it.
        self.pending_oauth = Some((server.id.clone(), pending.clone()));

        self.log(
            LogLevel::Info,
            "Opening browser for authorization...".to_string(),
        );
        self.log(LogLevel::Info, format!("URL: {auth_url}"));
        if let Err(err) = open::that(&auth_url) {
            self.log(LogLevel::Warn, format!("Could not open browser: {err}"));
        }

        self.log(
            LogLevel::Info,
            format!(
                "Waiting for callback on {}... If the browser redirects elsewhere, \
                copy the URL from the browser and run:\n  /mcp auth-code {} <url>",
                pending.redirect_uri, server.id
            ),
        );

        // Start local callback server and wait for the redirect.
        let wait_result = self.runtime.block_on(async {
            mcp::oauth::wait_for_oauth_callback(&pending, Duration::from_secs(120)).await
        });

        match wait_result {
            Ok(token) => {
                self.pending_oauth = None;
                self.store_mcp_token(&server.id, &token.access_token);
                if let Some(refresh) = &token.refresh_token {
                    self.store_mcp_refresh_token(&server.id, refresh);
                }
                if let Some(client_id) = &token.client_id {
                    self.store_mcp_client_id(&server.id, client_id, auth);
                }
                self.log(LogLevel::Info, "OAuth complete. Token stored.".to_string());
            }
            Err(err) => {
                self.log(LogLevel::Warn, format!("Callback not received: {err:#}"));
                self.log(
                    LogLevel::Info,
                    format!(
                        "If the browser showed a page with ?code= in the URL, \
                        copy that full URL and run:\n  /mcp auth-code {} <url>",
                        server.id
                    ),
                );
            }
        }
    }

    fn complete_oauth_manual(&mut self, server_id: &str, raw_input: &str) {
        let Some((pending_id, pending)) = &self.pending_oauth else {
            self.log(
                LogLevel::Warn,
                "No pending OAuth flow. Run /mcp auth <id> first.".to_string(),
            );
            return;
        };

        if pending_id != server_id {
            self.log(
                LogLevel::Warn,
                format!(
                    "Pending OAuth is for '{}', not '{server_id}'. Run /mcp auth {server_id} first.",
                    pending_id
                ),
            );
            return;
        }

        let pending = pending.clone();
        let http_client = reqwest::Client::new();

        let result = self
            .runtime
            .block_on(mcp::oauth::exchange_manual_code_with_input(
                &http_client,
                &pending,
                raw_input,
            ));

        match result {
            Ok(token) => {
                self.pending_oauth = None;
                self.store_mcp_token(server_id, &token.access_token);
                if let Some(refresh) = &token.refresh_token {
                    self.store_mcp_refresh_token(server_id, refresh);
                }
                if let Some(client_id) = &token.client_id {
                    if let Some(server) = self.mcp_config.find_by_id_or_name(server_id) {
                        if let Some(auth) = &server.auth {
                            self.store_mcp_client_id(server_id, client_id, auth);
                        }
                    }
                }
                self.log(
                    LogLevel::Info,
                    "OAuth complete (manual). Token stored.".to_string(),
                );
            }
            Err(err) => {
                self.log(
                    LogLevel::Error,
                    format!("Manual token exchange failed: {err:#}"),
                );
            }
        }
    }

    fn resolve_mcp_client_id(
        &mut self,
        server: &McpServer,
        auth: &crate::mcp::config::McpAuth,
    ) -> Option<String> {
        if let Some(client_id) = &auth.client_id {
            return Some(client_id.clone());
        }
        if let Some(env_key) = &auth.client_id_env {
            if let Ok(value) = env::var(env_key) {
                return Some(value);
            }
        }
        if let Some(value) = self.local_mcp_store.client_ids.get(&server.id) {
            return Some(value.clone());
        }
        if auth.redirect_uri.is_none() {
            return None;
        }
        let key = format!("mcp_client_{}", server.id);
        if let Ok(Some(Value::String(value))) = self.runtime.block_on(self.rice.get_variable(&key))
        {
            return Some(value);
        }
        None
    }

    fn resolve_mcp_client_secret(&mut self, auth: &crate::mcp::config::McpAuth) -> Option<String> {
        if let Some(secret) = &auth.client_secret {
            return Some(secret.clone());
        }
        if let Some(env_key) = &auth.client_secret_env {
            if let Ok(value) = env::var(env_key) {
                return Some(value);
            }
        }
        None
    }

    fn store_mcp_client_id(
        &mut self,
        id: &str,
        client_id: &str,
        auth: &crate::mcp::config::McpAuth,
    ) {
        if auth.redirect_uri.is_none() {
            return;
        }
        self.local_mcp_store
            .client_ids
            .insert(id.to_string(), client_id.to_string());
        if let Err(err) = persist_local_mcp_store(&self.local_mcp_store) {
            self.log(
                LogLevel::Warn,
                format!("Failed to persist local client id: {err:#}"),
            );
        }
        let key = format!("mcp_client_{id}");
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            &key,
            Value::String(client_id.to_string()),
            "explicit",
        )) {
            self.log(
                LogLevel::Warn,
                format!("Failed to store client id: {err:#}"),
            );
        }
    }

    fn store_mcp_refresh_token(&mut self, id: &str, token: &str) {
        let key = format!("mcp_refresh_{id}");
        self.local_mcp_store
            .refresh_tokens
            .insert(id.to_string(), token.to_string());
        if let Err(err) = persist_local_mcp_store(&self.local_mcp_store) {
            self.log(
                LogLevel::Warn,
                format!("Failed to persist local refresh token: {err:#}"),
            );
        }
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            &key,
            Value::String(token.to_string()),
            "explicit",
        )) {
            self.log(
                LogLevel::Warn,
                format!("Failed to store refresh token: {err:#}"),
            );
        }
    }

    fn store_mcp_token(&mut self, id: &str, token: &str) {
        let key = format!("mcp_token_{id}");
        self.local_mcp_store
            .tokens
            .insert(id.to_string(), token.to_string());
        if let Err(err) = persist_local_mcp_store(&self.local_mcp_store) {
            self.log(
                LogLevel::Warn,
                format!("Failed to persist local token: {err:#}"),
            );
        }
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            &key,
            Value::String(token.to_string()),
            "explicit",
        )) {
            self.log(
                LogLevel::Warn,
                format!("Stored token locally, but Rice persistence failed: {err:#}"),
            );
        } else {
            self.log(
                LogLevel::Info,
                format!("Stored MCP token for {id} ({}).", mask_key(token)),
            );
        }
    }

    fn clear_mcp_token(&mut self, id: &str) {
        let key = format!("mcp_token_{id}");
        self.local_mcp_store.tokens.remove(id);
        if let Err(err) = persist_local_mcp_store(&self.local_mcp_store) {
            self.log(
                LogLevel::Warn,
                format!("Failed to update local token cache: {err:#}"),
            );
        }
        if let Err(err) = self.runtime.block_on(self.rice.delete_variable(&key)) {
            self.log(LogLevel::Error, format!("Failed to delete token: {err:#}"));
            return;
        }
        self.log(LogLevel::Info, format!("Cleared MCP token for {id}."));
    }

    fn resolve_mcp_token(&mut self, server: &McpServer) -> Option<String> {
        if let Some(token) = self.local_mcp_store.tokens.get(&server.id) {
            return Some(token.clone());
        }
        let key = format!("mcp_token_{}", server.id);
        if let Ok(Some(Value::String(token))) = self.runtime.block_on(self.rice.get_variable(&key))
        {
            return Some(token);
        }

        if let Some(auth) = &server.auth {
            if let Some(token) = &auth.bearer_token {
                return Some(token.clone());
            }
            if let Some(env_key) = &auth.bearer_env {
                if let Ok(token) = env::var(env_key) {
                    return Some(token);
                }
            }
        }

        None
    }

    fn reload_mcp_config(&mut self) {
        match McpConfig::load() {
            Ok((config, source)) => {
                self.mcp_config = config;
                self.mcp_source = source;
                self.log(
                    LogLevel::Info,
                    format!(
                        "Reloaded MCP config from {} ({} servers).",
                        self.mcp_source.label(),
                        self.mcp_config.servers.len()
                    ),
                );
            }
            Err(err) => {
                self.log(
                    LogLevel::Error,
                    format!("Failed to reload MCP config: {err:#}"),
                );
            }
        }
    }

    fn show_openai_status(&mut self) {
        match &self.openai_key_hint {
            Some(hint) => self.log(LogLevel::Info, format!("OpenAI key stored ({hint}).")),
            None => self.log(LogLevel::Info, "OpenAI key not set.".to_string()),
        }
    }

    fn persist_openai_key(&mut self, key: &str) {
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            OPENAI_KEY_VAR,
            Value::String(key.to_string()),
            "explicit",
        )) {
            self.log(
                LogLevel::Error,
                format!("Failed to store OpenAI key: {err:#}"),
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
            self.log(LogLevel::Error, format!("Failed to delete key: {err:#}"));
            return;
        }

        self.openai_key = None;
        self.openai_key_hint = None;
        self.log(LogLevel::Info, "OpenAI key removed.".to_string());
    }

    fn import_openai_env(&mut self) {
        match env::var("OPENAI_API_KEY") {
            Ok(key) => self.persist_openai_key(&key),
            Err(_) => self.log(LogLevel::Warn, "OPENAI_API_KEY not set.".to_string()),
        }
    }

    fn load_openai_from_rice(&mut self) -> Result<()> {
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

    fn load_active_mcp_from_rice(&mut self) -> Result<()> {
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

    fn handle_chat_message(&mut self, message: &str, require_mcp: bool) {
        let key = match self.ensure_openai_key() {
            Ok(key) => key,
            Err(err) => {
                self.log(LogLevel::Error, format!("OpenAI key missing: {err}"));
                self.log(
                    LogLevel::Info,
                    "Use /openai set <key> or /key <key> to configure.".to_string(),
                );
                return;
            }
        };

        if require_mcp && self.mcp_connection.is_none() {
            self.log(LogLevel::Warn, "No active MCP connection.".to_string());
            return;
        }

        if let Err(err) = self.runtime.block_on(self.rice.focus(message)) {
            self.log(LogLevel::Warn, format!("Rice focus failed: {err:#}"));
        }

        let embedding = match self.runtime.block_on(self.openai.embedding(&key, message)) {
            Ok(vec) => Some(vec),
            Err(err) => {
                self.log(LogLevel::Warn, format!("Embedding failed: {err:#}"));
                None
            }
        };

        let memories = if let Some(embed) = embedding.clone() {
            match self
                .runtime
                .block_on(self.rice.reminisce(embed, self.memory_limit, message))
            {
                Ok(traces) => traces,
                Err(err) => {
                    self.log(LogLevel::Warn, format!("Rice recall failed: {err:#}"));
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        let memory_context = format_memories(&memories);
        let mut input = Vec::new();
        input.push(json!({"role": "system", "content": system_prompt(require_mcp)}));
        if !memory_context.is_empty() {
            input.push(json!({"role": "system", "content": memory_context}));
        }
        input.push(json!({"role": "user", "content": message}));

        let tools = match self.openai_tools_for_mcp(require_mcp) {
            Ok(tools) => tools,
            Err(err) => {
                self.log(
                    LogLevel::Error,
                    format!("Failed to load MCP tools: {err:#}"),
                );
                return;
            }
        };

        let mut response =
            match self
                .runtime
                .block_on(self.openai.response(&key, &input, tools.as_deref()))
            {
                Ok(response) => response,
                Err(err) => {
                    self.log(LogLevel::Error, format!("OpenAI request failed: {err:#}"));
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

        while !tool_calls.is_empty() {
            if tool_loop_limit_reached(tool_loops) {
                self.log(LogLevel::Warn, "Tool loop limit reached.".to_string());
                break;
            }
            tool_loops += 1;

            for call in tool_calls {
                let tool_output = match self.call_mcp_tool_value(&call.name, call.arguments) {
                    Ok(value) => serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
                    Err(err) => {
                        let error = format!("{{\"error\":\"{err}\"}}");
                        error
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
                    Ok(response) => response,
                    Err(err) => {
                        self.log(LogLevel::Error, format!("OpenAI request failed: {err:#}"));
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

        if output_text.is_empty() {
            self.log(
                LogLevel::Warn,
                "OpenAI returned no text output.".to_string(),
            );
        } else {
            self.log(LogLevel::Info, format!("Assistant: {output_text}"));
        }

        let commit_embedding = embedding.unwrap_or_default();
        let commit = self.runtime.block_on(self.rice.commit_trace(
            message,
            &output_text,
            "chat",
            commit_embedding,
            agent_id(),
        ));
        if let Err(err) = commit {
            self.log(LogLevel::Warn, format!("Rice commit failed: {err:#}"));
        }
    }

    fn openai_tools_for_mcp(&mut self, require_mcp: bool) -> Result<Option<Vec<Value>>> {
        let has_connection = self.mcp_connection.is_some();
        if !has_connection {
            if require_mcp {
                return Err(anyhow!("No active MCP connection"));
            }
            return Ok(None);
        }

        let cache_empty = self
            .mcp_connection
            .as_ref()
            .map(|conn| conn.tool_cache.is_empty())
            .unwrap_or(true);
        if cache_empty {
            self.list_mcp_tools();
        }

        let tools = self
            .mcp_connection
            .as_ref()
            .map(|conn| conn.tool_cache.clone())
            .unwrap_or_default();

        if tools.is_empty() {
            if require_mcp {
                return Err(anyhow!("MCP connected but no tools available"));
            }
            return Ok(None);
        }

        let openai_tools = mcp::tools_to_openai(&tools)?;
        Ok(Some(openai_tools))
    }

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

    fn log(&mut self, level: LogLevel, message: String) {
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        self.logs.push(LogLine {
            timestamp,
            level,
            message,
        });
        if self.logs.len() > MAX_LOGS {
            let overflow = self.logs.len() - MAX_LOGS;
            self.logs.drain(0..overflow);
        }
    }

    fn mcp_status_label(&self) -> String {
        if let Some(connection) = &self.mcp_connection {
            format!("{} (connected)", connection.server.display_name())
        } else if let Some(server) = &self.active_mcp {
            format!("{} (saved)", server.display_name())
        } else {
            "none".to_string()
        }
    }

    fn mcp_status_color(&self) -> Color {
        if self.mcp_connection.is_some() {
            Color::Green
        } else if self.active_mcp.is_some() {
            Color::Yellow
        } else {
            Color::DarkGray
        }
    }

    fn openai_status_label(&self) -> String {
        match &self.openai_key_hint {
            Some(hint) => hint.clone(),
            None => "unset".to_string(),
        }
    }

    fn openai_status_color(&self) -> Color {
        if self.openai_key_hint.is_some() {
            Color::Green
        } else {
            Color::DarkGray
        }
    }

    fn rice_status_color(&self) -> Color {
        match self.rice.status {
            RiceStatus::Connected => Color::Green,
            RiceStatus::Disabled(_) => Color::DarkGray,
        }
    }
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct LocalMcpStore {
    tokens: HashMap<String, String>,
    client_ids: HashMap<String, String>,
    refresh_tokens: HashMap<String, String>,
}

fn local_store_path() -> Option<PathBuf> {
    ProjectDirs::from("com", APP_NAME, APP_NAME)
        .map(|dirs| dirs.config_dir().join("local_mcp_store.json"))
}

fn load_local_mcp_store() -> LocalMcpStore {
    let Some(path) = local_store_path() else {
        return LocalMcpStore::default();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return LocalMcpStore::default();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

fn persist_local_mcp_store(store: &LocalMcpStore) -> Result<()> {
    let Some(path) = local_store_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create config dir")?;
    }
    let contents = serde_json::to_string_pretty(store).context("serialize local mcp store")?;
    fs::write(&path, contents).context("write local mcp store")?;
    Ok(())
}

#[derive(Clone, Debug)]
enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn label(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    fn color(&self) -> Color {
        match self {
            LogLevel::Info => Color::Cyan,
            LogLevel::Warn => Color::Yellow,
            LogLevel::Error => Color::Red,
        }
    }
}

#[derive(Clone, Debug)]
struct LogLine {
    timestamp: String,
    level: LogLevel,
    message: String,
}

impl LogLine {
    fn render(&self) -> Line<'_> {
        Line::from(vec![
            Span::styled(
                format!("[{}] ", self.timestamp),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<5} ", self.level.label()),
                Style::default()
                    .fg(self.level.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(self.message.clone()),
        ])
    }
}

fn mask_key(key: &str) -> String {
    let suffix = key
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("***{suffix}")
}
