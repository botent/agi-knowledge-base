//! Logging primitives for the activity panel.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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

// ── Log line ─────────────────────────────────────────────────────────

/// A single timestamped entry in the activity log.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub timestamp: String,
    pub level: LogLevel,
    pub message: String,
}

impl LogLine {
    /// Render this entry as a styled ratatui [`Line`].
    pub fn render(&self) -> Line<'_> {
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
