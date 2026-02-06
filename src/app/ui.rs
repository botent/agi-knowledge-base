//! Terminal UI rendering — dashboard grid, agent sessions, and status bar.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::rice::RiceStatus;

use super::App;
use super::ViewMode;
use super::daemon::AgentWindowStatus;

impl App {
    /// Render the full TUI frame, dispatching to the active view mode.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        match self.view_mode.clone() {
            ViewMode::Dashboard => self.draw_dashboard(frame),
            ViewMode::AgentSession(window_id) => self.draw_agent_session(frame, window_id),
        }
    }

    // ── Dashboard view ───────────────────────────────────────────────

    /// Home screen: status bar, activity log (compact) + 3×3 agent grid, input prompt.
    fn draw_dashboard(&mut self, frame: &mut Frame<'_>) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // status bar
                Constraint::Min(1),    // main area
                Constraint::Length(3), // input prompt
            ])
            .split(frame.area());

        // ── Status bar ───────────────────────────────────────────────
        self.draw_status_bar(frame, rows[0]);

        // ── Main area: activity log (left) + agent grid (right) ──────
        if self.agent_windows.is_empty() && self.daemon_handles.is_empty() {
            // No agents yet — show full-width activity log with hint.
            self.draw_activity_log(frame, rows[1]);
        } else {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(rows[1]);
            self.draw_activity_log(frame, cols[0]);
            self.draw_agent_grid(frame, cols[1]);
        }

        // ── Input prompt ─────────────────────────────────────────────
        let prompt_label = " Command ";
        let input_panel = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(prompt_label));
        frame.render_widget(input_panel, rows[2]);

        let input_width = rows[2].width.saturating_sub(2) as usize;
        let cursor = self.cursor.min(input_width);
        frame.set_cursor_position(Position::new(rows[2].x + 1 + cursor as u16, rows[2].y + 1));
    }

    // ── Agent grid (3×3) ─────────────────────────────────────────────

    /// Render the 3×3 grid of agent cards in the dashboard.
    fn draw_agent_grid(&self, frame: &mut Frame<'_>, area: Rect) {
        let grid_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .split(area);

        for row in 0..3u16 {
            let grid_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                ])
                .split(grid_rows[row as usize]);

            for col in 0..3u16 {
                let cell_idx = (row * 3 + col) as usize;
                let cell_area = grid_cols[col as usize];
                let is_selected = cell_idx == self.grid_selected;

                if let Some(window) = self.agent_windows.get(cell_idx) {
                    self.draw_agent_card(frame, cell_area, window, is_selected);
                } else {
                    self.draw_empty_card(frame, cell_area, cell_idx, is_selected);
                }
            }
        }
    }

    /// Render one agent card in the grid.
    fn draw_agent_card(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        window: &super::daemon::AgentWindow,
        is_selected: bool,
    ) {
        let (status_icon, status_color) = match window.status {
            AgentWindowStatus::Thinking => ("⟳", Color::Yellow),
            AgentWindowStatus::Done => ("✓", Color::Green),
            AgentWindowStatus::WaitingForInput => ("?", Color::Red),
        };

        let title = format!(" #{} {} {} ", window.id, status_icon, window.label);

        let border_style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(status_color)
        };

        // Build card content: prompt preview + last few output lines.
        let inner_height = area.height.saturating_sub(2) as usize;
        let mut lines: Vec<Line> = Vec::new();

        // Prompt preview (first line).
        let preview: String = window.prompt.chars().take(40).collect();
        lines.push(Line::from(Span::styled(
            format!(" {preview}…"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));

        // Status line.
        let status_text = match window.status {
            AgentWindowStatus::Thinking => "Thinking…",
            AgentWindowStatus::Done => "Done",
            AgentWindowStatus::WaitingForInput => "NEEDS INPUT",
        };
        lines.push(Line::from(Span::styled(
            format!(" {status_text}"),
            Style::default().fg(status_color),
        )));

        // Tail of output.
        let remaining = inner_height.saturating_sub(lines.len());
        if remaining > 0 {
            let tail: Vec<&String> = window
                .output_lines
                .iter()
                .rev()
                .take(remaining)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            for s in tail {
                let truncated: String = s.chars().take(50).collect();
                lines.push(Line::from(Span::styled(
                    format!(" {truncated}"),
                    Style::default().fg(Color::White),
                )));
            }
        }

        let card = Paragraph::new(Text::from(lines)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(Span::styled(title, Style::default().fg(status_color))),
        );
        frame.render_widget(card, area);
    }

    /// Render an empty grid cell placeholder.
    fn draw_empty_card(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        _cell_idx: usize,
        is_selected: bool,
    ) {
        let border_style = if is_selected {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Rgb(40, 40, 40))
        };

        let hint = if is_selected {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  /spawn <prompt>",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "  to start an agent",
                    Style::default().fg(Color::Rgb(60, 60, 60)),
                )),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  ─",
                    Style::default().fg(Color::Rgb(40, 40, 40)),
                )),
            ]
        };

        let card = Paragraph::new(Text::from(hint)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style),
        );
        frame.render_widget(card, area);
    }

    // ── Agent session view ───────────────────────────────────────────

    /// Full-screen view for a single agent session.
    fn draw_agent_session(&mut self, frame: &mut Frame<'_>, window_id: usize) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // status bar
                Constraint::Min(1),    // agent output
                Constraint::Length(3), // input prompt
            ])
            .split(frame.area());

        // ── Status bar ───────────────────────────────────────────────
        self.draw_status_bar(frame, rows[0]);

        // ── Agent output (full width) ────────────────────────────────
        if let Some(window) = self.agent_windows.iter().find(|w| w.id == window_id) {
            let (status_label, status_color) = match window.status {
                AgentWindowStatus::Thinking => ("thinking…", Color::Yellow),
                AgentWindowStatus::Done => ("done", Color::Green),
                AgentWindowStatus::WaitingForInput => ("NEEDS INPUT", Color::Red),
            };

            let title = format!(
                " #{} {} — {} [Esc: back] ",
                window.id, window.label, status_label
            );

            let inner_height = rows[1].height.saturating_sub(2) as usize;
            let display_lines: Vec<Line> = window
                .output_lines
                .iter()
                .rev()
                .take(inner_height.max(1))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|s| {
                    let color = if s.starts_with(">>") {
                        Color::Red
                    } else if s.starts_with("--") {
                        Color::DarkGray
                    } else if s.starts_with("Thinking")
                        || s.starts_with("Recalling")
                        || s.starts_with("Saving")
                        || s.starts_with("Found")
                    {
                        Color::Yellow
                    } else {
                        Color::White
                    };
                    Line::from(Span::styled(format!(" {s}"), Style::default().fg(color)))
                })
                .collect();

            let panel = Paragraph::new(Text::from(display_lines))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(status_color))
                        .title(Span::styled(
                            title,
                            Style::default()
                                .fg(status_color)
                                .add_modifier(Modifier::BOLD),
                        )),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(panel, rows[1]);
        } else {
            // Window no longer exists — show message.
            let msg = Paragraph::new("Agent window not found. Press Esc to return.")
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL));
            frame.render_widget(msg, rows[1]);
        }

        // ── Input prompt ─────────────────────────────────────────────
        let prompt_label = if self
            .agent_windows
            .iter()
            .any(|w| w.id == window_id && w.status == AgentWindowStatus::WaitingForInput)
        {
            format!(" Reply to Agent #{window_id} ")
        } else {
            format!(" Command (Agent #{window_id}) ")
        };

        let input_panel = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(prompt_label));
        frame.render_widget(input_panel, rows[2]);

        let input_width = rows[2].width.saturating_sub(2) as usize;
        let cursor = self.cursor.min(input_width);
        frame.set_cursor_position(Position::new(rows[2].x + 1 + cursor as u16, rows[2].y + 1));
    }

    // ── Status bar ───────────────────────────────────────────────────

    fn draw_status_bar(&self, frame: &mut Frame<'_>, area: Rect) {
        let thread_turns = self.conversation_thread.len() / 2;
        let daemon_count = self.daemon_handles.len();
        let window_count = self.agent_windows.len();
        let thinking = self
            .agent_windows
            .iter()
            .filter(|w| w.status == AgentWindowStatus::Thinking)
            .count();
        let waiting = self
            .agent_windows
            .iter()
            .filter(|w| w.status == AgentWindowStatus::WaitingForInput)
            .count();

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
        if window_count > 0 {
            let mut agent_label = format!("  Agents: {window_count}");
            if thinking > 0 {
                agent_label.push_str(&format!(" ({thinking} thinking)"));
            }
            if waiting > 0 {
                agent_label.push_str(&format!(" ({waiting} waiting)"));
            }
            spans.push(Span::styled(agent_label, Style::default().fg(Color::Cyan)));
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

    #[allow(dead_code)]
    fn openai_status_label(&self) -> String {
        match &self.openai_key_hint {
            Some(hint) => hint.clone(),
            None => "unset".to_string(),
        }
    }

    #[allow(dead_code)]
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
