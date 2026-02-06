//! Terminal UI rendering — layout, status bar, side panel, and activity panel.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::rice::RiceStatus;

use super::App;

impl App {
    /// Render the full TUI frame: header bar, activity log, side panel, and input prompt.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(frame.area());

        // ── Status bar ───────────────────────────────────────────────
        self.draw_status_bar(frame, rows[0]);

        // ── Main content area (log + optional side panel) ────────────
        if self.show_side_panel {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(rows[1]);

            self.draw_activity_log(frame, cols[0]);
            self.draw_side_panel(frame, cols[1]);
        } else {
            self.draw_activity_log(frame, rows[1]);
        }

        // ── Input prompt ─────────────────────────────────────────────
        let input_panel = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Command"));
        frame.render_widget(input_panel, rows[2]);

        let input_width = rows[2].width.saturating_sub(2) as usize;
        let cursor = self.cursor.min(input_width);
        frame.set_cursor_position(Position::new(rows[2].x + 1 + cursor as u16, rows[2].y + 1));
    }

    // ── Status bar ───────────────────────────────────────────────────

    fn draw_status_bar(&self, frame: &mut Frame<'_>, area: Rect) {
        let thread_turns = self.conversation_thread.len() / 2;
        let daemon_count = self.daemon_handles.len();
        let spawn_count = self.daemon_results.len();
        let mut spans = vec![
            Span::styled("Persona: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.active_agent.name.clone(),
                Style::default().fg(Color::Magenta),
            ),
        ];
        if let Some(ws) = &self.rice.shared_run_id {
            spans.push(Span::styled(
                "  Workspace: ",
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(ws.clone(), Style::default().fg(Color::Green)));
        }
        spans.extend([
            Span::styled("  Tools: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.mcp_status_label(),
                Style::default().fg(self.mcp_status_color()),
            ),
            Span::styled("  Rice: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.rice.status_label(),
                Style::default().fg(self.rice_status_color()),
            ),
            Span::styled(
                format!("  Turns: {thread_turns}"),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        if daemon_count > 0 {
            spans.push(Span::styled(
                format!("  Auto: {daemon_count}"),
                Style::default().fg(Color::Yellow),
            ));
        }
        if spawn_count > 0 {
            spans.push(Span::styled(
                format!("  Runs: {spawn_count}"),
                Style::default().fg(Color::Cyan),
            ));
        }
        if self.show_side_panel {
            spans.push(Span::styled(
                "  [Tab: panel]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    // ── Activity log ─────────────────────────────────────────────────

    fn draw_activity_log(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2) as usize;

        let log_lines: Vec<Line> = self.logs.iter().flat_map(|l| l.render()).collect();
        let log_paragraph = Paragraph::new(Text::from(log_lines)).wrap(Wrap { trim: false });

        let total_visual = log_paragraph.line_count(inner_width);
        let max_scroll = total_visual.saturating_sub(inner_height);

        if (self.scroll_offset as usize) > max_scroll {
            self.scroll_offset = max_scroll as u16;
        }
        let top_row = max_scroll.saturating_sub(self.scroll_offset as usize) as u16;

        let scroll_indicator = if self.scroll_offset > 0 {
            format!(" Memini [^{}] ", self.scroll_offset)
        } else {
            " Memini ".to_string()
        };

        let panel = log_paragraph
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(scroll_indicator),
            )
            .scroll((top_row, 0));
        frame.render_widget(panel, area);
    }

    // ── Side panel (Autopilot / Agents) ──────────────────────────────

    fn draw_side_panel(&self, frame: &mut Frame<'_>, area: Rect) {
        let inner_width = area.width.saturating_sub(2);

        let mut lines: Vec<Line> = Vec::new();

        // ── Running tasks section ──
        lines.push(Line::from(Span::styled(
            "Running Tasks",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        if self.daemon_handles.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (none -- use /auto start <name>)",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for handle in &self.daemon_handles {
                let name = &handle.def.name;
                let interval = handle.def.interval_secs;
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(name.to_string(), Style::default().fg(Color::Green)),
                    Span::styled(
                        format!("  every {interval}s"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }

        lines.push(Line::from(""));

        // ── Available tasks section ──
        let builtins = super::daemon::builtin_tasks();
        let available: Vec<_> = builtins
            .iter()
            .filter(|b| !self.daemon_handles.iter().any(|h| h.def.name == b.name))
            .collect();

        if !available.is_empty() {
            lines.push(Line::from(Span::styled(
                "Available Tasks",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )));
            for task in available {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(task.name.clone(), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("  {}s", task.interval_secs),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }

        // ── Recent results section ──
        lines.push(Line::from(Span::styled(
            "Recent Results",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

        if self.daemon_results.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no results yet)",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            // Show the most recent results (newest first), truncated per row.
            let max_msg_width = inner_width.saturating_sub(12) as usize;
            for event in self.daemon_results.iter().rev().take(20) {
                let name_span = Span::styled(
                    format!("  {}", event.task_name),
                    Style::default().fg(Color::Cyan),
                );
                let time_span = Span::styled(
                    format!(" {}", event.timestamp),
                    Style::default().fg(Color::DarkGray),
                );
                lines.push(Line::from(vec![name_span, time_span]));

                // Show first line of the message, truncated.
                let first_line = event
                    .message
                    .lines()
                    .next()
                    .unwrap_or("(empty)")
                    .chars()
                    .take(max_msg_width)
                    .collect::<String>();
                lines.push(Line::from(Span::styled(
                    format!("    {first_line}"),
                    Style::default().fg(Color::White),
                )));
            }
        }

        let panel = Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title(" Autopilot "))
            .wrap(Wrap { trim: false });

        frame.render_widget(panel, area);
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
