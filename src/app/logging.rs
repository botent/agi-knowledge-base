//! Logging primitives for the activity panel.
//!
//! Log entries come in two flavours:
//!
//! - **Plain** – timestamped single-line messages (system info, warnings, etc.)
//! - **Markdown** – multi-line rich content from LLM responses, rendered with
//!   `tui-markdown` for proper headings, bold, code blocks, lists, etc.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

// ── Log severity ─────────────────────────────────────────────────────

/// Severity level for an activity-log entry.
#[derive(Clone, Debug)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Short uppercase label for display.
    pub fn label(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    /// Colour associated with this severity.
    pub fn color(&self) -> Color {
        match self {
            LogLevel::Info => Color::Cyan,
            LogLevel::Warn => Color::Yellow,
            LogLevel::Error => Color::Red,
        }
    }
}

// ── Log content ──────────────────────────────────────────────────────

/// The body of a log entry — either a single plain string or pre-parsed
/// markdown lines ready for rendering.
#[derive(Clone, Debug)]
pub enum LogContent {
    /// A simple single-line message.
    Plain(String),
    /// Rich markdown content from an LLM response.  The label (e.g.
    /// "memini") is stored separately so it can be rendered as a
    /// header line before the markdown body.
    Markdown { label: String, body: String },
}

// ── Log line ─────────────────────────────────────────────────────────

/// A single timestamped entry in the activity log.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub timestamp: String,
    pub level: LogLevel,
    pub content: LogContent,
}

impl LogLine {
    /// Render this entry as one or more styled ratatui [`Line`]s.
    ///
    /// Plain entries produce a single line:
    ///   `[HH:MM:SS] INFO  message text`
    ///
    /// Markdown entries produce a coloured header line followed by
    /// tui-markdown–rendered lines for the body.
    pub fn render(&self) -> Vec<Line<'_>> {
        match &self.content {
            LogContent::Plain(msg) => {
                vec![Line::from(vec![
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
                    Span::raw(msg.clone()),
                ])]
            }
            LogContent::Markdown { label, body } => {
                let mut lines = Vec::new();

                // Header: timestamp + coloured label
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("[{}] ", self.timestamp),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        label.clone(),
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                // Render markdown body through tui-markdown.
                let text: Text<'_> = tui_markdown::from_str(body);
                for line in text.lines {
                    lines.push(line.to_owned());
                }

                // Blank line after the response for readability.
                lines.push(Line::raw(""));
                lines
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Mask a secret key, keeping only the last 4 characters visible.
pub fn mask_key(key: &str) -> String {
    let suffix: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("***{suffix}")
}
