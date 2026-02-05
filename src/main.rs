use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use directories::ProjectDirs;
use mcp_protocol_sdk::client::McpClient;
use mcp_protocol_sdk::protocol::types::Tool as McpTool;
use mcp_protocol_sdk::transport::http::HttpClientTransport;
use mcp_protocol_sdk::transport::traits::TransportConfig;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use rice_sdk::rice_core::config::{StateConfig, StorageConfig};
use rice_sdk::rice_state::proto::Trace;
use rice_sdk::{Client, RiceConfig};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::runtime::Runtime;

const OPENAI_KEY_VAR: &str = "openai_api_key";
const ACTIVE_MCP_VAR: &str = "active_mcp";
const APP_NAME: &str = "memini";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_RUN_ID: &str = "memini";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const DEFAULT_OPENAI_EMBED_MODEL: &str = "text-embedding-3-small";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_TOOL_LOOPS: usize = 6;
const DEFAULT_MEMORY_LIMIT: u64 = 6;
const MAX_LOGS: usize = 1000;

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let mut terminal = setup_terminal()?;
    let mut app = App::new()?;

    let run_result = run_app(&mut terminal, &mut app);

    restore_terminal()?;
    run_result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal() -> Result<()> {
    terminal::disable_raw_mode().context("disable raw mode")?;
    let mut stdout = io::stdout();
    stdout.execute(LeaveAlternateScreen)?;
    stdout.execute(DisableMouseCapture)?;
    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui(frame, app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key)?;
            }
        }
    }

    Ok(())
}

