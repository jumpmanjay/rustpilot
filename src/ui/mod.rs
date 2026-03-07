use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Overlay};
use crate::panels::code::{DiffLineKind, EditorMode, ScmStatus};
use crate::panels::editor::TextBuffer;
use crate::panels::prompt::PromptView;
use crate::panels::PanelId;

/// Layout:
/// ┌──────────┬─────────────────┬──────────────────┐
/// │ Explorer │    Editor       │   LLM (2/3)      │
/// │          │                 │                   │
/// │          │                 ├──────────────────┤
/// │          │                 │   Prompt (1/3)   │
/// └──────────┴─────────────────┴──────────────────┘
pub fn draw(f: &mut Frame, app: &mut App) {
    // Quit confirmation overlay
    if app.quit_confirm {
        let area = f.area();
        let unsaved = &app.quit_unsaved_files;

        let mut lines = Vec::new();
        if unsaved.is_empty() {
            lines.push(Line::from(Span::styled(
                "Quit RustPilot?",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("⚠ {} unsaved file(s):", unsaved.len()),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            for f_path in unsaved.iter().take(8) {
                lines.push(Line::from(Span::styled(
                    format!("  • {}", short_path(f_path)),
                    Style::default().fg(Color::Yellow),
                )));
            }
            if unsaved.len() > 8 {
                lines.push(Line::from(Span::styled(
                    format!("  ... and {} more", unsaved.len() - 8),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Ctrl+Q again to quit without saving, any other key to cancel",
            Style::default().fg(Color::DarkGray),
        )));

        let height = (lines.len() + 2) as u16; // +2 for borders
        let width = (area.width * 2 / 3).min(60);
        let center = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + area.height / 2 - height / 2,
            width,
            height,
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" Quit? ");
        f.render_widget(Clear, center);
        f.render_widget(Paragraph::new(lines).block(block), center);
        return;
    }

    let area = f.area();
    app.panel_rects.clear();

    let show_explorer = app.visible[PanelId::Explorer as usize];
    let show_editor = app.visible[PanelId::Editor as usize];
    let show_llm = app.visible[PanelId::Llm as usize];
    let show_prompt = app.visible[PanelId::Prompt as usize];
    let show_right = show_llm || show_prompt;

    // Nothing visible
    if !show_explorer && !show_editor && !show_right {
        let msg = Paragraph::new("No panels visible. Press Alt+F1/F2/F3/F4 to show a panel.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, area);
        return;
    }

    // Build horizontal constraints
    let mut h_constraints = Vec::new();
    let mut h_panels: Vec<&str> = Vec::new(); // track what's in each slot

    if show_explorer {
        h_constraints.push(Constraint::Length(30)); // fixed-width sidebar
        h_panels.push("explorer");
    }
    if show_editor {
        h_constraints.push(Constraint::Percentage(if show_right { 50 } else { 100 }));
        h_panels.push("editor");
    }
    if show_right {
        h_constraints.push(Constraint::Percentage(if show_editor { 50 } else { 100 }));
        h_panels.push("right");
    }

    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(h_constraints)
        .split(area);

    for (i, panel_name) in h_panels.iter().enumerate() {
        let chunk = h_chunks[i];
        match *panel_name {
            "explorer" => {
                app.panel_rects.push((PanelId::Explorer, chunk));
                match app.code_panel.mode {
                    EditorMode::Files => draw_explorer(f, app, chunk, app.focused == PanelId::Explorer),
                    EditorMode::SourceControl => draw_scm_explorer(f, app, chunk, app.focused == PanelId::Explorer),
                }
            }
            "editor" => {
                app.panel_rects.push((PanelId::Editor, chunk));
                match app.code_panel.mode {
                    EditorMode::Files => draw_editor(f, app, chunk, app.focused == PanelId::Editor),
                    EditorMode::SourceControl => draw_scm_diff(f, app, chunk, app.focused == PanelId::Editor),
                }
            }
            "right" => {
                // Split right pane vertically: LLM top 2/3, Prompt bottom 1/3
                if show_llm && show_prompt {
                    let v_chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Percentage(67),
                            Constraint::Percentage(33),
                        ])
                        .split(chunk);
                    app.panel_rects.push((PanelId::Llm, v_chunks[0]));
                    draw_llm_panel(f, app, v_chunks[0], app.focused == PanelId::Llm);
                    app.panel_rects.push((PanelId::Prompt, v_chunks[1]));
                    draw_prompt_panel(f, app, v_chunks[1], app.focused == PanelId::Prompt);
                } else if show_llm {
                    app.panel_rects.push((PanelId::Llm, chunk));
                    draw_llm_panel(f, app, chunk, app.focused == PanelId::Llm);
                } else if show_prompt {
                    app.panel_rects.push((PanelId::Prompt, chunk));
                    draw_prompt_panel(f, app, chunk, app.focused == PanelId::Prompt);
                }
            }
            _ => {}
        }
    }

    // Draw overlay on top of everything
    if app.overlay.is_some() {
        draw_overlay(f, app);
    }
}

fn panel_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

// ─── Explorer Panel ───

fn draw_explorer(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let title = format!(" {} ", short_path(&app.code_panel.cwd));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let selected = app.code_panel.selected_idx;

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

    f.render_widget(List::new(items), inner);
}

// ─── Editor Panel ───

fn draw_editor(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let path = app
        .code_panel
        .file_path
        .as_deref()
        .unwrap_or("(no file)");
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
    let title = format!(" {}{} —{}", short_path(path), modified, pos);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    app.code_panel.viewport_height = inner.height as usize;
    draw_text_buffer(f, &mut app.code_panel.buffer, inner, focused, true);
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

            if show_line_numbers {
                spans.push(Span::styled(
                    format!("{:>4} ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            if let Some((sel_start, sel_end)) = buf.selection_cols_for_row(i) {
                let s = sel_start.min(content.len());
                let e = sel_end.min(content.len());
                let before = &content[..s];
                let selected = &content[s..e];
                let after = &content[e..];

                if !before.is_empty() {
                    spans.push(Span::styled(before.to_string(), Style::default().fg(Color::White)));
                }
                if !selected.is_empty() {
                    spans.push(Span::styled(
                        selected.to_string(),
                        Style::default().bg(Color::DarkGray).fg(Color::White),
                    ));
                }
                if !after.is_empty() {
                    spans.push(Span::styled(after.to_string(), Style::default().fg(Color::White)));
                }
            } else {
                spans.push(Span::styled(content.to_string(), Style::default().fg(Color::White)));
            }

            Line::from(spans)
        })
        .collect();

    f.render_widget(Paragraph::new(lines), area);

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
    let status = if panel.streaming { "streaming..." } else { "idle" };
    let title = format!(" LLM [{}] in:{} out:{} ", status, panel.tokens_in, panel.tokens_out);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let end = panel.total_lines().saturating_sub(panel.scroll_offset);
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

    f.render_widget(
        Paragraph::new(visible_lines).wrap(Wrap { trim: false }),
        inner,
    );
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
            // Build header lines for pending refs and changed files
            let has_refs = !app.prompt_panel.pending_references.is_empty();
            let has_changed = !app.prompt_panel.changed_files.is_empty();
            let header_lines = (if has_refs { 1 } else { 0 }) + (if has_changed { 1 } else { 0 });

            let compose_area = if header_lines > 0 {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(header_lines as u16),
                        Constraint::Min(1),
                    ])
                    .split(inner);

                let mut header = Vec::new();
                if has_refs {
                    header.push(Line::from(Span::styled(
                        format!(
                            "📎 {} pending refs",
                            app.prompt_panel.pending_references.len()
                        ),
                        Style::default().fg(Color::Yellow),
                    )));
                }
                if has_changed {
                    let files: Vec<&str> = app
                        .prompt_panel
                        .changed_files
                        .iter()
                        .map(|f| short_path(f))
                        .collect();
                    header.push(Line::from(Span::styled(
                        format!("✏️  Changed: {}", files.join(", ")),
                        Style::default().fg(Color::Green),
                    )));
                }
                f.render_widget(Paragraph::new(header), chunks[0]);
                chunks[1]
            } else {
                inner
            };

            app.prompt_panel.viewport_height = compose_area.height as usize;
            draw_text_buffer(f, &mut app.prompt_panel.compose, compose_area, focused, false);
        }
        PromptView::History => {
            let msg = Paragraph::new("Thread history will appear here.\nPress Esc to go back.")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(msg, inner);
        }
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
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
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

        f.render_widget(
            List::new(items).block(
                Block::default()
                    .title(header)
                    .title_style(Style::default().fg(Color::DarkGray)),
            ),
            area,
        );
    } else {
        let items: Vec<ListItem> = panel
            .threads
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let style = if i == panel.selected_thread {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(format!("💬 {}", name), style)))
            })
            .collect();

        let header = format!(
            "{} — Threads (Enter, Backspace, Ctrl+N)",
            panel.current_project.as_deref().unwrap_or("")
        );

        f.render_widget(
            List::new(items).block(
                Block::default()
                    .title(header)
                    .title_style(Style::default().fg(Color::DarkGray)),
            ),
            area,
        );
    }
}

