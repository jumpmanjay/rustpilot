mod syntax;

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

use self::syntax::SyntaxHighlighter;

lazy_static::lazy_static! {
    static ref HIGHLIGHTER: SyntaxHighlighter = SyntaxHighlighter::new();
}

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

    let full_area = f.area();
    app.panel_rects.clear();

    // Menu bar (1 row at top)
    let has_menu = true; // always show menu bar
    let (menu_area, area) = if has_menu {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(full_area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, full_area)
    };

    if let Some(ma) = menu_area {
        draw_menu_bar(f, app, ma);
    }

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

    // Build horizontal constraints (using configurable sizes)
    let mut h_constraints = Vec::new();
    let mut h_panels: Vec<&str> = Vec::new();

    if show_explorer {
        h_constraints.push(Constraint::Length(app.explorer_width));
        h_panels.push("explorer");
    }
    if show_editor {
        let pct = if show_right { 100 - app.right_pane_percent } else { 100 };
        h_constraints.push(Constraint::Percentage(pct));
        h_panels.push("editor");
    }
    if show_right {
        let pct = if show_editor { app.right_pane_percent } else { 100 };
        h_constraints.push(Constraint::Percentage(pct));
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
                // Split: editor | terminal (if terminal visible)
                let (editor_chunk, terminal_chunk) = if app.terminal_panel.visible {
                    let v = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
                        .split(chunk);
                    (v[0], Some(v[1]))
                } else {
                    (chunk, None)
                };

                // Split editor: left | right (if split active)
                let (left_chunk, right_chunk) = if app.split_editor.is_some() {
                    let h = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(editor_chunk);
                    (h[0], Some(h[1]))
                } else {
                    (editor_chunk, None)
                };

                app.panel_rects.push((PanelId::Editor, left_chunk));
                match app.code_panel.mode {
                    EditorMode::Files => draw_editor(f, app, left_chunk, app.focused == PanelId::Editor),
                    EditorMode::SourceControl => draw_scm_diff(f, app, left_chunk, app.focused == PanelId::Editor),
                }

                // Right split
                if let Some(rc) = right_chunk {
                    draw_split_editor(f, app, rc);
                }

                // Terminal panel
                if let Some(tc) = terminal_chunk {
                    app.panel_rects.push((PanelId::Terminal, tc));
                    draw_terminal_panel(f, app, tc, app.focused == PanelId::Terminal);
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

    // Save As overlay
    if let Some(ref input) = app.save_as_input {
        let area = f.area();
        let width = 50u16;
        let height = 3u16;
        let popup = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + 2,
            width,
            height,
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Save As ");
        f.render_widget(Clear, popup);
        let text = format!("{}", input);
        f.render_widget(
            Paragraph::new(text).block(block),
            popup,
        );
    }

    // Menu dropdown
    if let Some(ref menu) = app.menu {
        draw_menu_dropdown(f, menu);
    }

    // Go-to-line overlay
    if let Some(ref input) = app.goto_line_input {
        let area = f.area();
        let width = 30u16;
        let height = 3u16;
        let popup = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + 2,
            width,
            height,
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Go to Line (Ctrl+G) ");
        f.render_widget(Clear, popup);
        let text = format!(":{}", input);
        f.render_widget(
            Paragraph::new(text).block(block),
            popup,
        );
    }
}

// ─── Menu Bar ───

fn draw_menu_bar(f: &mut Frame, app: &App, area: Rect) {
    let menus = ["  File ", " Edit ", " View "];
    let is_open = app.menu.is_some();
    let active = app.menu.as_ref().map(|m| m.active_menu).unwrap_or(usize::MAX);

    let mut spans = Vec::new();
    for (i, label) in menus.iter().enumerate() {
        let style = if is_open && i == active {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::Gray).bg(Color::Rgb(30, 30, 40))
        };
        spans.push(Span::styled(*label, style));
    }

    // Fill rest of bar
    let used: usize = menus.iter().map(|m| m.len()).sum();
    let remaining = (area.width as usize).saturating_sub(used);
    spans.push(Span::styled(
        " ".repeat(remaining),
        Style::default().bg(Color::Rgb(30, 30, 40)),
    ));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_menu_dropdown(f: &mut Frame, menu: &crate::app::MenuState) {
    let items: Vec<Vec<(&str, &str)>> = vec![
        // File
        vec![
            ("New File", "Ctrl+N"),
            ("Open File", "Ctrl+P"),
            ("Save", "Ctrl+S"),
            ("Save As...", "Ctrl+Shift+S"),
            ("Close Tab", "Ctrl+W"),
            ("Quit", "Ctrl+Q"),
        ],
        // Edit
        vec![
            ("Undo", "Ctrl+Z"),
            ("Redo", "Ctrl+Y"),
            ("Copy", "Ctrl+C"),
            ("Cut", "Ctrl+X"),
            ("Paste", "Ctrl+V"),
            ("Select All", "Ctrl+A"),
        ],
        // View
        vec![
            ("Toggle Explorer", "Alt+F1"),
            ("Toggle LLM Panel", "Alt+F3"),
            ("Toggle Prompt Panel", "Alt+F4"),
            ("Toggle Terminal", "Ctrl+`"),
            ("Toggle Hidden Files", ""),
        ],
    ];

    let active = menu.active_menu.min(items.len() - 1);
    let menu_items = &items[active];

    let width = 30u16;
    let height = (menu_items.len() + 2) as u16;
    let x_offset: u16 = match active {
        0 => 0,
        1 => 7,
        2 => 14,
        _ => 0,
    };

    let popup = Rect::new(x_offset, 1, width, height);
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lines: Vec<Line> = menu_items
        .iter()
        .enumerate()
        .map(|(i, (label, shortcut))| {
            let style = if i == menu.selected_item {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default().fg(Color::White)
            };

            let pad = (inner.width as usize).saturating_sub(label.len() + shortcut.len() + 1);
            Line::from(vec![
                Span::styled(format!(" {}", label), style),
                Span::styled(" ".repeat(pad), style),
                Span::styled(format!("{} ", shortcut), Style::default().fg(Color::DarkGray).bg(if i == menu.selected_item { Color::White } else { Color::Reset })),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner into: tab bar (1) | editor | status bar (1)
    let tab_paths = app.code_panel.open_buffer_paths();
    let has_tabs = tab_paths.len() > 1 || app.code_panel.file_path.is_some();
    let tab_height = if has_tabs { 1u16 } else { 0 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(tab_height),
            Constraint::Min(1),
            Constraint::Length(1), // status bar
        ])
        .split(inner);

    // ── Tab bar ──
    if has_tabs {
        draw_tab_bar(f, app, chunks[0]);
    }

    // ── Editor area (with optional minimap) ──
    let editor_area = chunks[1];
    let minimap_width: u16 = if editor_area.width > 80 { 12 } else { 0 };

    let (edit_rect, minimap_rect) = if minimap_width > 0 {
        let h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(minimap_width),
            ])
            .split(editor_area);
        (h[0], Some(h[1]))
    } else {
        (editor_area, None)
    };

    app.code_panel.viewport_height = edit_rect.height as usize;

    let ext = app.code_panel.file_path.as_deref()
        .and_then(|p| std::path::Path::new(p).extension())
        .and_then(|e| e.to_str())
        .unwrap_or("");

    draw_text_buffer_highlighted(f, &mut app.code_panel.buffer, edit_rect, focused, true, ext);

    // ── Minimap ──
    if let Some(mr) = minimap_rect {
        draw_minimap(f, &app.code_panel.buffer, mr);
    }

    // ── Status bar ──
    draw_status_bar(f, app, chunks[2]);
}

fn draw_tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let paths = app.code_panel.open_buffer_paths();
    let current = app.code_panel.file_path.as_deref().unwrap_or("");

    if paths.is_empty() && app.code_panel.file_path.is_none() {
        // Show "untitled"
        let spans = vec![
            Span::styled(" untitled● ", Style::default().fg(Color::White).bg(Color::DarkGray)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    let mut spans = Vec::new();

    // Left scroll arrow
    if app.code_panel.tab_scroll > 0 {
        spans.push(Span::styled("◀ ", Style::default().fg(Color::Yellow)));
    }

    let visible_paths: Vec<&String> = paths.iter().skip(app.code_panel.tab_scroll).collect();

    for path in &visible_paths {
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);

        let is_modified = if path.as_str() == current {
            app.code_panel.buffer.modified
        } else {
            app.code_panel.open_buffers.get(path.as_str()).map_or(false, |b| b.modified)
        };

        let label = if is_modified {
            format!(" {}● ", name)
        } else {
            format!(" {} ", name)
        };

        let style = if path.as_str() == current {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Gray)
        };

        spans.push(Span::styled(label, style));
        spans.push(Span::styled("│", Style::default().fg(Color::Rgb(60, 60, 60))));
    }

    // Right scroll indicator
    if app.code_panel.tab_scroll + visible_paths.len() < paths.len() {
        spans.push(Span::styled(" ▶", Style::default().fg(Color::Yellow)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let buf = &app.code_panel.buffer;
    let path = app.code_panel.file_path.as_deref().unwrap_or("(no file)");

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("plain");

    let lang = match ext {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "c" | "h" => "C",
        "cpp" | "hpp" => "C++",
        "java" => "Java",
        "rb" => "Ruby",
        "sh" | "bash" => "Shell",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "json" => "JSON",
        "md" => "Markdown",
        "html" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        "lua" => "Lua",
        "zig" => "Zig",
        _ => ext,
    };

    let branch = &app.code_panel.scm.branch;
    let git_info = if branch.is_empty() {
        String::new()
    } else {
        format!("  {} ", branch)
    };

    let left = format!("{}{}", git_info, if app.code_panel.buffer.modified { " [+]" } else { "" });
    let right = format!(
        "Ln {}, Col {}  {}  UTF-8  Spaces: 4 ",
        buf.cursor_row + 1,
        buf.cursor_col + 1,
        lang,
    );

    let pad = (area.width as usize).saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 80))),
        Span::styled(" ".repeat(pad), Style::default().bg(Color::Rgb(30, 30, 80))),
        Span::styled(right, Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 80))),
    ]);

    f.render_widget(Paragraph::new(line), area);
}

