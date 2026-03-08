/// Syntax highlighting using syntect.
/// Provides a cached highlighter that maps lines → styled spans for ratatui.
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use syntect::highlighting::{ThemeSet, Style as SynStyle};
use syntect::parsing::SyntaxSet;
use syntect::easy::HighlightLines;

pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme_name: String,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            theme_name: "Monokai Extended".to_string(),
        }
    }

    /// Highlight a slice of lines for a given file extension.
    /// Returns Vec of Vec<Span> — one inner vec per line.
    pub fn highlight_lines<'a>(
        &self,
        lines: &'a [String],
        extension: &str,
    ) -> Vec<Vec<Span<'a>>> {
        let syntax = self
            .syntax_set
            .find_syntax_by_extension(extension)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = &self.theme_set.themes[&self.theme_name];
        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut result = Vec::with_capacity(lines.len());

        for line in lines {
            let line_with_nl = format!("{}\n", line);
            let spans = match highlighter.highlight_line(&line_with_nl, &self.syntax_set) {
                Ok(ranges) => ranges
                    .into_iter()
                    .map(|(style, text)| {
                        let text = text.trim_end_matches('\n');
                        if text.is_empty() {
                            return Span::raw("");
                        }
                        Span::styled(text.to_string(), syn_to_ratatui(style))
                    })
                    .collect(),
                Err(_) => vec![Span::raw(line.as_str().to_string())],
            };
            result.push(spans);
        }

        result
    }
}

fn syn_to_ratatui(style: SynStyle) -> Style {
    let fg = style.foreground;
    // Use direct RGB — works in truecolor terminals.
    // For xterm-256 fallback, we could map to nearest 256-color,
    // but most modern terminals support truecolor.
    Style::default().fg(Color::Rgb(
        saturate(fg.r),
        saturate(fg.g),
        saturate(fg.b),
    ))
}

/// Boost muted colors to be more vibrant on dark backgrounds
fn saturate(c: u8) -> u8 {
    // Lift dark colors slightly so they're visible, and boost mid-range
    if c < 60 {
        c.saturating_add(30)
    } else if c < 180 {
        c.saturating_add(15)
    } else {
        c
    }
}
