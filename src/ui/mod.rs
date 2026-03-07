use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::panels::code::CodeView;
use crate::panels::editor::TextBuffer;
use crate::panels::prompt::PromptView;
use crate::panels::PanelId;

pub fn draw(f: &mut Frame, app: &mut App) {
    // Quit confirmation overlay
    if app.quit_confirm {
        let area = f.area();
        let msg = Paragraph::new(
            "Quit RustPilot? Press Ctrl+Q again to confirm, any other key to cancel.",
        )
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
        let center = Rect::new(area.x + area.width / 4, area.y + area.height / 2 - 1, area.width / 2, 3);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" Quit? ");
        f.render_widget(Clear, center);
        f.render_widget(msg.block(block), center);
        return;
    }

    let visible = app.visible_panels();
    if visible.is_empty() {
        let msg = Paragraph::new("No panels visible. Press Alt+F1/F2/F3 to show a panel.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, f.area());
        return;
    }

    let constraints: Vec<Constraint> = visible
        .iter()
        .map(|_| Constraint::Percentage(100 / visible.len() as u16))
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(f.area());

    for (i, panel_id) in visible.iter().enumerate() {
        let is_focused = *panel_id == app.focused;
        let area = chunks[i];

        match panel_id {
            PanelId::Code => draw_code_panel(f, app, area, is_focused),
            PanelId::Llm => draw_llm_panel(f, app, area, is_focused),
            PanelId::Prompt => draw_prompt_panel(f, app, area, is_focused),
        }
    }
}

fn panel_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

// ─── Code Panel ───

fn draw_code_panel(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let title = match app.code_panel.view {
        CodeView::Explorer => format!(" Explorer: {} ", short_path(&app.code_panel.cwd)),
        CodeView::Editor => {
            let path = app
                .code_panel
                .file_path
                .as_deref()
                .unwrap_or("untitled");
            let modified = if app.code_panel.buffer.modified {
                " [+]"
            } else {
                ""
            };
            let pos = format!(
                " Ln {}, Col {} ",
                app.code_panel.buffer.cursor_row + 1,
                app.code_panel.buffer.cursor_col + 1
            );
            format!(" {}{} —{}", short_path(path), modified, pos)
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.code_panel.view {
        CodeView::Explorer => draw_explorer(f, app, inner),
        CodeView::Editor => {
            app.code_panel.viewport_height = inner.height as usize;
            draw_text_buffer(f, &mut app.code_panel.buffer, inner, focused, true);
        }
    }
}

fn draw_explorer(f: &mut Frame, app: &App, area: Rect) {
    let height = area.height as usize;
    let total = app.code_panel.entries.len();
    let selected = app.code_panel.selected_idx;

    // Scroll to keep selected visible
    let scroll = if selected >= height {
        selected - height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = app
        .code_panel
        .entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, entry)| {
            let prefix = if entry.is_dir { "📁 " } else { "  " };
            let style = if i == selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                Style::default().fg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(
                format!("{}{}", prefix, entry.name),
                style,
            )))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, area);
}

// ─── Shared TextBuffer rendering ───

