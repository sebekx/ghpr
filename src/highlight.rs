use ratatui::style::Color;
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub struct Highlighter {
    ps: SyntaxSet,
    ts: ThemeSet,
}

/// Pre-highlighted file: maps diff line index -> colored spans (only for code lines)
#[derive(Debug, Clone)]
pub struct HighlightedFile {
    lines: HashMap<usize, Vec<(Color, String)>>,
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            ps: SyntaxSet::load_defaults_newlines(),
            ts: ThemeSet::load_defaults(),
        }
    }

    /// Highlight code lines of a file.
    /// `path` determines syntax. `code_lines` is (diff_line_index, stripped_content) pairs
    /// for only Added/Removed/Context lines.
    pub fn highlight_file(&self, path: &str, code_lines: &[(usize, &str)]) -> HighlightedFile {
        let syntax = self.ps.find_syntax_for_file(path)
            .ok()
            .flatten()
            .or_else(|| {
                let ext = path.rsplit('.').next().unwrap_or("");
                match ext {
                    "ts" | "tsx" => self.ps.find_syntax_by_extension("js"),
                    "jsx" => self.ps.find_syntax_by_extension("js"),
                    "mjs" | "cjs" => self.ps.find_syntax_by_extension("js"),
                    "yml" => self.ps.find_syntax_by_extension("yaml"),
                    "md" => self.ps.find_syntax_by_extension("markdown"),
                    _ => self.ps.find_syntax_by_extension(ext),
                }
            })
            .unwrap_or_else(|| self.ps.find_syntax_plain_text());

        let theme = &self.ts.themes["base16-ocean.dark"];
        let mut h = HighlightLines::new(syntax, theme);

        // Build text from only code lines (no meta/hunk garbage)
        let full_text: String = code_lines.iter().map(|(_, l)| format!("{}\n", l)).collect();
        let mut result = HashMap::new();
        let mut idx = 0;

        for line in LinesWithEndings::from(&full_text) {
            if idx >= code_lines.len() { break; }
            let diff_li = code_lines[idx].0;
            if let Ok(ranges) = h.highlight_line(line, &self.ps) {
                let spans: Vec<(Color, String)> = ranges
                    .iter()
                    .map(|(style, text)| {
                        let color = syntect_fg_to_ratatui(style);
                        (color, text.trim_end_matches('\n').to_string())
                    })
                    .filter(|(_, text)| !text.is_empty())
                    .collect();
                result.insert(diff_li, spans);
            }
            idx += 1;
        }

        HighlightedFile { lines: result }
    }
}

impl HighlightedFile {
    pub fn get_spans(&self, line_idx: usize) -> Option<&Vec<(Color, String)>> {
        self.lines.get(&line_idx)
    }
}

fn syntect_fg_to_ratatui(style: &Style) -> Color {
    let fg = style.foreground;
    // Ensure minimum brightness so colors are visible on dark diff backgrounds
    let r = fg.r.max(60);
    let g = fg.g.max(60);
    let b = fg.b.max(60);
    Color::Rgb(r, g, b)
}
