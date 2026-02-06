mod app;
mod constants;
mod mcp;
mod openai;
mod rice;
mod util;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::App;

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
        terminal.draw(|frame| app.draw(frame))?;

        if app.should_quit() {
            break;
        }

        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            app.handle_event(ev)?;
        }
    }

    Ok(())
}