fn draw_text_buffer(
    f: &mut Frame,
    buf: &mut TextBuffer,
    area: Rect,
    focused: bool,
    show_line_numbers: bool,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    let gutter_width: usize = if show_line_numbers { 5 } else { 0 };

    buf.adjust_scroll(height, width);

    let start = buf.scroll_row;
    let end = (start + height).min(buf.lines.len());

    let lines: Vec<Line> = (start..end)
        .map(|i| {
            let content = &buf.lines[i];

            let mut spans = Vec::new();

            // Line number gutter
            if show_line_numbers {
                spans.push(Span::styled(
                    format!("{:>4} ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // Content with selection highlighting
            if let Some((sel_start, sel_end)) = buf.selection_cols_for_row(i) {
                let before = &content[..sel_start.min(content.len())];
                let selected = &content
                    [sel_start.min(content.len())..sel_end.min(content.len())];
                let after = &content[sel_end.min(content.len())..];

                if !before.is_empty() {
                    spans.push(Span::styled(
                        before.to_string(),
                        Style::default().fg(Color::White),
                    ));
                }
                if !selected.is_empty() {
                    spans.push(Span::styled(
                        selected.to_string(),
                        Style::default()
                            .bg(Color::DarkGray)
                            .fg(Color::White),
                    ));
                }
                if !after.is_empty() {
                    spans.push(Span::styled(
                        after.to_string(),
                        Style::default().fg(Color::White),
                    ));
                }
            } else {
                spans.push(Span::styled(
                    content.to_string(),
                    Style::default().fg(Color::White),
                ));
            }

            Line::from(spans)
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);

    // Cursor
    if focused {
        let cursor_y = area.y + (buf.cursor_row.saturating_sub(buf.scroll_row)) as u16;
        let cursor_x =
            area.x + gutter_width as u16 + (buf.cursor_col.saturating_sub(buf.scroll_col)) as u16;
        if cursor_y < area.y + area.height && cursor_x < area.x + area.width {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

// ─── LLM Panel ───

fn draw_llm_panel(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let panel = &app.llm_panel;
    let status = if panel.streaming {
        "streaming..."
    } else {
        "idle"
    };
    let title = format!(
        " LLM [{}] in:{} out:{} ",
        status, panel.tokens_in, panel.tokens_out
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let total = panel.total_lines();

    let end = total.saturating_sub(panel.scroll_offset);
    let start = end.saturating_sub(height);

    let mut visible_lines: Vec<Line> = Vec::new();
    for i in start..end {
        let text = if i < panel.lines.len() {
            &panel.lines[i]
        } else {
            &panel.current_line
        };
        visible_lines.push(Line::from(Span::styled(
            text.to_string(),
            Style::default().fg(Color::White),
        )));
    }

    let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
    f.render_widget(paragraph, inner);
}

// ─── Prompt Panel ───

fn draw_prompt_panel(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let title = match app.prompt_panel.view {
        PromptView::Browser => " Prompts ",
        PromptView::Compose => " Compose (Ctrl+Enter to send) ",
        PromptView::History => " History ",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.prompt_panel.view {
        PromptView::Browser => draw_prompt_browser(f, app, inner),
        PromptView::Compose => {
            // Show pending refs header
            let compose_area = if !app.prompt_panel.pending_references.is_empty() {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(2), Constraint::Min(1)])
                    .split(inner);

                let refs_text = format!(
                    "📎 {} pending refs (Enter thread to insert)",
                    app.prompt_panel.pending_references.len()
                );
                let refs_para = Paragraph::new(refs_text)
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(refs_para, chunks[0]);
                chunks[1]
            } else {
                inner
            };

            app.prompt_panel.viewport_height = compose_area.height as usize;
            draw_text_buffer(f, &mut app.prompt_panel.compose, compose_area, focused, false);
        }
        PromptView::History => draw_prompt_history(f, app, inner),
    }
}

fn draw_prompt_browser(f: &mut Frame, app: &App, area: Rect) {
    let panel = &app.prompt_panel;

    if panel.current_project.is_none() {
        let items: Vec<ListItem> = panel
            .projects
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let style = if i == panel.selected_project {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(format!("📁 {}", name), style)))
            })
            .collect();

        let header = if items.is_empty() {
            "No projects. Press Ctrl+N to create one."
        } else {
            "Projects (Enter to select, Ctrl+N for new)"
        };

        let list = List::new(items).block(
            Block::default()
                .title(header)
                .title_style(Style::default().fg(Color::DarkGray)),
        );
        f.render_widget(list, area);
    } else {
        let items: Vec<ListItem> = panel
            .threads
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let style = if i == panel.selected_thread {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(format!("💬 {}", name), style)))
            })
            .collect();

        let header = format!(
            "{} — Threads (Enter to open, Backspace back, Ctrl+N new)",
            panel.current_project.as_deref().unwrap_or("")
        );

        let list = List::new(items).block(
            Block::default()
                .title(header)
                .title_style(Style::default().fg(Color::DarkGray)),
        );
        f.render_widget(list, area);
    }
}

fn draw_prompt_history(f: &mut Frame, _app: &App, area: Rect) {
    let msg = Paragraph::new("Thread history will appear here.\nPress Esc to go back.")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(msg, area);
}

// ─── Helpers ───

fn short_path(path: &str) -> &str {
    // Show last 2 path components
    let parts: Vec<&str> = path.rsplit('/').take(3).collect();
    let start = path.len().saturating_sub(parts.iter().map(|p| p.len() + 1).sum::<usize>());
    &path[start..]
}
