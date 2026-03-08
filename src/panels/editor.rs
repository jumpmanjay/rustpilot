/// Shared text editing buffer with undo/redo, clipboard, word navigation, etc.
/// Used by both the Code editor and the Prompt composer.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::Mutex;

/// Internal clipboard shared across all TextBuffer instances.
/// Always used for in-app copy/paste. System clipboard is set as a bonus.
static INTERNAL_CLIPBOARD: Mutex<String> = Mutex::new(String::new());

fn clipboard_set(text: &str) {
    // Always set internal clipboard
    if let Ok(mut cb) = INTERNAL_CLIPBOARD.lock() {
        *cb = text.to_string();
    }
    // Also try system clipboard (best-effort)
    let _ = cli_clipboard::set_contents(text.to_string());
}

fn clipboard_get() -> String {
    // Always read from internal clipboard (what was copied in-app)
    INTERNAL_CLIPBOARD
        .lock()
        .map(|cb| cb.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
struct UndoEntry {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

pub struct TextBuffer {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_row: usize,
    pub scroll_col: usize,
    pub modified: bool,

    // Selection: anchor point (if Some, selection is anchor..cursor)
    pub select_anchor: Option<(usize, usize)>, // (row, col)

    // Undo/redo
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    #[allow(dead_code)]
    undo_batch: bool, // whether we're in a batch (e.g., typing chars)
    last_edit_kind: EditKind,

    /// Comment prefix for toggle-comment (set based on file extension)
    pub comment_prefix: String,

    /// Enable auto-close brackets/quotes
    pub auto_pair: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditKind {
    None,
    Insert,
    Delete,
    Other,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            scroll_row: 0,
            scroll_col: 0,
            modified: false,
            select_anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_batch: false,
            last_edit_kind: EditKind::None,
            comment_prefix: "// ".to_string(),
            auto_pair: true,
        }
    }

    pub fn from_string(content: &str) -> Self {
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        Self {
            lines: if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
            ..Self::new()
        }
    }

    pub fn to_string(&self) -> String {
        self.lines.join("\n")
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_row = 0;
        self.scroll_col = 0;
        self.modified = false;
        self.select_anchor = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    // ─── Undo / Redo ───

    fn save_undo(&mut self, kind: EditKind) {
        // Batch consecutive same-kind edits (typing, deleting)
        if kind == self.last_edit_kind && (kind == EditKind::Insert || kind == EditKind::Delete) {
            // Don't push a new undo entry for every keystroke
        } else {
            self.undo_stack.push(UndoEntry {
                lines: self.lines.clone(),
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
            });
            // Cap undo stack
            if self.undo_stack.len() > 200 {
                self.undo_stack.remove(0);
            }
        }
        self.last_edit_kind = kind;
        self.redo_stack.clear();
    }

    fn force_save_undo(&mut self) {
        self.undo_stack.push(UndoEntry {
            lines: self.lines.clone(),
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
        });
        if self.undo_stack.len() > 200 {
            self.undo_stack.remove(0);
        }
        self.last_edit_kind = EditKind::Other;
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        if let Some(entry) = self.undo_stack.pop() {
            self.redo_stack.push(UndoEntry {
                lines: self.lines.clone(),
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
            });
            self.lines = entry.lines;
            self.cursor_row = entry.cursor_row;
            self.cursor_col = entry.cursor_col;
            self.modified = true;
            self.last_edit_kind = EditKind::None;
        }
    }

    pub fn redo(&mut self) {
        if let Some(entry) = self.redo_stack.pop() {
            self.undo_stack.push(UndoEntry {
                lines: self.lines.clone(),
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
            });
            self.lines = entry.lines;
            self.cursor_row = entry.cursor_row;
            self.cursor_col = entry.cursor_col;
            self.modified = true;
            self.last_edit_kind = EditKind::None;
        }
    }

    // ─── Selection ───

    /// Returns (start_row, start_col, end_row, end_col) normalized so start <= end
    pub fn selection_range(&self) -> Option<(usize, usize, usize, usize)> {
        let (ar, ac) = self.select_anchor?;
        let (cr, cc) = (self.cursor_row, self.cursor_col);
        if ar < cr || (ar == cr && ac <= cc) {
            Some((ar, ac, cr, cc))
        } else {
            Some((cr, cc, ar, ac))
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let (sr, sc, er, ec) = self.selection_range()?;
        if sr == er {
            Some(self.lines[sr][sc..ec].to_string())
        } else {
            let mut result = String::new();
            result.push_str(&self.lines[sr][sc..]);
            result.push('\n');
            for r in (sr + 1)..er {
                result.push_str(&self.lines[r]);
                result.push('\n');
            }
            result.push_str(&self.lines[er][..ec]);
            Some(result)
        }
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((sr, sc, er, ec)) = self.selection_range() {
            self.force_save_undo();
            if sr == er {
                self.lines[sr].drain(sc..ec);
            } else {
                let rest = self.lines[er][ec..].to_string();
                self.lines[sr].truncate(sc);
                self.lines[sr].push_str(&rest);
                self.lines.drain((sr + 1)..=er);
            }
            self.cursor_row = sr;
            self.cursor_col = sc;
            self.select_anchor = None;
            self.modified = true;
            true
        } else {
            false
        }
    }

    fn start_or_extend_selection(&mut self) {
        if self.select_anchor.is_none() {
            self.select_anchor = Some((self.cursor_row, self.cursor_col));
        }
    }

    // ─── Word navigation helpers ───

    fn word_boundary_left(&self) -> usize {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col == 0 {
            return 0;
        }
        let bytes = line.as_bytes();
        let mut pos = self.cursor_col - 1;
        // Skip whitespace
        while pos > 0 && bytes[pos].is_ascii_whitespace() {
            pos -= 1;
        }
        // Skip word chars
        while pos > 0 && !bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        pos
    }

    fn word_boundary_right(&self) -> usize {
        let line = &self.lines[self.cursor_row];
        let len = line.len();
        if self.cursor_col >= len {
            return len;
        }
        let bytes = line.as_bytes();
        let mut pos = self.cursor_col;
        // Skip current word
        while pos < len && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        // Skip whitespace
        while pos < len && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        pos
    }

    // ─── Clipboard ───

    pub fn copy(&mut self) {
        let text = if let Some(text) = self.selected_text() {
            text
        } else {
            // No selection: copy current line
            self.lines[self.cursor_row].clone() + "\n"
        };
        clipboard_set(&text);
    }

    pub fn cut(&mut self) {
        self.copy();
        if self.select_anchor.is_some() {
            self.delete_selection();
        } else {
            // No selection: cut current line
            self.force_save_undo();
            if self.lines.len() > 1 {
                self.lines.remove(self.cursor_row);
                if self.cursor_row >= self.lines.len() {
                    self.cursor_row = self.lines.len() - 1;
                }
                self.clamp_col();
            } else {
                self.lines[0].clear();
                self.cursor_col = 0;
            }
            self.modified = true;
        }
    }

    pub fn paste(&mut self) {
        let clip_text = clipboard_get();
        if clip_text.is_empty() {
            return;
        }
        self.delete_selection();
        self.force_save_undo();

        let clip: Vec<&str> = clip_text.split('\n').collect();
        if clip.len() == 1 {
            self.lines[self.cursor_row].insert_str(self.cursor_col, clip[0]);
            self.cursor_col += clip[0].len();
        } else {
            let rest = self.lines[self.cursor_row].split_off(self.cursor_col);
            self.lines[self.cursor_row].push_str(clip[0]);
            for (i, line) in clip[1..].iter().enumerate() {
                self.cursor_row += 1;
                if i == clip.len() - 2 {
                    let mut last = line.to_string();
                    self.cursor_col = last.len();
                    last.push_str(&rest);
                    self.lines.insert(self.cursor_row, last);
                } else {
                    self.lines.insert(self.cursor_row, line.to_string());
                }
            }
        }
        self.modified = true;
    }

    // ─── Core editing ───

    pub fn insert_char(&mut self, c: char) {
        self.delete_selection();
        self.save_undo(EditKind::Insert);
        self.lines[self.cursor_row].insert(self.cursor_col, c);
        self.cursor_col += 1;
        self.modified = true;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        self.force_save_undo();
        self.lines[self.cursor_row].insert_str(self.cursor_col, s);
        self.cursor_col += s.len();
        self.modified = true;
    }

    pub fn insert_newline(&mut self) {
        self.delete_selection();
        self.force_save_undo();
        // Auto-indent: copy leading whitespace from current line
        let indent: String = self.lines[self.cursor_row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let rest = self.lines[self.cursor_row].split_off(self.cursor_col);
        self.cursor_row += 1;
        let new_line = format!("{}{}", indent, rest);
        self.cursor_col = indent.len();
        self.lines.insert(self.cursor_row, new_line);
        self.modified = true;
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.save_undo(EditKind::Delete);
        if self.cursor_col > 0 {
            self.lines[self.cursor_row].remove(self.cursor_col - 1);
            self.cursor_col -= 1;
            self.modified = true;
        } else if self.cursor_row > 0 {
            let line = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&line);
            self.modified = true;
        }
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.save_undo(EditKind::Delete);
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            self.lines[self.cursor_row].remove(self.cursor_col);
            self.modified = true;
        } else if self.cursor_row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
            self.modified = true;
        }
    }

    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.force_save_undo();
        let target = self.word_boundary_left();
        if target < self.cursor_col {
            self.lines[self.cursor_row].drain(target..self.cursor_col);
            self.cursor_col = target;
            self.modified = true;
        }
    }

    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.force_save_undo();
        let target = self.word_boundary_right();
        if target > self.cursor_col {
            self.lines[self.cursor_row].drain(self.cursor_col..target);
            self.modified = true;
        }
    }

    pub fn delete_line(&mut self) {
        self.force_save_undo();
        self.select_anchor = None;
        if self.lines.len() > 1 {
            self.lines.remove(self.cursor_row);
            if self.cursor_row >= self.lines.len() {
                self.cursor_row = self.lines.len() - 1;
            }
            self.clamp_col();
        } else {
            self.lines[0].clear();
            self.cursor_col = 0;
        }
        self.modified = true;
    }

    pub fn duplicate_line(&mut self) {
        self.force_save_undo();
        let line = self.lines[self.cursor_row].clone();
        self.lines.insert(self.cursor_row + 1, line);
        self.cursor_row += 1;
        self.modified = true;
    }

    // ─── Indentation ───

    pub fn indent(&mut self) {
        if let Some((sr, _, er, _)) = self.selection_range() {
            self.force_save_undo();
            for r in sr..=er {
                self.lines[r].insert_str(0, "    ");
            }
            self.cursor_col += 4;
            if let Some((ar, ac)) = self.select_anchor {
                self.select_anchor = Some((ar, ac + 4));
            }
            self.modified = true;
        } else {
            self.force_save_undo();
            self.lines[self.cursor_row].insert_str(0, "    ");
            self.cursor_col += 4;
            self.modified = true;
        }
    }

    pub fn dedent(&mut self) {
        if let Some((sr, _, er, _)) = self.selection_range() {
            self.force_save_undo();
            for r in sr..=er {
                let spaces = self.lines[r]
                    .chars()
                    .take(4)
                    .take_while(|c| *c == ' ')
                    .count();
                if spaces > 0 {
                    self.lines[r].drain(..spaces);
                }
            }
            // Adjust cursors
            let spaces = 4.min(self.cursor_col);
            self.cursor_col = self.cursor_col.saturating_sub(spaces);
            if let Some((ar, ac)) = self.select_anchor {
                self.select_anchor = Some((ar, ac.saturating_sub(4)));
            }
            self.modified = true;
        } else {
            let spaces = self.lines[self.cursor_row]
                .chars()
                .take(4)
                .take_while(|c| *c == ' ')
                .count();
            if spaces > 0 {
                self.force_save_undo();
                self.lines[self.cursor_row].drain(..spaces);
                self.cursor_col = self.cursor_col.saturating_sub(spaces);
                self.modified = true;
            }
        }
    }

    // ─── Navigation ───

    pub fn move_up(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.clamp_col();
        }
    }

    pub fn move_down(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.clamp_col();
        }
    }

    pub fn move_left(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
        }
    }

    pub fn move_right(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        if self.cursor_col < self.lines[self.cursor_row].len() {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    pub fn move_word_left(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        if self.cursor_col == 0 && self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
        } else {
            self.cursor_col = self.word_boundary_left();
        }
    }

    pub fn move_word_right(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        let len = self.lines[self.cursor_row].len();
        if self.cursor_col >= len && self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        } else {
            self.cursor_col = self.word_boundary_right();
        }
    }

    pub fn move_home(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        // Smart home: first press goes to first non-whitespace, second to column 0
        let first_non_ws = self.lines[self.cursor_row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        if self.cursor_col == first_non_ws || first_non_ws == self.lines[self.cursor_row].len() {
            self.cursor_col = 0;
        } else {
            self.cursor_col = first_non_ws;
        }
    }

    pub fn move_end(&mut self, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    pub fn page_up(&mut self, page_size: usize, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        self.cursor_row = self.cursor_row.saturating_sub(page_size);
        self.clamp_col();
    }

    pub fn page_down(&mut self, page_size: usize, shift: bool) {
        if shift {
            self.start_or_extend_selection();
        } else {
            self.select_anchor = None;
        }
        self.cursor_row = (self.cursor_row + page_size).min(self.lines.len() - 1);
        self.clamp_col();
    }

    pub fn select_all(&mut self) {
        self.select_anchor = Some((0, 0));
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    // ─── Scroll management ───

    pub fn adjust_scroll(&mut self, viewport_height: usize, viewport_width: usize) {
        if viewport_height == 0 {
            return;
        }
        // Vertical
        if self.cursor_row < self.scroll_row {
            self.scroll_row = self.cursor_row;
        }
        if self.cursor_row >= self.scroll_row + viewport_height {
            self.scroll_row = self.cursor_row - viewport_height + 1;
        }
        // Horizontal (account for gutter)
        if self.cursor_col < self.scroll_col {
            self.scroll_col = self.cursor_col;
        }
        if viewport_width > 4 && self.cursor_col >= self.scroll_col + viewport_width - 4 {
            self.scroll_col = self.cursor_col - viewport_width + 5;
        }
    }

    fn clamp_col(&mut self) {
        let len = self.lines[self.cursor_row].len();
        if self.cursor_col > len {
            self.cursor_col = len;
        }
    }

    // ─── Line operations ───

    pub fn move_line_up(&mut self) {
        if self.cursor_row == 0 {
            return;
        }
        self.force_save_undo();
        self.lines.swap(self.cursor_row, self.cursor_row - 1);
        self.cursor_row -= 1;
        self.modified = true;
    }

    pub fn move_line_down(&mut self) {
        if self.cursor_row + 1 >= self.lines.len() {
            return;
        }
        self.force_save_undo();
        self.lines.swap(self.cursor_row, self.cursor_row + 1);
        self.cursor_row += 1;
        self.modified = true;
    }

    pub fn toggle_comment(&mut self) {
        let prefix = &self.comment_prefix.clone();
        if let Some((sr, _, er, _)) = self.selection_range() {
            self.force_save_undo();
            // Check if all lines are commented
            let all_commented = (sr..=er).all(|r| self.lines[r].trim_start().starts_with(prefix.trim()));
            for r in sr..=er {
                if all_commented {
                    // Remove comment
                    if let Some(pos) = self.lines[r].find(prefix) {
                        self.lines[r].drain(pos..pos + prefix.len());
                    }
                } else {
                    // Add comment at the indentation level
                    let indent: usize = self.lines[r].chars().take_while(|c| c.is_whitespace()).count();
                    self.lines[r].insert_str(indent, prefix);
                }
            }
        } else {
            self.force_save_undo();
            let line = &self.lines[self.cursor_row];
            let indent: usize = line.chars().take_while(|c| c.is_whitespace()).count();
            if line.trim_start().starts_with(prefix.trim()) {
                if let Some(pos) = self.lines[self.cursor_row].find(prefix) {
                    self.lines[self.cursor_row].drain(pos..pos + prefix.len());
                    self.cursor_col = self.cursor_col.saturating_sub(prefix.len());
                }
            } else {
                self.lines[self.cursor_row].insert_str(indent, prefix);
                self.cursor_col += prefix.len();
            }
        }
        self.modified = true;
    }

    pub fn select_next_occurrence(&mut self) {
        if self.select_anchor.is_none() {
            // No selection — select word under cursor
            let line = &self.lines[self.cursor_row];
            let mut start = self.cursor_col;
            let mut end = self.cursor_col;
            let bytes = line.as_bytes();
            // Expand left
            while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
                start -= 1;
            }
            // Expand right
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if start < end {
                self.select_anchor = Some((self.cursor_row, start));
                self.cursor_col = end;
            }
        } else {
            // Has selection — find next occurrence
            if let Some(text) = self.selected_text() {
                let start_row = self.cursor_row;
                let start_col = self.cursor_col;
                // Search forward from cursor
                for r in start_row..self.lines.len() {
                    let search_from = if r == start_row { start_col } else { 0 };
                    if let Some(pos) = self.lines[r][search_from..].find(&text) {
                        let col = search_from + pos;
                        self.select_anchor = Some((r, col));
                        self.cursor_row = r;
                        self.cursor_col = col + text.len();
                        return;
                    }
                }
                // Wrap around from beginning
                for r in 0..=start_row {
                    let end_at = if r == start_row { start_col.saturating_sub(text.len()) } else { self.lines[r].len() };
                    if let Some(pos) = self.lines[r][..end_at].find(&text) {
                        self.select_anchor = Some((r, pos));
                        self.cursor_row = r;
                        self.cursor_col = pos + text.len();
                        return;
                    }
                }
            }
        }
    }

    pub fn insert_char_with_autopair(&mut self, c: char) {
        if !self.auto_pair {
            self.insert_char(c);
            return;
        }

        let closing = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            '`' => Some('`'),
            _ => None,
        };

        // Skip-over closing brackets
        if matches!(c, ')' | ']' | '}' | '"' | '\'' | '`') {
            let line = &self.lines[self.cursor_row];
            if self.cursor_col < line.len() {
                let next = line.as_bytes()[self.cursor_col] as char;
                if next == c {
                    // Just move past it
                    self.cursor_col += 1;
                    return;
                }
            }
        }

        if let Some(closer) = closing {
            // For quotes, only auto-pair if they're openers (not inside a word)
            if matches!(c, '"' | '\'' | '`') {
                let line = &self.lines[self.cursor_row];
                if self.cursor_col > 0 {
                    let prev = line.as_bytes()[self.cursor_col - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' {
                        // Likely closing a string, just insert
                        self.insert_char(c);
                        return;
                    }
                }
            }
            self.delete_selection();
            self.save_undo(EditKind::Insert);
            self.lines[self.cursor_row].insert(self.cursor_col, closer);
            self.lines[self.cursor_row].insert(self.cursor_col, c);
            self.cursor_col += 1;
            self.modified = true;
        } else {
            self.insert_char(c);
        }
    }

    /// Find the matching bracket for the char at/before cursor
    pub fn matching_bracket(&self) -> Option<(usize, usize)> {
        let line = &self.lines[self.cursor_row];

        // Check char at cursor and before cursor
        let positions = [self.cursor_col, self.cursor_col.wrapping_sub(1)];
        for &pos in &positions {
            if pos >= line.len() {
                continue;
            }
            let ch = line.as_bytes()[pos] as char;
            let (target, forward) = match ch {
                '(' => (')', true),
                '[' => (']', true),
                '{' => ('}', true),
                ')' => ('(', false),
                ']' => ('[', false),
                '}' => ('{', false),
                _ => continue,
            };
            if let Some(result) = self.find_matching(self.cursor_row, pos, ch, target, forward) {
                return Some(result);
            }
        }
        None
    }

    fn find_matching(&self, row: usize, col: usize, open: char, close: char, forward: bool) -> Option<(usize, usize)> {
        let mut depth = 0i32;
        if forward {
            let mut r = row;
            let mut c = col;
            while r < self.lines.len() {
                let line = &self.lines[r];
                while c < line.len() {
                    let ch = line.as_bytes()[c] as char;
                    if ch == open { depth += 1; }
                    if ch == close { depth -= 1; }
                    if depth == 0 { return Some((r, c)); }
                    c += 1;
                }
                r += 1;
                c = 0;
            }
        } else {
            let mut r = row as isize;
            let mut c = col as isize;
            while r >= 0 {
                let line = &self.lines[r as usize];
                if c < 0 { c = line.len() as isize - 1; }
                while c >= 0 {
                    let ch = line.as_bytes()[c as usize] as char;
                    if ch == close { depth += 1; }
                    if ch == open { depth -= 1; }
                    if depth == 0 { return Some((r as usize, c as usize)); }
                    c -= 1;
                }
                r -= 1;
                c = -1;
            }
        }
        None
    }

    pub fn go_to_line(&mut self, line: usize) {
        self.select_anchor = None;
        self.cursor_row = line.saturating_sub(1).min(self.lines.len() - 1);
        self.cursor_col = 0;
    }

    pub fn insert_line_above(&mut self) {
        self.force_save_undo();
        let indent: String = self.lines[self.cursor_row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        self.lines.insert(self.cursor_row, indent.clone());
        self.cursor_col = indent.len();
        self.modified = true;
    }

    /// Set comment prefix based on file extension
    pub fn set_comment_for_ext(&mut self, ext: &str) {
        self.comment_prefix = match ext {
            "rs" | "ts" | "tsx" | "js" | "jsx" | "go" | "c" | "cpp" | "h" | "java"
            | "cs" | "swift" | "kt" | "scala" | "dart" | "zig" => "// ",
            "py" | "sh" | "bash" | "zsh" | "rb" | "pl" | "yaml" | "yml" | "toml"
            | "r" | "jl" | "nim" | "elixir" | "ex" | "exs" => "# ",
            "sql" | "lua" | "hs" | "ada" => "-- ",
            "html" | "xml" | "svg" => "<!-- ",
            "css" | "scss" | "less" => "/* ",
            "vim" => "\" ",
            "lisp" | "clj" | "cljs" | "el" => ";; ",
            "bat" | "cmd" => "REM ",
            _ => "// ",
        }
        .to_string();
    }

    // ─── Dispatch key events ───

    /// Handle a key event. Returns true if the key was consumed.
    pub fn handle_key(&mut self, key: KeyEvent, page_size: usize) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            // Line movement (Alt+Up/Down)
            KeyCode::Up if alt => { self.move_line_up(); true }
            KeyCode::Down if alt => { self.move_line_down(); true }

            // Navigation
            KeyCode::Up => { self.move_up(shift); true }
            KeyCode::Down => { self.move_down(shift); true }
            KeyCode::Left if ctrl => { self.move_word_left(shift); true }
            KeyCode::Right if ctrl => { self.move_word_right(shift); true }
            KeyCode::Left => { self.move_left(shift); true }
            KeyCode::Right => { self.move_right(shift); true }
            KeyCode::Home => { self.move_home(shift); true }
            KeyCode::End => { self.move_end(shift); true }
            KeyCode::PageUp => { self.page_up(page_size, shift); true }
            KeyCode::PageDown => { self.page_down(page_size, shift); true }

            // Editing with Ctrl
            KeyCode::Char('z') if ctrl => { self.undo(); true }
            KeyCode::Char('y') if ctrl => { self.redo(); true }
            KeyCode::Char('c') if ctrl => { self.copy(); true }
            KeyCode::Char('x') if ctrl => { self.cut(); true }
            KeyCode::Char('v') if ctrl => { self.paste(); true }
            KeyCode::Char('a') if ctrl => { self.select_all(); true }
            KeyCode::Char('d') if ctrl => { self.select_next_occurrence(); true }
            KeyCode::Char('D') if ctrl => { self.duplicate_line(); true }
            KeyCode::Char('K') if ctrl && shift => { self.delete_line(); true }
            KeyCode::Char('K') if ctrl => { self.delete_line(); true }
            KeyCode::Char('/') if ctrl => { self.toggle_comment(); true }
            KeyCode::Backspace if ctrl => { self.delete_word_back(); true }
            KeyCode::Delete if ctrl => { self.delete_word_forward(); true }

            // Insert line above (Ctrl+Shift+Enter)
            KeyCode::Enter if ctrl && shift => { self.insert_line_above(); true }
            KeyCode::Enter if ctrl => { self.insert_line_above(); true }

            // Tab / Shift+Tab
            KeyCode::Tab if shift => { self.dedent(); true }
            KeyCode::BackTab => { self.dedent(); true }
            KeyCode::Tab => { self.indent(); true }

            // Basic editing (with auto-pair)
            KeyCode::Char(c) if !ctrl && !alt => { self.insert_char_with_autopair(c); true }
            KeyCode::Backspace => { self.backspace(); true }
            KeyCode::Delete => { self.delete(); true }
            KeyCode::Enter => { self.insert_newline(); true }

            _ => false,
        }
    }

    /// Check if a row is within the current selection
    #[allow(dead_code)]
    pub fn is_row_selected(&self, row: usize) -> bool {
        if let Some((sr, _, er, _)) = self.selection_range() {
            row >= sr && row <= er
        } else {
            false
        }
    }

    /// Get the selected column range for a given row (for highlighting)
    pub fn selection_cols_for_row(&self, row: usize) -> Option<(usize, usize)> {
        let (sr, sc, er, ec) = self.selection_range()?;
        if row < sr || row > er {
            return None;
        }
        let line_len = self.lines[row].len();
        let start = if row == sr { sc } else { 0 };
        let end = if row == er { ec } else { line_len };
        Some((start, end))
    }
}
