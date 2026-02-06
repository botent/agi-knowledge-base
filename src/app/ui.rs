//! Terminal UI rendering — layout, status bar, and activity panel.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::rice::RiceStatus;

use super::App;

impl App {
    /// Render the full TUI frame: header bar, activity log, and input prompt.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(frame.size());

        // ── Status bar ───────────────────────────────────────────────
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
        frame.render_widget(Paragraph::new(header_line), chunks[0]);

        // ── Activity log ─────────────────────────────────────────────
        let log_height = chunks[1].height as usize;
        let start = self.logs.len().saturating_sub(log_height);
        let lines: Vec<Line> = self.logs[start..].iter().map(|l| l.render()).collect();

        let log_panel = Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title("Activity"))
            .wrap(Wrap { trim: true });
        frame.render_widget(log_panel, chunks[1]);

        // ── Input prompt ─────────────────────────────────────────────
        let input_panel = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Command"));
        frame.render_widget(input_panel, chunks[2]);

        let input_width = chunks[2].width.saturating_sub(2) as usize;
        let cursor = self.cursor.min(input_width);
        frame.set_cursor(chunks[2].x + 1 + cursor as u16, chunks[2].y + 1);
    }

    // ── Status-bar helpers ───────────────────────────────────────────

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
