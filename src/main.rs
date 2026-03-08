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

                // Handle save-as overlay
                if app.save_as_input.is_some() {
                    match key.code {
                        KeyCode::Esc => { app.save_as_input = None; }
                        KeyCode::Enter => {
                            if let Some(ref input) = app.save_as_input.clone() {
                                if !input.is_empty() {
                                    let path = if input.starts_with('/') {
                                        input.clone()
                                    } else {
                                        format!("{}/{}", app.code_panel.cwd, input)
                                    };
                                    app.code_panel.save_file_as(&path);
                                }
                            }
                            app.save_as_input = None;
                        }
                        KeyCode::Char(c) => {
                            if let Some(ref mut input) = app.save_as_input {
                                input.push(c);
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(ref mut input) = app.save_as_input {
                                input.pop();
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                // Handle menu
                if let Some(ref mut menu) = app.menu.clone() {
                    match key.code {
                        KeyCode::Esc => { app.menu = None; }
                        KeyCode::Left => {
                            let mut m = menu.clone();
                            m.active_menu = if m.active_menu == 0 { 2 } else { m.active_menu - 1 };
                            m.selected_item = 0;
                            app.menu = Some(m);
                        }
                        KeyCode::Right => {
                            let mut m = menu.clone();
                            m.active_menu = (m.active_menu + 1) % 3;
                            m.selected_item = 0;
                            app.menu = Some(m);
                        }
                        KeyCode::Up => {
                            let mut m = menu.clone();
                            m.selected_item = m.selected_item.saturating_sub(1);
                            app.menu = Some(m);
                        }
                        KeyCode::Down => {
                            let mut m = menu.clone();
                            m.selected_item += 1;
                            app.menu = Some(m);
                        }
                        KeyCode::Enter => {
                            let active = menu.active_menu;
                            let item = menu.selected_item;
                            app.menu = None;
                            match (active, item) {
                                // File menu
                                (0, 0) => { app.code_panel.new_file(); }          // New
                                (0, 1) => { app.open_file_finder(); }             // Open
                                (0, 2) => {                                        // Save
                                    if app.code_panel.file_path.is_some() {
                                        // save existing
                                        let path = app.code_panel.file_path.clone().unwrap();
                                        let content = app.code_panel.buffer.to_string();
                                        if std::fs::write(&path, &content).is_ok() {
                                            app.code_panel.buffer.modified = false;
                                        }
                                    } else {
                                        app.save_as_input = Some(String::new());
                                    }
                                }
                                (0, 3) => { app.save_as_input = Some(String::new()); } // Save As
                                (0, 4) => { app.code_panel.close_current_tab(); }      // Close Tab
                                (0, 5) => {                                             // Quit
                                    app.quit_unsaved_files = app.unsaved_files();
                                    app.quit_confirm = true;
                                }
                                // Edit menu
                                (1, 0) => { app.code_panel.buffer.undo(); }
                                (1, 1) => { app.code_panel.buffer.redo(); }
                                (1, 2) => { app.code_panel.buffer.copy(); }
                                (1, 3) => { app.code_panel.buffer.cut(); }
                                (1, 4) => { app.code_panel.buffer.paste(); }
                                (1, 5) => { app.code_panel.buffer.select_all(); }
                                // View menu
                                (2, 0) => { app.toggle_panel(panels::PanelId::Explorer); }
                                (2, 1) => { app.toggle_panel(panels::PanelId::Llm); }
                                (2, 2) => { app.toggle_panel(panels::PanelId::Prompt); }
                                (2, 3) => { app.terminal_panel.toggle(); }
                                (2, 4) => { app.code_panel.show_hidden = !app.code_panel.show_hidden; app.code_panel.refresh_entries(); }
                                _ => {}
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

                // F10 / F1 = menu
                if matches!(key.code, KeyCode::F(10) | KeyCode::F(1)) && !key.modifiers.contains(KeyModifiers::ALT) {
                    app.menu = Some(app::MenuState { active_menu: 0, selected_item: 0, open: true });
                    continue;
                }

                // Ctrl+Alt+arrows: resize panels
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::ALT) {
                    match key.code {
                        KeyCode::Left => { app.adjust_explorer_width(-3); continue; }
                        KeyCode::Right => { app.adjust_explorer_width(3); continue; }
                        KeyCode::Up => { app.adjust_right_pane(5); continue; }
                        KeyCode::Down => { app.adjust_right_pane(-5); continue; }
                        _ => {}
                    }
                }

                // Global: Ctrl+keybinds
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('p') => { app.open_file_finder(); continue; }
                        KeyCode::Char('f') => { app.open_find_in_file(); continue; }
                        KeyCode::Char('F') => { app.open_find_in_workspace(); continue; }
                        KeyCode::Char('n') => { app.code_panel.new_file(); continue; }
                        KeyCode::Char('w') => { app.code_panel.close_current_tab(); continue; }
                        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            // Ctrl+Shift+Tab: prev tab
                            if app.code_panel.tab_scroll > 0 {
                                app.code_panel.tab_scroll -= 1;
                            }
                            let paths = app.code_panel.open_buffer_paths();
                            if paths.len() > 1 {
                                let current = app.code_panel.file_path.as_deref().unwrap_or("");
                                let idx = paths.iter().position(|p| p == current).unwrap_or(0);
                                let prev = if idx == 0 { paths.len() - 1 } else { idx - 1 };
                                if let Some(path) = paths.get(prev).cloned() {
                                    app.code_panel.switch_to_buffer(&path);
                                }
                            }
                            continue;
                        }
                        KeyCode::Tab => {
                            // Ctrl+Tab: next tab
                            let paths = app.code_panel.open_buffer_paths();
                            if paths.len() > 1 {
                                let current = app.code_panel.file_path.as_deref().unwrap_or("");
                                let idx = paths.iter().position(|p| p == current).unwrap_or(0);
                                let next = (idx + 1) % paths.len();
                                if let Some(path) = paths.get(next).cloned() {
                                    app.code_panel.switch_to_buffer(&path);
                                }
                            }
                            continue;
                        }
                        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            // Ctrl+Shift+S: Save As
                            app.save_as_input = Some(String::new());
                            continue;
                        }
                        KeyCode::Char('s') => {
                            // Ctrl+S: Save
                            if let Some(ref path) = app.code_panel.file_path.clone() {
                                let content = app.code_panel.buffer.to_string();
                                if std::fs::write(path, &content).is_ok() {
                                    app.code_panel.buffer.modified = false;
                                }
                            } else {
                                app.save_as_input = Some(String::new());
                            }
                            continue;
                        }
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

        // Auto-save
        app.auto_save();
    }
}
