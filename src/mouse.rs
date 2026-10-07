//! Mouse input: wheel scrolling, click to focus/select, drag to select and
//! copy text. Pane positions come from the `HitMap` the last frame recorded.

use crate::app::{App, DiffFocus, Panel, Selection};
use crate::ui::inner;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::io::Write;
use std::process::{Command, Stdio};

/// Handle one mouse event. Returns whether the screen needs a repaint.
pub fn handle(app: &mut App, m: MouseEvent) -> bool {
    let pos = (m.column, m.row);
    match m.kind {
        MouseEventKind::ScrollUp => {
            app.selection = None;
            scroll(app, pos, -3)
        }
        MouseEventKind::ScrollDown => {
            app.selection = None;
            scroll(app, pos, 3)
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.flash = None;
            let pane = pane_at(app, pos);
            let anchor = clamp(pos, pane);
            app.selection = Some(Selection {
                pane,
                anchor,
                head: anchor,
                dragged: false,
                copy: false,
            });
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => match app.selection.as_mut() {
            Some(sel) => {
                sel.head = clamp(pos, sel.pane);
                sel.dragged = true;
                true
            }
            None => false,
        },
        MouseEventKind::Up(MouseButton::Left) => {
            match app.selection.as_mut() {
                Some(sel) if sel.dragged && sel.head != sel.anchor => sel.copy = true,
                _ => {
                    app.selection = None;
                    click(app, pos);
                }
            }
            true
        }
        _ => false,
    }
}

/// Wheel scrolls the code in the diff view whatever has focus; in the
/// overview it acts on the pane under the pointer.
fn scroll(app: &mut App, pos: (u16, u16), delta: isize) -> bool {
    let hit = app.hit.borrow().clone();
    if let Some(dv) = &mut app.diff_view {
        if dv.loading_review || !dv.review_output.is_empty() {
            dv.review_scroll = if delta < 0 {
                dv.review_scroll.saturating_sub(delta.unsigned_abs() as u16)
            } else {
                dv.review_scroll.saturating_add(delta as u16)
            };
        } else {
            dv.wheel_scroll(delta);
        }
        return true;
    }
    if contains(hit.prs, pos) {
        if delta < 0 {
            app.move_up();
        } else {
            app.move_down();
        }
        true
    } else if contains(hit.details, pos) {
        app.details_scroll = if delta < 0 {
            app.details_scroll.saturating_sub(delta.unsigned_abs() as u16)
        } else {
            app.details_scroll.saturating_add(delta as u16)
        };
        true
    } else {
        false
    }
}

fn click(app: &mut App, pos: (u16, u16)) {
    if app.show_help {
        app.show_help = false;
        return;
    }
    if app.approve_popup.is_some() || app.comment_popup.is_some() || app.confirm_quit.is_some() {
        return;
    }
    let hit = app.hit.borrow().clone();

    if let Some(dv) = &mut app.diff_view {
        if dv.input_mode.is_some() || hit.popup.is_some() {
            return;
        }
        if contains(hit.tree, pos) {
            app.diff_focus = DiffFocus::Files;
            if let Some(row) = row_in(hit.tree, pos) {
                let idx = hit.tree_offset + row;
                if dv.tree.get(idx).is_some_and(|n| !n.is_dir) {
                    app.tree_index = idx;
                    dv.tree_select(idx);
                }
            }
        } else if contains(hit.content, pos) {
            app.diff_focus = DiffFocus::Content;
            if let Some(&(_, li)) = hit.code_rows.iter().find(|(y, _)| *y == pos.1) {
                dv.cursor_line = li;
            }
        }
        return;
    }

    if app.search_mode {
        return;
    }
    if contains(hit.prs, pos) {
        app.active_panel = Panel::PullRequests;
        if let Some(row) = row_in(hit.prs, pos) {
            // The separator is one row, every PR two.
            let mut top = 0;
            for item in hit.pr_items.iter().skip(hit.prs_offset) {
                let height = if item.is_some() { 2 } else { 1 };
                if row < top + height {
                    if let Some(i) = item {
                        app.select_pr(*i);
                    }
                    break;
                }
                top += height;
            }
        }
    } else if contains(hit.details, pos) {
        app.active_panel = Panel::Details;
    }
}

/// The area a drag starting at `pos` is confined to.
fn pane_at(app: &App, pos: (u16, u16)) -> Rect {
    let hit = app.hit.borrow();
    let candidates = [hit.popup.unwrap_or_default(), hit.tree, hit.content, hit.prs, hit.details];
    candidates
        .into_iter()
        .find(|r| contains(*r, pos))
        .map(inner)
        .unwrap_or(hit.screen)
}

fn contains(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// Row inside a bordered pane, if `pos` isn't on the border.
fn row_in(r: Rect, pos: (u16, u16)) -> Option<usize> {
    let i = inner(r);
    contains(i, pos).then(|| (pos.1 - i.y) as usize)
}

fn clamp((x, y): (u16, u16), r: Rect) -> (u16, u16) {
    if r.width == 0 || r.height == 0 {
        return (x, y);
    }
    (
        x.clamp(r.x, r.right() - 1),
        y.clamp(r.y, r.bottom() - 1),
    )
}

/// Put text on the system clipboard. Tries the platform tools first, then
/// falls back to OSC 52, which most modern terminals honour.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let tools: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
    ];
    for (cmd, args) in tools {
        let Ok(mut child) = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return Ok(());
        }
    }

    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_padding() {
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"a"), "YQ==");
    }
}
