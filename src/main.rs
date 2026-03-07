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
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
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
            let ev = event::read()?;

            // Handle mouse events
            if let Event::Mouse(mouse) = ev {
                if app.quit_confirm {
                    app.quit_confirm = false;
                    continue;
                }
                app.handle_mouse(mouse);
                continue;
            }

            if let Event::Key(key) = ev {
                // Global quit: Ctrl+Q (with confirmation)
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('q')
                {
                    if app.quit_confirm {
                        return Ok(());
                    } else {
                        app.quit_confirm = true;
                        continue;
                    }
                }

                // Any other key cancels quit confirmation
                if app.quit_confirm {
                    app.quit_confirm = false;
                }

                // Global: Alt+F1/F2/F3 toggle panels, Alt+` cycle focus
                if key.modifiers.contains(KeyModifiers::ALT) {
                    match key.code {
                        KeyCode::F(1) => { app.toggle_panel(panels::PanelId::Code); continue; }
                        KeyCode::F(2) => { app.toggle_panel(panels::PanelId::Llm); continue; }
                        KeyCode::F(3) => { app.toggle_panel(panels::PanelId::Prompt); continue; }
                        KeyCode::Char('`') => { app.cycle_focus(); continue; }
                        _ => {}
                    }
                }

                // Delegate to focused panel
                app.handle_key(key);
            } else if let Event::Resize(_, _) = ev {
                // Terminal will redraw on next loop
            }
        }

        // Check for LLM stream updates
        app.poll_llm_updates();
    }
}
