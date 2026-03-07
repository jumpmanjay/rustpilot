mod app;
mod config;
mod llm;
mod panels;
mod refs;
mod storage;
mod ui;

use anyhow::Result;
use app::App;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
#[tokio::main]
async fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new()?;

    // Run main loop
    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for events with a small timeout so we can check async channels
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Global quit: Ctrl+Q
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('q')
                {
                    return Ok(());
                }

                // Global panel toggles: Ctrl+1/2/3
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('1') => app.toggle_panel(panels::PanelId::Code),
                        KeyCode::Char('2') => app.toggle_panel(panels::PanelId::Llm),
                        KeyCode::Char('3') => app.toggle_panel(panels::PanelId::Prompt),
                        KeyCode::Char('`') => app.cycle_focus(),
                        _ => {}
                    }
                }

                // Delegate to focused panel
                app.handle_key(key);
            } else if let Event::Resize(_, _) = event::read()? {
                // Terminal will redraw on next loop
            }
        }

        // Check for LLM stream updates
        app.poll_llm_updates();
    }
}
