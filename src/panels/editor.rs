/// Shared text editing buffer with undo/redo, clipboard, word navigation, etc.
/// Used by both the Code editor and the Prompt composer.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

    // Internal clipboard
    clipboard: Vec<String>,

    // Undo/redo
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    undo_batch: bool, // whether we're in a batch (e.g., typing chars)
    last_edit_kind: EditKind,
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
            clipboard: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_batch: false,
            last_edit_kind: EditKind::None,
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
        if let Some(text) = self.selected_text() {
            self.clipboard = text.lines().map(|l| l.to_string()).collect();
            if self.clipboard.is_empty() {
                self.clipboard.push(String::new());
            }
        } else {
            // No selection: copy current line
            self.clipboard = vec![self.lines[self.cursor_row].clone()];
        }
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
        if self.clipboard.is_empty() {
            return;
        }
        self.delete_selection(); // remove selected text first
        self.force_save_undo();

        let clip = self.clipboard.clone();
        if clip.len() == 1 {
            self.lines[self.cursor_row].insert_str(self.cursor_col, &clip[0]);
            self.cursor_col += clip[0].len();
        } else {
            let rest = self.lines[self.cursor_row].split_off(self.cursor_col);
            self.lines[self.cursor_row].push_str(&clip[0]);
            for (i, line) in clip[1..].iter().enumerate() {
                self.cursor_row += 1;
                if i == clip.len() - 2 {
                    // Last pasted line — append rest
                    let mut last = line.clone();
                    self.cursor_col = last.len();
                    last.push_str(&rest);
                    self.lines.insert(self.cursor_row, last);
                } else {
                    self.lines.insert(self.cursor_row, line.clone());
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

    // ─── Dispatch key events ───

    /// Handle a key event. Returns true if the key was consumed.
    pub fn handle_key(&mut self, key: KeyEvent, page_size: usize) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
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
            KeyCode::Char('d') if ctrl => { self.delete_line(); true }
            KeyCode::Char('D') if ctrl => { self.duplicate_line(); true }
            KeyCode::Backspace if ctrl => { self.delete_word_back(); true }
            KeyCode::Delete if ctrl => { self.delete_word_forward(); true }

            // Tab / Shift+Tab
            KeyCode::Tab if shift => { self.dedent(); true }
            KeyCode::BackTab => { self.dedent(); true }
            KeyCode::Tab => { self.indent(); true }

            // Basic editing
            KeyCode::Char(c) if !ctrl => { self.insert_char(c); true }
            KeyCode::Backspace => { self.backspace(); true }
            KeyCode::Delete => { self.delete(); true }
            KeyCode::Enter => { self.insert_newline(); true }

            _ => false,
        }
    }

    /// Check if a row is within the current selection
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