// ─── Helpers ───

// ─── Source Control Explorer ───

fn draw_scm_explorer(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let scm = &app.code_panel.scm;
    let title = format!(
        " SCM: {} [+{} ~{} ?{}] ",
        if scm.branch.is_empty() { "detached" } else { &scm.branch },
        scm.staged,
        scm.unstaged,
        scm.untracked
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if scm.entries.is_empty() {
        f.render_widget(
            Paragraph::new("No changes.\n\nPress 'r' to refresh\nCtrl+G to go back")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    let scroll = if scm.selected_idx >= height {
        scm.selected_idx - height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = scm
        .entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, entry)| {
            let (icon, color) = match entry.status {
                ScmStatus::Modified => ("M", Color::Yellow),
                ScmStatus::Added => ("A", Color::Green),
                ScmStatus::Deleted => ("D", Color::Red),
                ScmStatus::Renamed => ("R", Color::Blue),
                ScmStatus::Untracked => ("?", Color::DarkGray),
            };
            let staged_marker = if entry.staged { "●" } else { " " };
            let style = if i == scm.selected_idx {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(Span::styled(
                format!("{} {} {}", staged_marker, icon, entry.path),
                style,
            )))
        })
        .collect();

    f.render_widget(List::new(items), inner);
}

// ─── Source Control Diff View (Side-by-Side) ───

/// A row in the side-by-side diff
struct SideBySideRow {
    left_num: Option<usize>,
    left_text: String,
    left_kind: DiffLineKind,
    right_num: Option<usize>,
    right_text: String,
    right_kind: DiffLineKind,
}

fn build_side_by_side(diff_lines: &[crate::panels::code::DiffLine]) -> Vec<SideBySideRow> {
    let mut rows = Vec::new();
    let mut left_num: usize = 0;
    let mut right_num: usize = 0;

    // Collect removals and additions in chunks to pair them
    let mut i = 0;
    while i < diff_lines.len() {
        let dl = &diff_lines[i];
        match dl.kind {
            DiffLineKind::Header => {
                // Parse @@ -a,b +c,d @@ to get line numbers
                if dl.text.starts_with("@@") {
                    if let Some((l, r)) = parse_hunk_header(&dl.text) {
                        left_num = l.saturating_sub(1);
                        right_num = r.saturating_sub(1);
                    }
                }
                rows.push(SideBySideRow {
                    left_num: None,
                    left_text: dl.text.clone(),
                    left_kind: DiffLineKind::Header,
                    right_num: None,
                    right_text: dl.text.clone(),
                    right_kind: DiffLineKind::Header,
                });
                i += 1;
            }
            DiffLineKind::Context => {
                left_num += 1;
                right_num += 1;
                let text = dl.text.get(1..).unwrap_or(&dl.text).to_string();
                rows.push(SideBySideRow {
                    left_num: Some(left_num),
                    left_text: text.clone(),
                    left_kind: DiffLineKind::Context,
                    right_num: Some(right_num),
                    right_text: text,
                    right_kind: DiffLineKind::Context,
                });
                i += 1;
            }
            DiffLineKind::Removed | DiffLineKind::Added => {
                // Collect consecutive removed and added lines
                let mut removed = Vec::new();
                let mut added = Vec::new();
                while i < diff_lines.len() {
                    match diff_lines[i].kind {
                        DiffLineKind::Removed => {
                            removed.push(diff_lines[i].text.get(1..).unwrap_or("").to_string());
                            i += 1;
                        }
                        DiffLineKind::Added if removed.is_empty() || !added.is_empty() || {
                            // Check if we're still in the add portion after removes
                            let mut peek = i;
                            while peek < diff_lines.len() && diff_lines[peek].kind == DiffLineKind::Added {
                                peek += 1;
                            }
                            true
                        } => {
                            added.push(diff_lines[i].text.get(1..).unwrap_or("").to_string());
                            i += 1;
                        }
                        _ => break,
                    }
                }

                // Pair removed/added lines side by side
                let max = removed.len().max(added.len());
                for j in 0..max {
                    let has_left = j < removed.len();
                    let has_right = j < added.len();
                    if has_left {
                        left_num += 1;
                    }
                    if has_right {
                        right_num += 1;
                    }
                    rows.push(SideBySideRow {
                        left_num: if has_left { Some(left_num) } else { None },
                        left_text: if has_left { removed[j].clone() } else { String::new() },
                        left_kind: if has_left { DiffLineKind::Removed } else { DiffLineKind::Context },
                        right_num: if has_right { Some(right_num) } else { None },
                        right_text: if has_right { added[j].clone() } else { String::new() },
                        right_kind: if has_right { DiffLineKind::Added } else { DiffLineKind::Context },
                    });
                }
            }
        }
    }
    rows
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize)> {
    // Parse "@@ -a,b +c,d @@" → (a, c)
    let parts: Vec<&str> = header.split_whitespace().collect();
    let left = parts.get(1)?;
    let right = parts.get(2)?;
    let left_start: usize = left.trim_start_matches('-').split(',').next()?.parse().ok()?;
    let right_start: usize = right.trim_start_matches('+').split(',').next()?.parse().ok()?;
    Some((left_start, right_start))
}

fn draw_scm_diff(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let scm = &app.code_panel.scm;
    let title = if let Some(entry) = scm.entries.get(scm.selected_idx) {
        format!(" Diff: {} ", entry.path)
    } else {
        " Diff ".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if scm.diff_lines.is_empty() {
        f.render_widget(
            Paragraph::new("Select a file to view diff")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let sbs_rows = build_side_by_side(&scm.diff_lines);
    let height = inner.height as usize;
    let start = scm.diff_scroll.min(sbs_rows.len().saturating_sub(1));
    let end = (start + height).min(sbs_rows.len());

    // Split into left and right halves
    let half_width = inner.width as usize / 2;
    let gutter = 5; // line number width

    let lines: Vec<Line> = sbs_rows[start..end]
        .iter()
        .map(|row| {
            let content_width = half_width.saturating_sub(gutter + 1); // +1 for separator

            // Left side
            let left_num_str = row.left_num.map(|n| format!("{:>4} ", n)).unwrap_or_else(|| "     ".to_string());
            let left_text = truncate_str(&row.left_text, content_width);
            let left_pad = content_width.saturating_sub(left_text.len());

            // Right side
            let right_num_str = row.right_num.map(|n| format!("{:>4} ", n)).unwrap_or_else(|| "     ".to_string());
            let right_text = truncate_str(&row.right_text, content_width);

            let left_style = match row.left_kind {
                DiffLineKind::Removed => Style::default().fg(Color::Red).bg(Color::Rgb(40, 0, 0)),
                DiffLineKind::Header => Style::default().fg(Color::Cyan),
                _ => Style::default().fg(Color::White),
            };
            let right_style = match row.right_kind {
                DiffLineKind::Added => Style::default().fg(Color::Green).bg(Color::Rgb(0, 30, 0)),
                DiffLineKind::Header => Style::default().fg(Color::Cyan),
                _ => Style::default().fg(Color::White),
            };

            Line::from(vec![
                Span::styled(left_num_str, Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}{}", left_text, " ".repeat(left_pad)), left_style),
                Span::styled("│", Style::default().fg(Color::DarkGray)),
                Span::styled(right_num_str, Style::default().fg(Color::DarkGray)),
                Span::styled(right_text.to_string(), right_style),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else if max > 0 {
        &s[..max]
    } else {
        ""
    }
}

fn short_path(path: &str) -> &str {
    let parts: Vec<&str> = path.rsplit('/').take(3).collect();
    let start = path.len().saturating_sub(parts.iter().map(|p| p.len() + 1).sum::<usize>());
    &path[start..]
}

// ─── Overlay Rendering ───

fn draw_overlay(f: &mut Frame, app: &App) {
    let area = f.area();
    let width = (area.width * 2 / 3).min(80).max(40);
    let height = (area.height / 2).min(20).max(8);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + 2; // near top, like VS Code

    let overlay_area = Rect::new(x, y, width, height);
    f.render_widget(Clear, overlay_area);

    match &app.overlay {
        Some(Overlay::FileFinder {
            query,
            results,
            selected,
        }) => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Find File (Ctrl+P) ");
            let inner = block.inner(overlay_area);
            f.render_widget(block, overlay_area);

            // Search input + results
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);

            // Input line
            let input = Paragraph::new(Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Cyan)),
                Span::styled(query.as_str(), Style::default().fg(Color::White)),
                Span::styled("█", Style::default().fg(Color::Gray)),
            ]));
            f.render_widget(input, chunks[0]);

            // Results
            let max_visible = chunks[1].height as usize;
            let items: Vec<ListItem> = results
                .iter()
                .enumerate()
                .take(max_visible)
                .map(|(i, path)| {
                    let style = if i == *selected {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(Line::from(Span::styled(path.as_str(), style)))
                })
                .collect();
            f.render_widget(List::new(items), chunks[1]);
        }

        Some(Overlay::FindInFile {
            query,
            matches,
            current,
        }) => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(format!(
                    " Find in File — {}/{} matches ",
                    if matches.is_empty() { 0 } else { *current + 1 },
                    matches.len()
                ));
            let inner = block.inner(overlay_area);
            f.render_widget(block, overlay_area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);

            let input = Paragraph::new(Line::from(vec![
                Span::styled("🔍 ", Style::default().fg(Color::Yellow)),
                Span::styled(query.as_str(), Style::default().fg(Color::White)),
                Span::styled("█", Style::default().fg(Color::Gray)),
            ]));
            f.render_widget(input, chunks[0]);

            // Show matches with context
            let max_visible = chunks[1].height as usize;
            let items: Vec<ListItem> = matches
                .iter()
                .enumerate()
                .take(max_visible)
                .map(|(i, (row, col))| {
                    let style = if i == *current {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let line_text = app
                        .code_panel
                        .buffer
                        .lines
                        .get(*row)
                        .map(|l| l.trim())
                        .unwrap_or("");
                    ListItem::new(Line::from(Span::styled(
                        format!("  {}:{} {}", row + 1, col + 1, line_text),
                        style,
                    )))
                })
                .collect();
            f.render_widget(List::new(items), chunks[1]);
        }

        Some(Overlay::FindInWorkspace {
            query,
            results,
            selected,
        }) => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(format!(" Find in Workspace — {} results ", results.len()));
            let inner = block.inner(overlay_area);
            f.render_widget(block, overlay_area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);

            let input = Paragraph::new(Line::from(vec![
                Span::styled("🔍 ", Style::default().fg(Color::Magenta)),
                Span::styled(query.as_str(), Style::default().fg(Color::White)),
                Span::styled("█", Style::default().fg(Color::Gray)),
            ]));
            f.render_widget(input, chunks[0]);

            let max_visible = chunks[1].height as usize;
            let items: Vec<ListItem> = results
                .iter()
                .enumerate()
                .take(max_visible)
                .map(|(i, result)| {
                    let style = if i == *selected {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let text = format!(
                        "  {}:{} {}",
                        result.path,
                        result.line_num,
                        result.line_text.trim()
                    );
                    // Truncate long lines
                    let truncated = if text.len() > width as usize - 4 {
                        format!("{}…", &text[..width as usize - 5])
                    } else {
                        text
                    };
                    ListItem::new(Line::from(Span::styled(truncated, style)))
                })
                .collect();
            f.render_widget(List::new(items), chunks[1]);
        }

        None => {}
    }
}