fn ui(frame: &mut ratatui::Frame, app: &mut App) {
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
        Span::styled(app.mcp_status_label(), Style::default().fg(app.mcp_status_color())),
        Span::styled("  OpenAI: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.openai_status_label(), Style::default().fg(app.openai_status_color())),
        Span::styled("  Rice: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.rice.status_label(), Style::default().fg(app.rice.status_color())),
    ]);

    let header = Paragraph::new(header_line);
    frame.render_widget(header, chunks[0]);

    let log_height = chunks[1].height as usize;
    let start = app.logs.len().saturating_sub(log_height);
    let lines: Vec<Line> = app.logs[start..]
        .iter()
        .map(|line| line.render())
        .collect();
    let log_text = Text::from(lines);

    let log_panel = Paragraph::new(log_text)
        .block(Block::default().borders(Borders::ALL).title("Activity"))
        .wrap(Wrap { trim: true });
    frame.render_widget(log_panel, chunks[1]);

    let input_panel = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title("Command"));
    frame.render_widget(input_panel, chunks[2]);

    let input_width = chunks[2].width.saturating_sub(2) as usize;
    let cursor = app.cursor.min(input_width);
    let cursor_x = chunks[2].x + 1 + cursor as u16;
    let cursor_y = chunks[2].y + 1;
    frame.set_cursor(cursor_x, cursor_y);
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct McpConfig {
    servers: Vec<McpServer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct McpServer {
    id: String,
    name: Option<String>,
    url: String,
    #[serde(default)]
    sse_url: Option<String>,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    auth: Option<McpAuth>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct McpAuth {
    #[serde(rename = "type")]
    auth_type: String,
    login_url: Option<String>,
    notes: Option<String>,
    #[serde(default)]
    bearer_env: Option<String>,
    #[serde(default)]
    bearer_token: Option<String>,
}

impl McpServer {
    fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
    }
}

#[derive(Clone, Debug)]
enum McpSource {
    Embedded,
    File(PathBuf),
}

impl McpSource {
    fn label(&self) -> String {
        match self {
            McpSource::Embedded => "embedded defaults".to_string(),
            McpSource::File(path) => path.display().to_string(),
        }
    }
}

impl McpConfig {
    fn load() -> Result<(Self, McpSource)> {
        if let Ok(path) = env::var("MEMINI_MCP_JSON") {
            let path = PathBuf::from(path);
            return Ok((Self::load_from_path(&path)?, McpSource::File(path)));
        }

        let cwd_path = PathBuf::from("mcp.json");
        if cwd_path.exists() {
            return Ok((Self::load_from_path(&cwd_path)?, McpSource::File(cwd_path)));
        }

        if let Some(config_path) = config_dir_file("mcp.json") {
            if config_path.exists() {
                return Ok((Self::load_from_path(&config_path)?, McpSource::File(config_path)));
            }
        }

        let embedded: McpConfig = serde_json::from_str(include_str!("../mcp.json"))
            .context("parse embedded mcp.json")?;
        Ok((embedded, McpSource::Embedded))
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read mcp config from {}", path.display()))?;
        let config = serde_json::from_str(&contents)
            .with_context(|| format!("parse mcp config from {}", path.display()))?;
        Ok(config)
    }

    fn find_by_id_or_name(&self, query: &str) -> Option<McpServer> {
        let query = query.to_lowercase();
        self.servers.iter().find(|server| {
            server.id.to_lowercase() == query
                || server
                    .name
                    .as_ref()
                    .map(|name| name.to_lowercase() == query)
                    .unwrap_or(false)
        }).cloned()
    }
}

struct McpConnection {
    server: McpServer,
    client: McpClient,
    tool_cache: Vec<McpTool>,
}

struct App {
    runtime: Runtime,
    input: String,
    cursor: usize,
    logs: Vec<LogLine>,
    mcp_config: McpConfig,
    mcp_source: McpSource,
    active_mcp: Option<McpServer>,
    mcp_connection: Option<McpConnection>,
    rice: RiceStore,
    openai_key_hint: Option<String>,
    openai_key: Option<String>,
    openai_model: String,
    openai_embed_model: String,
    openai_base_url: String,
    http_client: HttpClient,
    memory_limit: u64,
    should_quit: bool,
}

impl App {
    fn new() -> Result<Self> {
        let runtime = Runtime::new().context("create tokio runtime")?;
        let (mcp_config, mcp_source) = McpConfig::load()?;
        let rice = runtime.block_on(RiceStore::connect());
        let openai_model = openai_model_from_env();
        let openai_embed_model = openai_embed_model_from_env();
        let openai_base_url = openai_base_url_from_env();
        let memory_limit = env_first(&["MEMINI_MEMORY_LIMIT"]) //
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
            rice,
            openai_key_hint: None,
            openai_key: None,
            openai_model,
            openai_embed_model,
            openai_base_url,
            http_client: HttpClient::new(),
            memory_limit,
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
        self.log(LogLevel::Info, "/mcp                   List MCP servers".to_string());
        self.log(LogLevel::Info, "/mcp connect <id>      Set active MCP".to_string());
        self.log(LogLevel::Info, "/mcp auth <id>         Show auth instructions".to_string());
        self.log(LogLevel::Info, "/mcp status            Show active MCP".to_string());
        self.log(LogLevel::Info, "/mcp tools             List tools on active MCP".to_string());
        self.log(LogLevel::Info, "/mcp call <tool> <json>Call MCP tool with JSON args".to_string());
        self.log(LogLevel::Info, "/mcp ask <prompt>      Chat using MCP tools".to_string());
        self.log(LogLevel::Info, "/mcp disconnect        Close MCP connection".to_string());
        self.log(LogLevel::Info, "/mcp token <id> <tok>  Store bearer token in Rice".to_string());
        self.log(LogLevel::Info, "/mcp token-clear <id>  Remove stored bearer token".to_string());
        self.log(LogLevel::Info, "/mcp reload            Reload MCP config".to_string());
        self.log(LogLevel::Info, "/openai                Show OpenAI key status".to_string());
        self.log(LogLevel::Info, "/openai set <key>      Persist OpenAI key in Rice".to_string());
        self.log(LogLevel::Info, "/key <key>             Set OpenAI key".to_string());
        self.log(LogLevel::Info, "/openai clear          Remove OpenAI key".to_string());
        self.log(LogLevel::Info, "/openai import-env     Store $OPENAI_API_KEY".to_string());
        self.log(LogLevel::Info, "/rice                  Show Rice connection status".to_string());
        self.log(LogLevel::Info, "/clear                 Clear activity log".to_string());
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
                    self.show_mcp_auth(target);
                } else {
                    self.log(LogLevel::Warn, "Usage: /mcp auth <id>".to_string());
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
                self.log(
                    LogLevel::Warn,
                    format!("Unknown /mcp command: {other}"),
                );
            }
        }
    }

    fn handle_openai_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_openai_status();
            return;
        }

        match args[0] {
            "set" => {
                if let Some(key) = args.get(1) {
                    self.persist_openai_key(key);
                } else {
                    self.log(LogLevel::Warn, "Usage: /openai set <key>".to_string());
                }
            }
            "key" => {
                if let Some(key) = args.get(1) {
                    self.persist_openai_key(key);
                } else {
                    self.log(LogLevel::Warn, "Usage: /openai key <key>".to_string());
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
            self.log(LogLevel::Warn, format!("Rice focus failed: {err}"));
        }

        let embedding = match self.openai_embedding(&key, message) {
            Ok(vec) => Some(vec),
            Err(err) => {
                self.log(LogLevel::Warn, format!("Embedding failed: {err}"));
                None
            }
        };

        let memories = if let Some(embed) = embedding.clone() {
            match self.runtime.block_on(self.rice.reminisce(embed, self.memory_limit, message)) {
                Ok(traces) => traces,
                Err(err) => {
                    self.log(LogLevel::Warn, format!("Rice recall failed: {err}"));
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
                self.log(LogLevel::Error, format!("Failed to load MCP tools: {err}"));
                return;
            }
        };

        let response = match self.openai_response(&key, &input, tools.as_deref()) {
            Ok(response) => response,
            Err(err) => {
                self.log(LogLevel::Error, format!("OpenAI request failed: {err}"));
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
            if tool_loops >= MAX_TOOL_LOOPS {
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

            let response = match self.openai_response(&key, &input, tools.as_deref()) {
                Ok(response) => response,
                Err(err) => {
                    self.log(LogLevel::Error, format!("OpenAI request failed: {err}"));
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
            self.log(LogLevel::Warn, "OpenAI returned no text output.".to_string());
        } else {
            self.log(LogLevel::Info, format!("Assistant: {output_text}"));
        }

        let commit_embedding = embedding.unwrap_or_default();
        let commit = self.runtime.block_on(self.rice.commit_trace(
            message,
            &output_text,
            "chat",
            commit_embedding,
        ));
        if let Err(err) = commit {
            self.log(LogLevel::Warn, format!("Rice commit failed: {err}"));
        }
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
                        "No bearer token found. Run /mcp auth <id> for OAuth info.".to_string(),
                    );
                }
            }
        }

        let base_url = normalize_url(&server.url);
        let sse_url = server
            .sse_url
            .as_deref()
            .map(normalize_url)
            .unwrap_or_else(|| base_url.clone());

        let connect_result = self.runtime.block_on(async {
            let mut config = TransportConfig::default();
            if let Some(headers) = &server.headers {
                for (key, value) in headers {
                    config.headers.insert(key.clone(), value.clone());
                }
            }
            if let Some(token) = &bearer {
                config.headers.insert(
                    "Authorization".to_string(),
                    format!("Bearer {token}"),
                );
            }

            let transport =
                HttpClientTransport::with_config(base_url.as_str(), Some(sse_url.as_str()), config)
                    .await?;
            let mut client = McpClient::new(APP_NAME.to_string(), APP_VERSION.to_string());
            client.connect(transport).await?;
            Ok::<_, anyhow::Error>(client)
        });

        match connect_result {
            Ok(client) => {
                self.active_mcp = Some(server.clone());
                self.mcp_connection = Some(McpConnection {
                    server: server.clone(),
                    client,
                    tool_cache: Vec::new(),
                });

                let store_result = self.runtime.block_on(self.rice.set_variable(
                    ACTIVE_MCP_VAR,
                    serde_json::to_value(&server).unwrap_or(Value::Null),
                    "explicit",
                ));

                if let Err(err) = store_result {
                    self.log(LogLevel::Warn, format!("Failed to persist MCP: {err}"));
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
                self.log(LogLevel::Error, format!("Failed to connect MCP: {err}"));
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
            let Some(client) = self.mcp_connection.as_ref().map(|conn| &conn.client) else {
                self.log(LogLevel::Warn, "No active MCP connection.".to_string());
                return;
            };
            self.runtime.block_on(async { client.list_tools(None).await })
        };
        match tools_result {
            Ok(tools) => {
                let tool_list = tools.tools.clone();
                if let Some(connection) = self.mcp_connection.as_mut() {
                    connection.tool_cache = tool_list.clone();
                }
                if tool_list.is_empty() {
                    self.log(LogLevel::Info, "No tools reported by MCP.".to_string());
                } else {
                    self.log(LogLevel::Info, "MCP tools:".to_string());
                    for tool in tool_list {
                        self.log(LogLevel::Info, format!("- {}", tool.name));
                    }
                }
            }
            Err(err) => {
                self.log(LogLevel::Error, format!("Failed to list tools: {err}"));
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
                self.log(LogLevel::Error, format!("Tool call failed: {err}"));
            }
        }
    }

    fn call_mcp_tool_value(&mut self, tool: &str, arg_value: Value) -> Result<Value> {
        let arg_map = match arg_value {
            Value::Null => None,
            Value::Object(map) => Some(map.into_iter().collect()),
            other => {
                return Err(anyhow!(
                    "Tool args must be a JSON object, got {other}"
                ));
            }
        };

        let result = {
            let Some(client) = self.mcp_connection.as_ref().map(|conn| &conn.client) else {
                return Err(anyhow!("No active MCP connection"));
            };
            self.runtime
                .block_on(async { client.call_tool(tool.to_string(), arg_map).await })
        }
        .context("call MCP tool")?;

        let value = serde_json::to_value(result).context("serialize tool result")?;
        Ok(value)
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

        let mut openai_tools = Vec::new();
        for tool in tools {
            let parameters =
                serde_json::to_value(&tool.input_schema).context("serialize tool schema")?;
            openai_tools.push(json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": parameters
            }));
        }

        Ok(Some(openai_tools))
    }

    fn ensure_openai_key(&mut self) -> Result<String> {
        if let Some(key) = &self.openai_key {
            return Ok(key.clone());
        }

        if let Ok(Some(Value::String(key))) =
            self.runtime.block_on(self.rice.get_variable(OPENAI_KEY_VAR))
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

    fn openai_embedding(&mut self, key: &str, text: &str) -> Result<Vec<f32>> {
        let model = self.openai_embed_model.clone();
        let body = json!({
            "model": model,
            "input": text
        });
        let response = self.openai_request(key, "embeddings", body)?;
        let data = response
            .get("data")
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow!("Missing embeddings data"))?;
        let first = data
            .get(0)
            .and_then(|value| value.get("embedding"))
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow!("Missing embedding vector"))?;
        let mut embedding = Vec::with_capacity(first.len());
        for value in first {
            if let Some(num) = value.as_f64() {
                embedding.push(num as f32);
            }
        }
        Ok(embedding)
    }

    fn openai_response(
        &mut self,
        key: &str,
        input: &[Value],
        tools: Option<&[Value]>,
    ) -> Result<Value> {
        let mut body = json!({
            "model": self.openai_model.clone(),
            "input": input,
        });
        if let Some(tools) = tools {
            body["tools"] = Value::Array(tools.to_vec());
        }
        self.openai_request(key, "responses", body)
    }

    fn openai_request(&mut self, key: &str, path: &str, body: Value) -> Result<Value> {
        let client = self.http_client.clone();
        let base_url = self.openai_base_url.clone();
        let url = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let payload = body;
        let key = key.to_string();

        let response = self.runtime.block_on(async move {
            let response = client
                .post(url)
                .bearer_auth(key)
                .json(&payload)
                .send()
                .await
                .context("send OpenAI request")?;
            let status = response.status();
            let text = response.text().await.context("read OpenAI response")?;
            let json: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw": text}));
            if !status.is_success() {
                return Err(anyhow!("OpenAI error {status}: {json}"));
            }
            Ok(json)
        })?;

        Ok(response)
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

    fn show_mcp_auth(&mut self, target: &str) {
        let Some(server) = self.mcp_config.find_by_id_or_name(target) else {
            self.log(LogLevel::Warn, format!("Unknown MCP server: {target}"));
            return;
        };

        if let Some(token) = self.resolve_mcp_token(&server) {
            self.log(
                LogLevel::Info,
                format!("Stored bearer token: {}", mask_key(&token)),
            );
        }

        if let Some(auth) = &server.auth {
            self.log(
                LogLevel::Info,
                format!("Auth for {}: {}", server.display_name(), auth.auth_type),
            );
            if let Some(login) = &auth.login_url {
                self.log(LogLevel::Info, format!("Login URL: {login}"));
            }
            if let Some(notes) = &auth.notes {
                self.log(LogLevel::Info, notes.clone());
            }
            if let Some(env_key) = &auth.bearer_env {
                self.log(
                    LogLevel::Info,
                    format!("Bearer token env: {env_key}"),
                );
            }
        } else {
            self.log(
                LogLevel::Info,
                format!("No auth info for {}.", server.display_name()),
            );
        }
    }

    fn store_mcp_token(&mut self, id: &str, token: &str) {
        let key = format!("mcp_token_{id}");
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            &key,
            Value::String(token.to_string()),
            "explicit",
        )) {
            self.log(LogLevel::Error, format!("Failed to store token: {err}"));
            return;
        }
        self.log(
            LogLevel::Info,
            format!("Stored MCP token for {id} ({}).", mask_key(token)),
        );
    }

    fn clear_mcp_token(&mut self, id: &str) {
        let key = format!("mcp_token_{id}");
        if let Err(err) = self.runtime.block_on(self.rice.delete_variable(&key)) {
            self.log(LogLevel::Error, format!("Failed to delete token: {err}"));
            return;
        }
        self.log(LogLevel::Info, format!("Cleared MCP token for {id}."));
    }

    fn resolve_mcp_token(&mut self, server: &McpServer) -> Option<String> {
        let key = format!("mcp_token_{}", server.id);
        if let Ok(Some(Value::String(token))) = self.runtime.block_on(self.rice.get_variable(&key)) {
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
                self.log(LogLevel::Error, format!("Failed to reload MCP config: {err}"));
            }
        }
    }

    fn show_openai_status(&mut self) {
        match &self.openai_key_hint {
            Some(hint) => self.log(
                LogLevel::Info,
                format!("OpenAI key stored ({hint})."),
            ),
            None => self.log(LogLevel::Info, "OpenAI key not set.".to_string()),
        }
    }

    fn persist_openai_key(&mut self, key: &str) {
        if let Err(err) = self.runtime.block_on(self.rice.set_variable(
            OPENAI_KEY_VAR,
            Value::String(key.to_string()),
            "explicit",
        )) {
            self.log(LogLevel::Error, format!("Failed to store OpenAI key: {err}"));
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
            self.log(LogLevel::Error, format!("Failed to delete key: {err}"));
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
            let server: McpServer = serde_json::from_value(value)
                .context("decode active MCP from Rice")?;
            self.active_mcp = Some(server);
        }
        Ok(())
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

fn normalize_url(raw: &str) -> String {
    if raw.contains("://") {
        return raw.to_string();
    }
    let scheme = if raw.starts_with("localhost") || raw.starts_with("127.") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{raw}")
}

fn env_first(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = env::var(key) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn openai_model_from_env() -> String {
    env_first(&["OPENAI_MODEL", "MEMINI_OPENAI_MODEL"])
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string())
}

fn openai_embed_model_from_env() -> String {
    env_first(&["OPENAI_EMBED_MODEL", "MEMINI_OPENAI_EMBED_MODEL"])
        .unwrap_or_else(|| DEFAULT_OPENAI_EMBED_MODEL.to_string())
}

fn openai_base_url_from_env() -> String {
    let raw = env_first(&["OPENAI_BASE_URL", "OPENAI_API_BASE"])
        .unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string());
    raw.trim_end_matches('/').to_string()
}

