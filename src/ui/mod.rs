use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::panels::code::CodeView;
use crate::panels::prompt::PromptView;
use crate::panels::PanelId;

pub fn draw(f: &mut Frame, app: &mut App) {
    let visible = app.visible_panels();
    if visible.is_empty() {
        let msg = Paragraph::new("No panels visible. Press Ctrl+1/2/3 to show a panel.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, f.area());
        return;
    }

    // Split area based on visible panel count
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

fn draw_code_panel(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(match app.code_panel.view {
            CodeView::Explorer => " Explorer ",
            CodeView::Editor => {
                " Editor "
            }
        });

    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.code_panel.view {
        CodeView::Explorer => draw_explorer(f, app, inner),
        CodeView::Editor => draw_editor(f, app, inner, focused),
    }
}

fn draw_explorer(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .code_panel
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let prefix = if entry.is_dir { "📁 " } else { "  " };
            let style = if i == app.code_panel.selected_idx {
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

fn draw_editor(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let height = area.height as usize;
    app.code_panel.adjust_scroll_for_height(height);

    let gutter_width = 4;
    let start = app.code_panel.scroll_offset;
    let end = (start + height).min(app.code_panel.lines.len());

    let lines: Vec<Line> = (start..end)
        .map(|i| {
            let line_num = format!("{:>3} ", i + 1);
            let content = &app.code_panel.lines[i];

            let is_selected = app.code_panel.select_anchor.map_or(false, |anchor| {
                let (lo, hi) = if anchor <= app.code_panel.cursor_row {
                    (anchor, app.code_panel.cursor_row)
                } else {
                    (app.code_panel.cursor_row, anchor)
                };
                i >= lo && i <= hi
            });

            let num_style = Style::default().fg(Color::DarkGray);
            let content_style = if is_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default().fg(Color::White)
            };

            Line::from(vec![
                Span::styled(line_num, num_style),
                Span::styled(content.to_string(), content_style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);

    // Cursor
    if focused {
        let cursor_y = area.y + (app.code_panel.cursor_row - app.code_panel.scroll_offset) as u16;
        let cursor_x = area.x + gutter_width + app.code_panel.cursor_col as u16;
        if cursor_y < area.y + area.height && cursor_x < area.x + area.width {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

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

    // Calculate visible range (scroll from bottom)
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
        PromptView::Compose => draw_prompt_compose(f, app, inner, focused),
        PromptView::History => draw_prompt_history(f, app, inner),
    }
}

fn draw_prompt_browser(f: &mut Frame, app: &App, area: Rect) {
    let panel = &app.prompt_panel;

    if panel.current_project.is_none() {
        // Show projects
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
        // Show threads
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
            "{} — Threads (Enter to open, Backspace to go back, Ctrl+N for new)",
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

fn draw_prompt_compose(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let panel = &app.prompt_panel;

    // Show pending references at top if any
    let mut lines: Vec<Line> = Vec::new();

    if !panel.pending_references.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("📎 {} pending refs", panel.pending_references.len()),
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
    }

    for (i, line) in panel.compose_lines.iter().enumerate() {
        let style = Style::default().fg(Color::White);
        lines.push(Line::from(Span::styled(line.to_string(), style)));
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);

    // Cursor
    if focused {
        let offset_y = if panel.pending_references.is_empty() {
            0
        } else {
            2
        };
        let cursor_y = area.y + offset_y + panel.compose_cursor_row as u16;
        let cursor_x = area.x + panel.compose_cursor_col as u16;
        if cursor_y < area.y + area.height && cursor_x < area.x + area.width {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

fn draw_prompt_history(f: &mut Frame, _app: &App, area: Rect) {
    let msg = Paragraph::new("Thread history will appear here.\nPress Esc to go back.")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(msg, area);
}
