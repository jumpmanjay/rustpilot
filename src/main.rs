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
                        app.quit_unsaved_files = app.unsaved_files();
                        app.quit_confirm = true;
                        continue;
                    }
                }

                // Any other key cancels quit confirmation
                if app.quit_confirm {
                    app.quit_confirm = false;
                }

                // Handle go-to-line overlay
                if app.goto_line_input.is_some() {
                    match key.code {
                        KeyCode::Esc => { app.goto_line_input = None; }
                        KeyCode::Enter => {
                            if let Some(ref input) = app.goto_line_input {
                                if let Ok(line) = input.parse::<usize>() {
                                    app.code_panel.buffer.go_to_line(line);
                                }
                            }
                            app.goto_line_input = None;
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            if let Some(ref mut input) = app.goto_line_input {
                                input.push(c);
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(ref mut input) = app.goto_line_input {
                                input.pop();
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                // Handle overlay input (file finder, search, etc.)
                if app.overlay.is_some() {
                    app.handle_overlay_key(key);
                    continue;
                }

                // Terminal focus: when terminal is focused, send keys to it
                if app.focused == panels::PanelId::Terminal {
                    match key.code {
                        KeyCode::Esc => {
                            app.focused = panels::PanelId::Editor;
                        }
                        KeyCode::Char('`') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.terminal_panel.toggle();
                            if !app.terminal_panel.visible {
                                app.focused = panels::PanelId::Editor;
                            }
                        }
                        KeyCode::Char(c) => {
                            app.terminal_panel.handle_input_char(c);
                        }
                        KeyCode::Backspace => {
                            app.terminal_panel.handle_backspace();
                        }
                        KeyCode::Enter => {
                            let cwd = app.code_panel.cwd.clone();
                            app.terminal_panel.handle_enter(&cwd);
                        }
                        KeyCode::Up => { app.terminal_panel.scroll_up(1); }
                        KeyCode::Down => { app.terminal_panel.scroll_down(1); }
                        KeyCode::PageUp => { app.terminal_panel.scroll_up(10); }
                        KeyCode::PageDown => { app.terminal_panel.scroll_down(10); }
                        _ => {}
                    }
                    continue;
                }

                // Global: Ctrl+P file finder, Ctrl+F find, Ctrl+Shift+F workspace search, Ctrl+G go-to-line/SCM
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('p') => { app.open_file_finder(); continue; }
                        KeyCode::Char('f') => { app.open_find_in_file(); continue; }
                        KeyCode::Char('F') => { app.open_find_in_workspace(); continue; }
                        KeyCode::Char('g') => {
                            // Ctrl+G: go to line (when editor focused) or toggle SCM
                            if app.focused == panels::PanelId::Editor {
                                app.goto_line_input = Some(String::new());
                            } else {
                                app.code_panel.toggle_mode();
                            }
                            continue;
                        }
                        KeyCode::Char('`') => {
                            // Ctrl+`: toggle terminal
                            app.terminal_panel.toggle();
                            if app.terminal_panel.visible {
                                app.focused = panels::PanelId::Terminal;
                            } else if app.focused == panels::PanelId::Terminal {
                                app.focused = panels::PanelId::Editor;
                            }
                            continue;
                        }
                        KeyCode::Char('\\') => {
                            // Ctrl+\: split editor
                            if app.split_editor.is_some() {
                                app.split_editor = None;
                            } else if let Some(ref path) = app.code_panel.file_path {
                                let buf = crate::panels::editor::TextBuffer::from_string(
                                    &app.code_panel.buffer.to_string(),
                                );
                                app.split_editor = Some(app::SplitEditor {
                                    file_path: path.clone(),
                                    buffer: buf,
                                    focused: false,
                                });
                            }
                            continue;
                        }
                        _ => {}
                    }
                }

                // Global: Alt+F1/F2/F3/F4 toggle panels, Alt+` cycle focus
                if key.modifiers.contains(KeyModifiers::ALT) {
                    match key.code {
                        KeyCode::F(1) => { app.toggle_panel(panels::PanelId::Explorer); continue; }
                        KeyCode::F(2) => { app.toggle_panel(panels::PanelId::Editor); continue; }
                        KeyCode::F(3) => { app.toggle_panel(panels::PanelId::Llm); continue; }
                        KeyCode::F(4) => { app.toggle_panel(panels::PanelId::Prompt); continue; }
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