fn rice_run_id() -> String {
    env::var("MEMINI_RUN_ID").unwrap_or_else(|_| DEFAULT_RUN_ID.to_string())
}

fn system_prompt(require_mcp: bool) -> String {
    if require_mcp {
        "You are Memini, a concise CLI assistant. Use available tools when needed to answer the user's request. Summarize results clearly.".to_string()
    } else {
        "You are Memini, a concise CLI assistant. Use any provided memory context when helpful and answer clearly.".to_string()
    }
}

fn format_memories(traces: &[Trace]) -> String {
    if traces.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push("Relevant memory from Rice:".to_string());
    for trace in traces {
        let input = trace.input.trim();
        let outcome = trace.outcome.trim();
        if input.is_empty() && outcome.is_empty() {
            continue;
        }
        let action = trace.action.trim();
        if action.is_empty() {
            lines.push(format!("- input: {input} | outcome: {outcome}"));
        } else {
            lines.push(format!(
                "- input: {input} | action: {action} | outcome: {outcome}"
            ));
        }
    }
    lines.join("\n")
}

fn format_json<T: Serialize>(value: T) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "<unrenderable>".to_string())
}

#[derive(Clone, Debug)]
struct ToolCall {
    name: String,
    arguments: Value,
    call_id: String,
}