// ─── Shared TextBuffer rendering ───

/// Plain text buffer rendering (used for prompt compose, etc.)
fn draw_text_buffer(
    f: &mut Frame,
    buf: &mut TextBuffer,
    area: Rect,
    focused: bool,
    show_line_numbers: bool,
) {
    draw_text_buffer_highlighted(f, buf, area, focused, show_line_numbers, "");
}

/// Syntax-highlighted text buffer with indent guides, whitespace dots, bracket matching
fn draw_text_buffer_highlighted(
    f: &mut Frame,
    buf: &mut TextBuffer,
    area: Rect,
    focused: bool,
    show_line_numbers: bool,
    ext: &str,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    let gutter_width: usize = if show_line_numbers { 5 } else { 0 };

    buf.adjust_scroll(height, width);

    let start = buf.scroll_row;
    let end = (start + height).min(buf.lines.len());

    // Get matching bracket position (if any)
    let matching_bracket = if focused { buf.matching_bracket() } else { None };

    // Get syntax-highlighted spans for the visible lines
    let visible_lines: Vec<String> = buf.lines[start..end].to_vec();
    let has_syntax = !ext.is_empty();
    let highlighted = if has_syntax {
        HIGHLIGHTER.highlight_lines(&visible_lines, ext)
    } else {
        Vec::new()
    };

    let scroll_col = buf.scroll_col;

    let lines: Vec<Line> = (start..end)
        .enumerate()
        .map(|(vi, i)| {
            let content = &buf.lines[i];
            let mut spans = Vec::new();

            // Line number gutter
            if show_line_numbers {
                let num_style = if i == buf.cursor_row && focused {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                spans.push(Span::styled(format!("{:>4} ", i + 1), num_style));
            }

            // Indent guides: vertical bars at each indent level
            let indent_spaces = content.chars().take_while(|c| *c == ' ').count();
            let indent_levels = indent_spaces / 4;

            // Build content with whitespace visualization
            if let Some((sel_start, sel_end)) = buf.selection_cols_for_row(i) {
                // Selection rendering (skip syntax highlighting for selected lines)
                let s = sel_start.min(content.len());
                let e = sel_end.min(content.len());
                let before = &content[..s];
                let selected = &content[s..e];
                let after = &content[e..];

                if !before.is_empty() {
                    spans.push(Span::styled(
                        render_whitespace(before, indent_levels, gutter_width),
                        Style::default().fg(Color::White),
                    ));
                }
                if !selected.is_empty() {
                    spans.push(Span::styled(
                        selected.to_string(),
                        Style::default().bg(Color::Rgb(40, 60, 100)).fg(Color::White),
                    ));
                }
                if !after.is_empty() {
                    spans.push(Span::styled(after.to_string(), Style::default().fg(Color::White)));
                }
            } else if has_syntax && vi < highlighted.len() {
                // Syntax highlighted rendering
                let syn_spans = &highlighted[vi];

                // Prepend indent guides
                if indent_levels > 0 {
                    let mut guide_str = String::new();
                    for level in 0..indent_levels {
                        let pos = level * 4;
                        if pos < indent_spaces {
                            guide_str.push('│');
                            // Whitespace dots for the remaining 3 spaces
                            for j in 1..4 {
                                if pos + j < indent_spaces {
                                    guide_str.push('·');
                                }
                            }
                        }
                    }
                    spans.push(Span::styled(guide_str, Style::default().fg(Color::Rgb(60, 60, 60))));

                    // Add syntax spans but skip the indent chars we already rendered
                    let mut chars_consumed = 0;
                    let indent_chars = indent_levels * 4;
                    for syn_span in syn_spans {
                        let text = syn_span.content.as_ref();
                        if chars_consumed + text.len() <= indent_chars {
                            chars_consumed += text.len();
                            continue;
                        }
                        if chars_consumed < indent_chars {
                            let skip = indent_chars - chars_consumed;
                            if skip < text.len() {
                                spans.push(Span::styled(text[skip..].to_string(), syn_span.style));
                            }
                            chars_consumed = indent_chars;
                        } else {
                            spans.push(syn_span.clone());
                        }
                    }
                } else {
                    spans.extend(syn_spans.iter().cloned());
                }
            } else {
                // Plain rendering with indent guides and whitespace dots
                spans.push(Span::styled(
                    render_whitespace(content, indent_levels, gutter_width),
                    Style::default().fg(Color::White),
                ));
            }

            // Apply horizontal scroll: trim scroll_col chars from content spans
            // (gutter span is always first if show_line_numbers)
            if scroll_col > 0 {
                let gutter_spans = if show_line_numbers { 1 } else { 0 };
                let mut chars_to_skip = scroll_col;
                let mut new_spans: Vec<Span> = spans[..gutter_spans].to_vec();

                for span in &spans[gutter_spans..] {
                    let text = span.content.as_ref();
                    if chars_to_skip >= text.len() {
                        chars_to_skip -= text.len();
                        continue;
                    }
                    if chars_to_skip > 0 {
                        new_spans.push(Span::styled(text[chars_to_skip..].to_string(), span.style));
                        chars_to_skip = 0;
                    } else {
                        new_spans.push(span.clone());
                    }
                }
                spans = new_spans;
            }

            Line::from(spans)
        })
        .collect();

    f.render_widget(Paragraph::new(lines), area);

    // Render bracket matching highlight
    if let Some((br, bc)) = matching_bracket {
        if br >= start && br < end {
            let y = area.y + (br - start) as u16;
            let x = area.x + gutter_width as u16 + bc.saturating_sub(buf.scroll_col) as u16;
            if x < area.x + area.width && y < area.y + area.height {
                // Highlight the matching bracket
                let _ch = buf.lines[br].as_bytes().get(bc).map(|b| *b as char).unwrap_or(' ');
                let buf_widget = f.buffer_mut();
                if let Some(cell) = buf_widget.cell_mut((x, y)) {
                    cell.set_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                }
            }
        }
    }

    if focused {
        let cursor_y = area.y + (buf.cursor_row.saturating_sub(buf.scroll_row)) as u16;
        let cursor_x =
            area.x + gutter_width as u16 + (buf.cursor_col.saturating_sub(buf.scroll_col)) as u16;
        if cursor_y < area.y + area.height && cursor_x < area.x + area.width {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

/// Replace leading spaces with indent guide chars and dots
fn render_whitespace(text: &str, indent_levels: usize, _gutter: usize) -> String {
    if indent_levels == 0 {
        return text.to_string();
    }
    let indent_chars = indent_levels * 4;
    let mut result = String::new();
    for level in 0..indent_levels {
        let pos = level * 4;
        if pos < text.len() {
            result.push('│');
            for j in 1..4 {
                if pos + j < text.len() && pos + j < indent_chars {
                    result.push('·');
                } else if pos + j < text.len() {
                    result.push(text.as_bytes()[pos + j] as char);
                }
            }
        }
    }
    if indent_chars < text.len() {
        result.push_str(&text[indent_chars..]);
    }
    result
}

// ─── Minimap ───

fn draw_minimap(f: &mut Frame, buf: &TextBuffer, area: Rect) {
    let height = area.height as usize;
    let total_lines = buf.lines.len();
    if total_lines == 0 || height == 0 {
        return;
    }

    // Each minimap row represents (total_lines / height) source lines
    let ratio = (total_lines as f64) / (height as f64);
    let viewport_start = buf.scroll_row;

    let mut lines = Vec::new();
    for y in 0..height {
        let source_line = (y as f64 * ratio) as usize;
        if source_line >= total_lines {
            lines.push(Line::from(Span::raw("")));
            continue;
        }

        let line = &buf.lines[source_line];
        // Compress the line into minimap width
        let mini_width = (area.width as usize).saturating_sub(1);
        let compressed: String = line
            .chars()
            .take(mini_width * 2) // sample 2x chars per minimap col
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, c)| if c.is_whitespace() { ' ' } else { '▪' })
            .take(mini_width)
            .collect();

        // Highlight viewport position
        let in_viewport = source_line >= viewport_start
            && source_line < viewport_start + buf.lines.len().min(height);

        let style = if in_viewport {
            Style::default()
                .fg(Color::Rgb(100, 100, 140))
                .bg(Color::Rgb(35, 35, 50))
        } else {
            Style::default().fg(Color::Rgb(60, 60, 60))
        };

        lines.push(Line::from(Span::styled(compressed, style)));
    }

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::Rgb(40, 40, 40)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}

// ─── LLM Panel ───

fn draw_llm_panel(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let panel = &app.llm_panel;
    let cost = panel.usage.total_cost();
    let status = if panel.streaming { "⟳" } else { "●" };
    let cost_str = if cost > 0.0 { format!(" ${:.4}", cost) } else { String::new() };
    let title = format!(
        " LLM {} {} in:{} out:{}{} ",
        status,
        panel.usage.model_name,
        format_tokens(panel.tokens_in),
        format_tokens(panel.tokens_out),
        cost_str,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Usage monitor overlay (Ctrl+U to toggle)
    if panel.show_usage {
        draw_usage_monitor(f, panel, inner);
        return;
    }

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

fn draw_usage_monitor(f: &mut Frame, panel: &crate::panels::llm::LlmPanel, area: Rect) {
    let usage = &panel.usage;
    let duration = usage.session_duration();
    let mins = duration.as_secs() / 60;
    let secs = duration.as_secs() % 60;
    let hours = mins / 60;
    let mins = mins % 60;

    let total_in = usage.total_tokens_in();
    let total_out = usage.total_tokens_out();
    let cost = usage.total_cost();
    let (rate_in, rate_out) = usage.recent_rate();

    let mut lines = Vec::new();

    // Header
    lines.push(Line::from(Span::styled(
        "╔══════════════════════════════════════╗",
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(Span::styled(
        "║       📊 Usage Monitor (Ctrl+U)      ║",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "╠══════════════════════════════════════╣",
        Style::default().fg(Color::Cyan),
    )));

    // Model
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Model: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<29}", usage.model_name),
            Style::default().fg(Color::White),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    // Session time
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Session: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<27}", format!("{}h {:02}m {:02}s", hours, mins, secs)),
            Style::default().fg(Color::White),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    // API calls
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("API Calls: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<25}", usage.api_calls),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    lines.push(Line::from(Span::styled(
        "╠══════════════════════════════════════╣",
        Style::default().fg(Color::Cyan),
    )));

    // Tokens
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Input tokens:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<21}", format_tokens_full(total_in)),
            Style::default().fg(Color::Green),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Output tokens: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<21}", format_tokens_full(total_out)),
            Style::default().fg(Color::Green),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Total tokens:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<21}", format_tokens_full(total_in + total_out)),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    lines.push(Line::from(Span::styled(
        "╠══════════════════════════════════════╣",
        Style::default().fg(Color::Cyan),
    )));

    // Cost
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Session cost:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<21}", format!("${:.4}", cost)),
            if cost > 1.0 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            },
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    // Cost per minute
    let cpm = usage.cost_per_minute();
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Cost/minute:   ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<21}", format!("${:.4}", cpm)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    // Budget
    if let Some(remaining) = usage.budget_remaining() {
        let pct = usage.budget_percent_used().unwrap_or(0.0);
        let bar_width = 20;
        let filled = (pct / 100.0 * bar_width as f64) as usize;
        let bar: String = format!(
            "{}{}",
            "█".repeat(filled.min(bar_width)),
            "░".repeat(bar_width.saturating_sub(filled)),
        );
        let color = if pct > 90.0 { Color::Red } else if pct > 70.0 { Color::Yellow } else { Color::Green };

        lines.push(Line::from(Span::styled(
            "╠══════════════════════════════════════╣",
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(vec![
            Span::styled("║ ", Style::default().fg(Color::Cyan)),
            Span::styled("Budget:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(bar, Style::default().fg(color)),
            Span::styled(
                format!(" {:.0}%", pct),
                Style::default().fg(color),
            ),
            Span::styled("   ║", Style::default().fg(Color::Cyan)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("║ ", Style::default().fg(Color::Cyan)),
            Span::styled("Remaining: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:<25}", format!("${:.4}", remaining)),
                Style::default().fg(if remaining < 0.0 { Color::Red } else { Color::Green }),
            ),
            Span::styled("║", Style::default().fg(Color::Cyan)),
        ]));
    }

    lines.push(Line::from(Span::styled(
        "╠══════════════════════════════════════╣",
        Style::default().fg(Color::Cyan),
    )));

    // Rate (last 5 min)
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Rate (5m): ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<25}", format!("↓{:.0}/m  ↑{:.0}/m", rate_in, rate_out)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    // Pricing
    lines.push(Line::from(vec![
        Span::styled("║ ", Style::default().fg(Color::Cyan)),
        Span::styled("Pricing:   ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<25}", format!("${}/Mi  ${}/Mo", usage.cost_per_m_input, usage.cost_per_m_output)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("║", Style::default().fg(Color::Cyan)),
    ]));

    lines.push(Line::from(Span::styled(
        "╚══════════════════════════════════════╝",
        Style::default().fg(Color::Cyan),
    )));

    f.render_widget(Paragraph::new(lines), area);
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

fn format_tokens_full(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{} ({:.2}M)", comma_sep(n), n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{} ({:.1}K)", comma_sep(n), n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

fn comma_sep(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
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
            draw_prompt_history(f, app, inner);
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

    // Naming overlay
    if let Some(ref input) = app.prompt_panel.naming_input {
        let title = format!(" New {} ", app.prompt_panel.naming_what);
        let width = 30u16.min(area.width);
        let height = 3u16;
        let popup = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + area.height / 2 - 1,
            width,
            height,
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title);
        f.render_widget(Clear, popup);
        f.render_widget(
            Paragraph::new(input.as_str()).block(block),
            popup,
        );
    }
}

// ─── Prompt History ───

fn draw_prompt_history(f: &mut Frame, app: &App, area: Rect) {
    let messages = &app.prompt_panel.history_messages;

    if messages.is_empty() {
        f.render_widget(
            Paragraph::new("No messages yet.\nPress Esc to go back.")
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let height = area.height as usize;
    let mut all_lines: Vec<Line> = Vec::new();

    for msg in messages {
        let (role_label, role_color) = match msg.role.as_str() {
            "user" => ("You", Color::Cyan),
            "assistant" => ("Assistant", Color::Green),
            _ => (&*msg.role, Color::DarkGray),
        };

        // Role header
        all_lines.push(Line::from(Span::styled(
            format!("── {} ──", role_label),
            Style::default().fg(role_color).add_modifier(Modifier::BOLD),
        )));

        // Message content (truncate long messages in history view)
        let content = if msg.content.len() > 2000 {
            format!("{}…", &msg.content[..2000])
        } else {
            msg.content.clone()
        };
        for line in content.lines() {
            all_lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::White),
            )));
        }
        all_lines.push(Line::from("")); // blank separator
    }

    let scroll = app.prompt_panel.history_scroll;
    // Scroll from the bottom (most recent messages)
    let total = all_lines.len();
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(height);

    let visible: Vec<Line> = all_lines[start..end].to_vec();

    f.render_widget(Paragraph::new(visible), area);
}

// ─── Terminal Panel ───

fn draw_terminal_panel(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(" Terminal (Ctrl+`) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let total = app.terminal_panel.lines.len();
    let scroll = app.terminal_panel.scroll_offset;

    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(height);

    let lines: Vec<Line> = app.terminal_panel.lines[start..end]
        .iter()
        .map(|l| {
            let style = if l.starts_with("$ ") {
                Style::default().fg(Color::Green)
            } else if l.starts_with("[stderr]") || l.starts_with("[error]") {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(l.as_str(), style))
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);

    // Input line at bottom
    if focused {
        let input_y = inner.y + inner.height.saturating_sub(1);
        let input_line = Line::from(vec![
            Span::styled("$ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(app.terminal_panel.input.as_str(), Style::default().fg(Color::White)),
        ]);
        let input_rect = Rect::new(inner.x, input_y, inner.width, 1);
        f.render_widget(Clear, input_rect);
        f.render_widget(Paragraph::new(input_line), input_rect);

        // Cursor
        let cx = inner.x + 2 + app.terminal_panel.input.len() as u16;
        if cx < inner.x + inner.width {
            f.set_cursor_position((cx, input_y));
        }
    }
}

// ─── Split Editor (right pane) ───

fn draw_split_editor(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.split_editor.as_ref().map_or(false, |s| s.focused);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(focused))
        .title(" Split ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(ref mut split) = app.split_editor {
        let ext = std::path::Path::new(&split.file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        draw_text_buffer_highlighted(f, &mut split.buffer, inner, focused, true, ext);
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
