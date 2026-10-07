//! Turn the HTML that shows up in GitHub comment bodies (`<details>`,
//! `<br>`, tables from bots, `<!-- hidden -->` markers, entities) into plain
//! text a terminal can show. Markdown is left as it is, and nothing inside
//! backticks or code fences is touched, so `Vec<String>` survives.

use std::borrow::Cow;

/// Tags we understand. Anything else that merely looks like a tag — a Rust
/// or TypeScript generic, say — is left untouched.
const KNOWN_TAGS: &[&str] = &[
    "a", "abbr", "b", "blockquote", "br", "code", "dd", "del", "details", "div", "dl", "dt",
    "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins", "kbd", "li", "mark",
    "ol", "p", "picture", "pre", "s", "samp", "source", "span", "strike", "strong", "sub",
    "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "tt", "u", "ul",
];

/// Display form of a comment body. Borrowed when there is nothing to convert,
/// which is the common case.
pub fn to_text(body: &str) -> Cow<'_, str> {
    if !body.contains('<') && !body.contains('&') {
        return Cow::Borrowed(body);
    }

    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    // Inside an HTML comment that started on an earlier line.
    let mut in_comment = false;
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut line = line;
        if in_comment {
            match line.find("-->") {
                Some(end) => {
                    in_comment = false;
                    line = &line[end + 3..];
                }
                None => continue,
            }
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
        } else if in_fence {
            out.push_str(line);
        } else {
            in_comment = convert_line(line, &mut out);
        }
    }

    Cow::Owned(tidy(&out))
}

/// Returns true when the line ends inside an unterminated `<!--`.
fn convert_line(line: &str, out: &mut String) -> bool {
    let mut rest = line;
    while !rest.is_empty() {
        // Inline code: copy through verbatim.
        if rest.starts_with('`') {
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                let span_end = ticks + end + ticks;
                out.push_str(&rest[..span_end]);
                rest = &rest[span_end..];
                continue;
            }
            out.push_str(fence);
            rest = &rest[ticks..];
            continue;
        }

        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => rest = &rest[end + 3..],
                None => return true,
            }
            continue;
        }

        if rest.starts_with('<') {
            if let Some((tag, consumed)) = parse_tag(rest) {
                emit_tag(&tag, out);
                rest = &rest[consumed..];
                continue;
            }
        }

        if rest.starts_with('&') {
            if let Some((decoded, consumed)) = decode_entity(rest) {
                out.push(decoded);
                rest = &rest[consumed..];
                continue;
            }
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    false
}

struct Tag {
    name: String,
    closing: bool,
    attrs: String,
}

fn parse_tag(s: &str) -> Option<(Tag, usize)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    let inner = inner.trim_end_matches('/');
    let name_len = inner
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(inner.len());
    let name = inner[..name_len].to_ascii_lowercase();
    if !KNOWN_TAGS.contains(&name.as_str()) {
        return None;
    }
    // `<b` followed by something that isn't whitespace isn't a tag (`<bar>`).
    let after = &inner[name_len..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    Some((
        Tag { name, closing, attrs: after.to_string() },
        end + 1,
    ))
}

fn emit_tag(tag: &Tag, out: &mut String) {
    match (tag.name.as_str(), tag.closing) {
        ("br", _) | ("hr", false) => out.push('\n'),
        ("li", false) => {
            newline(out);
            out.push_str("• ");
        }
        ("summary", false) => {
            newline(out);
            out.push_str("▸ ");
        }
        ("td" | "th", false) => {
            if !out.ends_with('\n') && !out.is_empty() {
                out.push_str(" │ ");
            }
        }
        ("img", false) => {
            let alt = attr(&tag.attrs, "alt").unwrap_or_default();
            if alt.is_empty() {
                out.push_str("[image]");
            } else {
                out.push_str(&format!("[{}]", alt));
            }
        }
        ("code" | "kbd" | "samp" | "tt", _) => out.push('`'),
        (
            "p" | "div" | "details" | "summary" | "table" | "thead" | "tbody" | "tfoot" | "tr"
            | "ul" | "ol" | "li" | "pre" | "blockquote" | "dl" | "dt" | "dd" | "h1" | "h2"
            | "h3" | "h4" | "h5" | "h6",
            _,
        ) => newline(out),
        _ => {}
    }
}

fn newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut search = 0;
    while let Some(pos) = lower[search..].find(name) {
        let start = search + pos;
        search = start + name.len();
        let preceded_ok = start == 0 || lower.as_bytes()[start - 1].is_ascii_whitespace();
        let rest = lower[search..].trim_start();
        if !preceded_ok || !rest.starts_with('=') {
            continue;
        }
        let value_start = attrs.len() - rest.len() + 1;
        let value = attrs[value_start..].trim_start();
        let value = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split_whitespace().next().unwrap_or(""),
        };
        return Some(to_text(value).into_owned());
    }
    None
}

fn decode_entity(s: &str) -> Option<(char, usize)> {
    let end = s[..s.len().min(12)].find(';')?;
    let name = &s[1..end];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "rarr" => '→',
        "larr" => '←',
        "check" => '✓',
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((ch, end + 1))
}

/// Drop trailing spaces and squeeze the runs of blank lines that block tags
/// leave behind.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out.trim_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::to_text;

    #[test]
    fn plain_markdown_is_borrowed() {
        assert!(matches!(to_text("just **text**"), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn details_and_breaks() {
        let html = "<details><summary>Click me</summary>\n\nhidden<br>body\n</details>";
        assert_eq!(to_text(html), "▸ Click me\n\nhidden\nbody");
    }

    #[test]
    fn comments_and_entities() {
        assert_eq!(to_text("a <!-- bot marker --> b &amp; c &#39;d&#x27;"), "a  b & c 'd'");
    }

    #[test]
    fn multiline_comment() {
        assert_eq!(to_text("before <!-- start\nhidden\nend --> after"), "before\n\n after");
        assert_eq!(to_text("<!--\nbot state\n-->\nReal text"), "Real text");
    }

    #[test]
    fn generics_are_not_tags() {
        assert_eq!(to_text("returns Vec<String> & HashMap<K, V>"), "returns Vec<String> & HashMap<K, V>");
        assert_eq!(to_text("use `<br>` here"), "use `<br>` here");
    }

    #[test]
    fn code_fences_untouched() {
        let s = "```html\n<p>hi</p>\n```\n<p>x</p>";
        assert_eq!(to_text(s), "```html\n<p>hi</p>\n```\nx");
    }

    #[test]
    fn tables_and_images() {
        let html = "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\n<img src=\"x.png\" alt=\"Logo\">";
        assert_eq!(to_text(html), "A │ B\n1 │ 2\n\n[Logo]");
    }

    #[test]
    fn lists() {
        assert_eq!(to_text("<ul><li>one</li><li>two</li></ul>"), "• one\n• two");
    }
}