fn extract_output_items(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default()
}

fn extract_output_text(output_items: &[Value]) -> String {
    let mut parts = Vec::new();
    for item in output_items {
        let item_type = item.get("type").and_then(|v| v.as_str());
        if item_type != Some("message") {
            continue;
        }
        let content = match item.get("content").and_then(|v| v.as_array()) {
            Some(content) => content,
            None => continue,
        };
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

fn extract_tool_calls(output_items: &[Value]) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    for item in output_items {
        if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
            continue;
        }
        let name = match item.get("name").and_then(|v| v.as_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let call_id = match item.get("call_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let raw_args = item
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        let arguments = serde_json::from_str(raw_args).unwrap_or_else(|_| json!({"_raw": raw_args}));
        calls.push(ToolCall {
            name,
            arguments,
            call_id,
        });
    }
    calls
}

fn config_dir_file(filename: &str) -> Option<PathBuf> {
    let proj_dirs = ProjectDirs::from("com", APP_NAME, APP_NAME)?;
    Some(proj_dirs.config_dir().join(filename))
}

struct RiceStore {
    client: Option<Client>,
    status: RiceStatus,
    run_id: String,
}

#[derive(Clone, Debug)]
enum RiceStatus {
    Connected,
    Disabled(String),
}

impl RiceStore {
    async fn connect() -> Self {
        let Some(config) = rice_config_from_env() else {
            return RiceStore {
                client: None,
                status: RiceStatus::Disabled("Rice env not configured".to_string()),
                run_id: rice_run_id(),
            };
        };

        match Client::new(config).await {
            Ok(client) => {
                let status = if client.state.is_some() {
                    RiceStatus::Connected
                } else {
                    RiceStatus::Disabled("Rice state module not enabled".to_string())
                };
                RiceStore {
                    client: Some(client),
                    status,
                    run_id: rice_run_id(),
                }
            }
            Err(err) => RiceStore {
                client: None,
                status: RiceStatus::Disabled(format!("Client init failed: {err}")),
                run_id: rice_run_id(),
            },
        }
    }

    fn status_label(&self) -> String {
        match &self.status {
            RiceStatus::Connected => "connected".to_string(),
            RiceStatus::Disabled(reason) => format!("disabled ({reason})"),
        }
    }

    fn status_color(&self) -> Color {
        match self.status {
            RiceStatus::Connected => Color::Green,
            RiceStatus::Disabled(_) => Color::DarkGray,
        }
    }

    async fn set_variable(&mut self, name: &str, value: Value, source: &str) -> Result<()> {
        let client = self.client.as_mut().ok_or_else(|| anyhow!("Rice not connected"))?;
        let state = client
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("Rice state module not enabled"))?;
        let value_json = serde_json::to_string(&value).context("serialize value")?;
        state
            .set_variable(
                self.run_id.clone(),
                name.to_string(),
                value_json,
                source.to_string(),
            )
            .await
            .context("set variable")?;
        Ok(())
    }

    async fn get_variable(&mut self, name: &str) -> Result<Option<Value>> {
        let client = self.client.as_mut().ok_or_else(|| anyhow!("Rice not connected"))?;
        let state = client
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("Rice state module not enabled"))?;
        let variable = state
            .get_variable(self.run_id.clone(), name.to_string())
            .await
            .context("get variable")?;
        if variable.value_json.trim().is_empty() {
            return Ok(None);
        }
        let value =
            serde_json::from_str::<Value>(&variable.value_json).context("parse value_json")?;
        Ok(Some(value))
    }

    async fn delete_variable(&mut self, name: &str) -> Result<()> {
        let client = self.client.as_mut().ok_or_else(|| anyhow!("Rice not connected"))?;
        let state = client
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("Rice state module not enabled"))?;
        state
            .delete_variable(self.run_id.clone(), name.to_string())
            .await
            .context("delete variable")?;
        Ok(())
    }

    async fn focus(&mut self, content: &str) -> Result<()> {
        let client = self.client.as_mut().ok_or_else(|| anyhow!("Rice not connected"))?;
        let state = client
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("Rice state module not enabled"))?;
        state
            .focus(content.to_string(), self.run_id.clone())
            .await
            .context("focus")?;
        Ok(())
    }

    async fn reminisce(
        &mut self,
        embedding: Vec<f32>,
        limit: u64,
        query_text: &str,
    ) -> Result<Vec<Trace>> {
        let client = self.client.as_mut().ok_or_else(|| anyhow!("Rice not connected"))?;
        let state = client
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("Rice state module not enabled"))?;
        let response = state
            .reminisce(embedding, limit, query_text.to_string(), self.run_id.clone())
            .await
            .context("reminisce")?;
        Ok(response.traces)
    }

    async fn commit_trace(
        &mut self,
        input: &str,
        outcome: &str,
        action: &str,
        embedding: Vec<f32>,
    ) -> Result<()> {
        let client = self.client.as_mut().ok_or_else(|| anyhow!("Rice not connected"))?;
        let state = client
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("Rice state module not enabled"))?;
        let trace = Trace {
            input: input.to_string(),
            reasoning: String::new(),
            action: action.to_string(),
            outcome: outcome.to_string(),
            agent_id: APP_NAME.to_string(),
            embedding,
            run_id: self.run_id.clone(),
        };
        state.commit(trace).await.context("commit trace")?;
        Ok(())
    }
}

fn rice_config_from_env() -> Option<RiceConfig> {
    let mut config = RiceConfig::default();
    let mut enabled = false;

    if let Some(state_url) = env_first(&["RICE_STATE_URL", "STATE_INSTANCE_URL"]) {
        enabled = true;
        config.state = Some(StateConfig {
            enabled: true,
            base_url: Some(normalize_url(&state_url)),
            auth_token: env_first(&["RICE_STATE_TOKEN", "STATE_AUTH_TOKEN"]),
            llm_mode: None,
            flux: None,
        });
    }

    if let Some(storage_url) = env_first(&["RICE_STORAGE_URL", "STORAGE_INSTANCE_URL"]) {
        enabled = true;
        config.storage = Some(StorageConfig {
            enabled: true,
            base_url: Some(normalize_url(&storage_url)),
            auth_token: env_first(&["RICE_STORAGE_TOKEN", "STORAGE_AUTH_TOKEN"]),
            username: None,
            password: None,
        });
    }

    if enabled {
        Some(config)
    } else {
        None
    }
}
