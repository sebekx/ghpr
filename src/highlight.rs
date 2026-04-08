use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub struct Highlighter {
    ps: SyntaxSet,
    ts: ThemeSet,
}

/// Pre-highlighted file: each line index maps to colored spans
#[derive(Debug, Clone)]
pub struct HighlightedFile {
    lines: Vec<Vec<(Color, String)>>,
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            ps: SyntaxSet::load_defaults_newlines(),
            ts: ThemeSet::load_defaults(),
        }
    }

    /// Highlight all lines of a file at once.
    /// `path` is used to determine syntax; `code_lines` are the raw code lines (without +/- prefix).
    pub fn highlight_file(&self, path: &str, code_lines: &[&str]) -> HighlightedFile {
        // Try file path first, then extension-based lookup
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

        let full_text: String = code_lines.iter().map(|l| format!("{}\n", l)).collect();
        let mut result = Vec::with_capacity(code_lines.len());

        for line in LinesWithEndings::from(&full_text) {
            if let Ok(ranges) = h.highlight_line(line, &self.ps) {
                let spans: Vec<(Color, String)> = ranges
                    .iter()
                    .map(|(style, text)| {
                        let color = syntect_fg_to_ratatui(style);
                        (color, text.trim_end_matches('\n').to_string())
                    })
                    .filter(|(_, text)| !text.is_empty())
                    .collect();
                result.push(spans);
            } else {
                result.push(Vec::new());
            }
        }

        HighlightedFile { lines: result }
    }
}

impl HighlightedFile {
    pub fn get_spans(&self, line_idx: usize) -> Option<&Vec<(Color, String)>> {
        self.lines.get(line_idx)
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
