use crate::app::{ci_icon, spinner_char as spinner, App, HitMap, Panel, Selection};
use crate::diff_view::{CursorTarget, DiffView, LineKind};
use crate::github::CiState;
use crate::html;
use unicode_width::UnicodeWidthStr;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

const NERD_PR: &str = "\u{f407}";
const NERD_USER: &str = "\u{f007}";
const NERD_BRANCH: &str = "\u{e725}";
const NERD_PLUS: &str = "\u{f457}";
const NERD_MINUS: &str = "\u{f458}";
const NERD_FILE: &str = "\u{f459}";
const NERD_REFRESH: &str = "\u{f450}";

/// Build a padded Line: apply bg to all spans + pad with spaces to fill `width`
fn padded_line(spans: Vec<Span<'_>>, width: usize, bg: Color) -> Line<'_> {
    let mut result: Vec<Span> = spans.into_iter().map(|s| {
        let mut style = s.style;
        style.bg = Some(bg);
        Span::styled(s.content, style)
    }).collect();
    let used: usize = result.iter().map(|s| s.content.len()).sum();
    if used < width {
        result.push(Span::styled(" ".repeat(width - used), Style::default().bg(bg)));
    }
    Line::from(result)
}

/// Word-wrap text: first output line at `first_width`, continuation lines at `rest_width`
/// Find the byte index of the char boundary at or before byte position `pos`
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() { return s.len(); }
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn wrap_text_2(text: &str, first_width: usize, rest_width: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    let fw = first_width.max(10);
    let rw = rest_width.max(10);
    for line in text.lines() {
        let w = if wrapped.is_empty() { fw } else { rw };
        if line.len() <= w {
            wrapped.push(line.to_string());
        } else {
            let mut remaining = line;
            let mut cur_w = w;
            while remaining.len() > cur_w {
                let safe_end = floor_char_boundary(remaining, cur_w);
                let break_at = remaining[..safe_end]
                    .rfind(' ')
                    .unwrap_or(safe_end);
                let break_at = break_at.max(1);
                wrapped.push(remaining[..break_at].to_string());
                remaining = remaining[break_at..].trim_start();
                cur_w = rw;
            }
            if !remaining.is_empty() {
                wrapped.push(remaining.to_string());
            }
        }
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
}

pub fn draw(f: &mut Frame, app: &mut App) {
    app.frame = app.frame.wrapping_add(1);
    *app.hit.borrow_mut() = HitMap { screen: f.area(), ..Default::default() };
    let size = f.area();

    // Ensure syntax highlighting and scroll are correct for current file
    if let Some(dv) = &mut app.diff_view {
        dv.ensure_highlighted(&app.highlighter);
        // Compute content area dimensions for scroll adjustment
        let file_pane_w = app.file_pane_width.min(size.width / 2);
        let content_w = size.width.saturating_sub(file_pane_w + 2) as usize;
        let content_h = size.height.saturating_sub(3) as usize; // borders + status bar
        dv.adjust_scroll(content_h, content_w);

        // Compute rendered position for input overlay
        dv.input_target_line = None;
        if dv.input_mode.is_some() {
            let heights = dv.compute_line_heights(content_w.saturating_sub(14));
            let target_diff_line = match &dv.input_mode {
                Some(crate::diff_view::InputMode::Reply { thread_idx, .. }) => {
                    // Find the diff line this thread is attached to
                    dv.line_threads.iter()
                        .find(|(_, tis)| tis.contains(thread_idx))
                        .map(|(&li, _)| li)
                },
                Some(crate::diff_view::InputMode::NewComment { diff_line }) => Some(*diff_line),
                Some(crate::diff_view::InputMode::EditClaude { claude_idx }) => {
                    dv.line_claude.iter()
                        .find(|(_, cis)| cis.contains(claude_idx))
                        .map(|(&li, _)| li)
                },
                Some(crate::diff_view::InputMode::EditDraft { draft_idx }) => {
                    dv.line_drafts.iter()
                        .find(|(_, dis)| dis.contains(draft_idx))
                        .map(|(&li, _)| li)
                },
                None => None,
            };
            if let Some(target) = target_diff_line {
                if target >= dv.scroll {
                    // Sum rendered heights from scroll to target (inclusive)
                    let rendered: usize = heights.get(dv.scroll..=target)
                        .map(|s| s.iter().sum())
                        .unwrap_or(0);
                    dv.input_target_line = Some(rendered);
                }
            }
        }
    }

    if let Some(dv) = &app.diff_view {
        // Diff view — full screen, no title bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(size);
        draw_diff_view(f, app, dv, chunks[0]);
        draw_diff_view_status_bar(f, dv, app.frame, chunks[1]);
    } else {
        // PR list view
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(size);
        draw_overview(f, app, chunks[0]);
        draw_status_bar(f, app, chunks[1]);
    }

    if let Some(popup) = &app.approve_popup {
        draw_approve_popup(f, popup, size);
    }

    if let Some(popup) = &app.comment_popup {
        draw_comment_popup(f, popup, size);
    }

    if app.confirm_quit.is_some() {
        draw_confirm_quit_popup(f, app, size);
    }

    if app.show_help {
        draw_help_popup(f, size);
    }

    if let Some(msg) = &app.flash {
        draw_flash(f, msg, size);
    }
    draw_selection(f, app);
}

/// Short notice in the bottom-right corner, over the status bar.
fn draw_flash(f: &mut Frame, msg: &str, area: Rect) {
    let text = format!(" {} ", msg);
    let w = (UnicodeWidthStr::width(text.as_str()) as u16).min(area.width);
    if w == 0 || area.height == 0 {
        return;
    }
    let rect = Rect::new(area.right() - w, area.bottom() - 1, w, 1);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Black).bg(Color::Green)),
        rect,
    );
}

/// Width of the line-number/sign gutter in front of each diff line. Copying
/// skips it so selected code pastes cleanly.
const DIFF_GUTTER: u16 = 14;

/// Per-row column ranges (inclusive) covered by the selection, in screen
/// order. Stream selection like a terminal, clipped to the pane.
fn selection_rows(sel: &Selection, hit: &HitMap) -> Vec<(u16, u16, u16)> {
    let (start, end) = if (sel.anchor.1, sel.anchor.0) <= (sel.head.1, sel.head.0) {
        (sel.anchor, sel.head)
    } else {
        (sel.head, sel.anchor)
    };
    let pane = sel.pane;
    let in_content = hit.content.width > 0 && pane == inner(hit.content);
    let mut rows = Vec::new();
    for y in start.1..=end.1 {
        let mut x0 = if y == start.1 { start.0 } else { pane.x };
        let x1 = if y == end.1 { end.0 } else { pane.right().saturating_sub(1) };
        if in_content && hit.code_rows.iter().any(|(row, _)| *row == y) {
            x0 = x0.max(pane.x + DIFF_GUTTER);
        }
        if x0 <= x1 {
            rows.push((y, x0, x1));
        }
    }
    rows
}

pub fn inner(r: Rect) -> Rect {
    Rect::new(
        r.x.saturating_add(1),
        r.y.saturating_add(1),
        r.width.saturating_sub(2),
        r.height.saturating_sub(2),
    )
}

fn draw_selection(f: &mut Frame, app: &mut App) {
    let Some(sel) = app.selection.as_mut() else { return };
    if !sel.dragged {
        return;
    }
    let rows = selection_rows(sel, &app.hit.borrow());
    let buf = f.buffer_mut();

    if sel.copy {
        sel.copy = false;
        let mut text = Vec::new();
        for &(y, x0, x1) in &rows {
            let mut line = String::new();
            let mut x = x0;
            while x <= x1 {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                // A wide character owns the cells after it.
                x += (UnicodeWidthStr::width(sym) as u16).max(1);
            }
            text.push(line.trim_end().to_string());
        }
        app.clipboard_out = Some(text.join("\n"));
    }

    for &(y, x0, x1) in &rows {
        for x in x0..=x1 {
            buf[(x, y)].set_bg(Color::Rgb(60, 80, 140));
        }
    }
}


fn draw_help_popup(f: &mut Frame, area: Rect) {
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 52u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup_area = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help — press any key to close ")
        .border_style(Style::default().fg(Color::Cyan));

    let h = |icon: &str, color: Color, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("  {} ", icon), Style::default().fg(color)),
            Span::styled(desc.to_string(), Style::default().fg(Color::White)),
        ])
    };

    // Legend row whose icon and count are coloured separately, matching the
    // two-tone indicators in the PR list.
    let two = |icon: &str,
               icon_color: Color,
               count: &str,
               count_color: Color,
               desc: &str|
     -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("  {}", icon), Style::default().fg(icon_color)),
            Span::styled(format!("{} ", count), Style::default().fg(count_color)),
            Span::styled(desc.to_string(), Style::default().fg(Color::White)),
        ])
    };

    let s = |text: &str| -> Line<'static> {
        Line::from(Span::styled(
            format!("  {}", text),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
    };

    let lines = vec![
        Line::from(""),
        s("PR Icons"),
        h("\u{f407}", Color::Green, "Open PR / ready to merge"),
        h("\u{f407}", Color::Red, "PR has conflicts / blocked"),
        h("\u{f407}", Color::Rgb(255, 165, 0), "Draft pull request"),
        Line::from(""),
        s("CI Status"),
        h("\u{f00c}", Color::Green, "CI passing"),
        h("\u{f00d}", Color::Red, "CI failing"),
        h("\u{f110}", Color::Yellow, "CI pending"),
        h("\u{f128}", Color::DarkGray, "CI unknown"),
        Line::from(""),
        s("Review Status"),
        h("\u{f164}", Color::Green, "Approved by you"),
        h("\u{f164}", Color::Cyan, "Approved by someone"),
        h("\u{f467}", Color::Cyan, "Changes requested (review)"),
        h("\u{f4a1}", Color::Cyan, "Pending review"),
        Line::from(""),
        s("Attention"),
        h("\u{f06a}", Color::Red, "Changes requested"),
        h("\u{f075}", Color::Yellow, "Has review comments"),
        Line::from(""),
        s("Local review draft"),
        h("⠹", Color::Rgb(200, 120, 255), "AI review running now"),
        h("\u{f017}", Color::DarkGray, "AI review queued"),
        h("\u{f071}", Color::Red, "AI review failed (see Details)"),
        two(
            "\u{f12a}",
            Color::Magenta,
            "n",
            crate::app::AI_COUNT_COLOR,
            "AI findings to triage",
        ),
        two(
            "\u{f040}",
            Color::Green,
            "n",
            crate::app::DRAFT_COUNT_COLOR,
            "your comments to submit",
        ),
        Line::from(""),
        s("Keys (PR list)"),
        h("↑↓/jk", Color::DarkGray, "Navigate"),
        h("a", Color::DarkGray, "Toggle assigned / all"),
        h("/", Color::DarkGray, "Filter"),
        h("c", Color::DarkGray, "Run / retry AI review"),
        Line::from(""),
        s("Keys (Details)"),
        h("↑↓/jk", Color::DarkGray, "Scroll"),
        h("a", Color::DarkGray, "Comment on PR"),
        Line::from(""),
        s("Keys (Global)"),
        h("Tab", Color::DarkGray, "Switch panel"),
        h("A", Color::DarkGray, "Approve PR"),
        h("Enter", Color::DarkGray, "Load diff"),
        h("r", Color::DarkGray, "Refresh"),
        h("o", Color::DarkGray, "Open in browser"),
        h("y", Color::DarkGray, "Copy thread (in diff view)"),
        h("?", Color::DarkGray, "Help"),
        h("q", Color::DarkGray, "Quit"),
        Line::from(""),
        s("Keys (AI comment in diff)"),
        h("a", Color::DarkGray, "Accept as draft comment"),
        h("e", Color::DarkGray, "Edit, then accept"),
        h("d", Color::DarkGray, "Discard / bring back"),
        Line::from(""),
        s("Keys (your own draft comment)"),
        h("n/N", Color::DarkGray, "Jump between all comments"),
        h("e", Color::DarkGray, "Edit the draft"),
        h("d", Color::DarkGray, "Remove the draft"),
        h("S", Color::DarkGray, "Submit all drafts"),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, popup_area);
}


fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ])
        .split(area);

    draw_prs_panel(f, app, chunks[0]);
    draw_details_panel(f, app, chunks[1]);
}

fn panel_border_style(app: &App, panel: Panel) -> Style {
    if app.active_panel == panel && app.diff_view.is_none() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn draw_prs_panel(f: &mut Frame, app: &App, area: Rect) {
    app.hit.borrow_mut().prs = area;
    if app.loading {
        let loading = Paragraph::new(format!(" Loading pull requests... {}", spinner(app.frame)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} Pull Requests {} ", NERD_PR, spinner(app.frame)))
                    .border_style(panel_border_style(app, Panel::PullRequests)),
            )
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(loading, area);
        return;
    }

    if let Some(ref error) = app.error {
        let err = Paragraph::new(error.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Error ")
                    .border_style(Style::default().fg(Color::Red)),
            )
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: true });
        f.render_widget(err, area);
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();

    for (i, fpr) in app.flat_prs.iter().enumerate() {
            // Insert separator before the approved section
            if let Some(sep_idx) = app.approved_separator {
                if i == sep_idx {
                    let sep_line = Line::from(vec![
                        Span::styled("  ── ", Style::default().fg(Color::DarkGray)),
                        Span::styled("Approved by me", Style::default().fg(Color::DarkGray)),
                        Span::styled(" ──────────────────────────────────────", Style::default().fg(Color::DarkGray)),
                    ]);
                    items.push(ListItem::new(vec![sep_line]));
                    app.hit.borrow_mut().pr_items.push(None);
                }
            }

            let pr = &fpr.pr;
            let style = if i == app.pr_index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == app.pr_index { "▸ " } else { "  " };

            let pr_icon = NERD_PR;
            let pr_icon_color = if pr.draft {
                Color::Rgb(255, 165, 0) // orange for draft
            } else if pr.mergeable == Some(false)
                || matches!(pr.mergeable_state.as_deref(), Some("dirty") | Some("blocked"))
            {
                Color::Red
            } else {
                Color::Green
            };

            let mut indicators = Vec::new();
            if let Some(status) = app.pr_status(&fpr.repo_name, pr.number) {
                let (ci_sym, ci_color) = match status.ci_state {
                    CiState::Success => (ci_icon(&CiState::Success), Color::Green),
                    CiState::Failure => (ci_icon(&CiState::Failure), Color::Red),
                    CiState::Pending => (ci_icon(&CiState::Pending), Color::Yellow),
                    CiState::Unknown => (ci_icon(&CiState::Unknown), Color::DarkGray),
                };
                indicators.push(Span::styled(
                    format!("{} ", ci_sym),
                    Style::default().fg(ci_color),
                ));

                // Changes requested / comments indicator
                let mut latest_reviews: Vec<(&str, &str)> = Vec::new();
                for review in &status.reviews {
                    if let Some(entry) = latest_reviews.iter_mut().find(|(u, _)| *u == review.user.login.as_str()) {
                        entry.1 = &review.state;
                    } else {
                        latest_reviews.push((&review.user.login, &review.state));
                    }
                }
                let has_changes_requested = latest_reviews.iter().any(|(_, s)| *s == "CHANGES_REQUESTED");
                let has_comments = !status.comments.is_empty();
                let has_commented_review = latest_reviews.iter().any(|(_, s)| *s == "COMMENTED");

                if has_changes_requested {
                    indicators.push(Span::styled(
                        "\u{f06a} ", // nf-fa-exclamation_circle
                        Style::default().fg(Color::Red),
                    ));
                } else if has_comments || has_commented_review {
                    indicators.push(Span::styled(
                        "\u{f075} ", // nf-fa-comment
                        Style::default().fg(Color::Yellow),
                    ));
                }

                let review_icon = app.review_icon(&fpr.repo_name, pr);
                if !review_icon.is_empty() {
                    let review_color = if app.is_approved_by_me(&fpr.repo_name, pr.number) {
                        Color::Green
                    } else {
                        Color::Cyan
                    };
                    indicators.push(Span::styled(
                        review_icon.to_string(),
                        Style::default().fg(review_color),
                    ));
                }
            }

            // Local draft state: review running/queued/failed, undecided AI
            // findings, unsubmitted changes
            for (icon, color) in app.draft_indicators(&fpr.repo_name, pr.number) {
                indicators.push(Span::styled(icon, Style::default().fg(color)));
            }

            // Merge status indicators are now reflected in PR icon color

            let mut spans = vec![
                Span::styled(prefix, style),
                Span::styled(
                    format!("{} ", pr_icon),
                    Style::default().fg(pr_icon_color),
                ),
            ];
            spans.extend(indicators);
            spans.push(Span::styled(
                format!("#{} ", pr.number),
                Style::default().fg(Color::Magenta),
            ));
            // Truncate title to fit
            let max_title = (area.width as usize).saturating_sub(20);
            let title = if pr.title.len() > max_title {
                format!("{}…", &pr.title[..max_title.saturating_sub(1)])
            } else {
                pr.title.clone()
            };
            spans.push(Span::styled(title, style));

            let line1 = Line::from(spans);
            let line2 = Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    &fpr.repo_short,
                    Style::default().fg(Color::Blue),
                ),
                Span::styled(
                    format!(" {} {}", NERD_USER, pr.user.login),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let item = ListItem::new(vec![line1, line2]);
            let is_mine = !app.username.is_empty() && pr.user.login == app.username;
            let bg = match (i == app.pr_index, is_mine) {
                (true, true) => Some(Color::Rgb(30, 70, 40)),
                (false, true) => Some(Color::Rgb(15, 45, 22)),
                (true, false) => Some(Color::Rgb(25, 25, 40)),
                (false, false) => None,
            };
            app.hit.borrow_mut().pr_items.push(Some(i));
            items.push(match bg {
                Some(bg) => item.style(Style::default().bg(bg)),
                None => item,
            });
    }

    let selected = if let Some(sep_idx) = app.approved_separator {
        if app.pr_index >= sep_idx {
            app.pr_index + 1 // account for separator item
        } else {
            app.pr_index
        }
    } else {
        app.pr_index
    };
    let spin = if app.is_fetching() {
        format!(" {}", spinner(app.frame))
    } else {
        String::new()
    };
    let title = if app.search_query.is_empty() {
        format!(" {} Pull Requests ({}){} ", NERD_PR, app.flat_prs.len(), spin)
    } else {
        format!(" {} Pull Requests ({}) /{}{} ", NERD_PR, app.flat_prs.len(), app.search_query, spin)
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(panel_border_style(app, Panel::PullRequests)),
    );
    let mut state = ListState::default().with_selected(Some(selected));
    f.render_stateful_widget(list, area, &mut state);
    app.hit.borrow_mut().prs_offset = state.offset();
}

fn draw_details_panel(f: &mut Frame, app: &App, area: Rect) {
    app.hit.borrow_mut().details = area;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Details ")
        .border_style(panel_border_style(app, Panel::Details));

    let Some(fpr) = app.selected_flat_pr() else {
        let empty = Paragraph::new(" Select a PR to view details")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, area);
        return;
    };

    let pr = &fpr.pr;
    let repo_name = &fpr.repo_name;
    let pr_icon = NERD_PR;
    let pr_icon_color = if pr.draft { Color::Rgb(255, 165, 0) } else { Color::Green };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} #{} ", pr_icon, pr.number),
                Style::default()
                    .fg(pr_icon_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if pr.draft { "[DRAFT]" } else { "" },
                Style::default().fg(Color::Rgb(255, 165, 0)),
            ),
        ]),
        Line::from(Span::styled(
            &pr.title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{} ", NERD_USER),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(&pr.user.login, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", NERD_BRANCH),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(
                format!("{} → {}", pr.head.ref_name, pr.base.ref_name),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{} ", NERD_PLUS),
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                format!("+{}", pr.additions),
                Style::default().fg(Color::Green),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} ", NERD_MINUS),
                Style::default().fg(Color::Red),
            ),
            Span::styled(
                format!("-{}", pr.deletions),
                Style::default().fg(Color::Red),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} ", NERD_FILE),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!("{} files", pr.changed_files),
                Style::default().fg(Color::Yellow),
            ),
        ]),
    ];

    // Merge status
    match pr.mergeable {
        Some(false) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "\u{f06a} Conflict — cannot merge",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }
        Some(true) => {
            if pr.mergeable_state.as_deref() == Some("clean") {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "\u{f00c} Ready to merge",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                )));
            }
        }
        _ => {}
    }

    // Description
    if let Some(body) = &pr.body {
        let body_text = html::to_text(body);
        let body_trimmed = body_text.trim();
        if !body_trimmed.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Description:",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )));
            for (i, desc_line) in body_trimmed.lines().enumerate() {
                if i >= 15 {
                    lines.push(Line::from(Span::styled(
                        "  …",
                        Style::default().fg(Color::DarkGray),
                    )));
                    break;
                }
                lines.push(Line::from(Span::styled(
                    format!("  {}", desc_line),
                    Style::default().fg(Color::Rgb(180, 180, 200)),
                )));
            }
        }
    }

    // Local AI review state — including why it failed and how to retry.
    if let Some(draft) = app.drafts.get(repo_name, pr.number) {
        use crate::draft::ReviewState;
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("\u{f12a} {} review:", app.config.ai.name),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));

        let running = app.is_reviewing(repo_name, pr.number);
        if running {
            lines.push(Line::from(Span::styled(
                format!("  {} running…", spinner(app.frame)),
                Style::default().fg(Color::Magenta),
            )));
        } else {
            match draft.review_state {
                ReviewState::Queued => lines.push(Line::from(Span::styled(
                    "  \u{f017} queued",
                    Style::default().fg(Color::DarkGray),
                ))),
                ReviewState::Running => lines.push(Line::from(Span::styled(
                    "  \u{f017} queued (will restart)",
                    Style::default().fg(Color::DarkGray),
                ))),
                ReviewState::Failed => {
                    lines.push(Line::from(Span::styled(
                        "  \u{f071} failed",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )));
                    // The panel wraps its own text, so hand it one line.
                    let reason = draft
                        .review_error
                        .as_deref()
                        .unwrap_or("no reason recorded");
                    lines.push(Line::from(Span::styled(
                        format!("  {}", reason),
                        Style::default().fg(Color::Rgb(255, 140, 140)),
                    )));
                    lines.push(Line::from(vec![
                        Span::styled("  press ", Style::default().fg(Color::DarkGray)),
                        Span::styled("c", Style::default().fg(Color::Yellow)),
                        Span::styled(" to run it again", Style::default().fg(Color::DarkGray)),
                    ]));
                }
                ReviewState::Done => {
                    let pending = draft.pending_ai_count();
                    lines.push(Line::from(Span::styled(
                        format!(
                            "  \u{f00c} {} finding{}, {} to triage",
                            draft.ai_comments.len(),
                            if draft.ai_comments.len() == 1 { "" } else { "s" },
                            pending
                        ),
                        Style::default().fg(Color::Green),
                    )));
                }
                ReviewState::NotRun => lines.push(Line::from(Span::styled(
                    "  not run — press c to run it",
                    Style::default().fg(Color::DarkGray),
                ))),
            }
        }
    } else if app.is_approved_by_me(repo_name, pr.number) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "\u{f12a} AI review skipped — approved by you (c to run anyway)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    if let Some(status) = app.pr_status(repo_name, pr.number) {
        lines.push(Line::from(""));

        let (ci_sym, ci_color, ci_text) = match status.ci_state {
            CiState::Success => (ci_icon(&CiState::Success), Color::Green, "Passing"),
            CiState::Failure => (ci_icon(&CiState::Failure), Color::Red, "Failing"),
            CiState::Pending => (ci_icon(&CiState::Pending), Color::Yellow, "Pending"),
            CiState::Unknown => (ci_icon(&CiState::Unknown), Color::DarkGray, "Unknown"),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} CI: ", ci_sym), Style::default().fg(ci_color)),
            Span::styled(ci_text, Style::default().fg(ci_color)),
        ]));

        if !status.reviews.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Reviews:",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));

            let mut latest: Vec<(&str, &str)> = Vec::new();
            for review in &status.reviews {
                if let Some(entry) =
                    latest.iter_mut().find(|(u, _)| *u == review.user.login.as_str())
                {
                    entry.1 = &review.state;
                } else {
                    latest.push((&review.user.login, &review.state));
                }
            }
            for (user, state) in &latest {
                let (icon, color) = match *state {
                    "APPROVED" => ("\u{f00c}", Color::Green),
                    "CHANGES_REQUESTED" => ("\u{f00d}", Color::Red),
                    "COMMENTED" => ("\u{f075}", Color::Cyan),
                    _ => ("\u{f128}", Color::DarkGray),
                };
                let is_me = *user == app.username;
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                    Span::styled(
                        format!("{}{}", user, if is_me { " (you)" } else { "" }),
                        Style::default().fg(if is_me { Color::Yellow } else { Color::White }),
                    ),
                ]));
            }
        }

        // Changed files
        if !status.files.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("{} Files ({}):", NERD_FILE, status.files.len()),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            for file in &status.files {
                let (icon, color) = match file.status.as_str() {
                    "added" => ("\u{f457}", Color::Green),       // nf-oct-diff_added
                    "removed" => ("\u{f458}", Color::Red),       // nf-oct-diff_removed
                    "renamed" => ("\u{f553}", Color::Cyan),      // nf-oct-arrow_right
                    _ => ("\u{f459}", Color::Yellow),            // nf-oct-diff_modified
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                    Span::styled(&file.filename, Style::default().fg(Color::White)),
                    Span::styled(
                        format!(" +{} -{}", file.additions, file.deletions),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }

        let total_comments = status.comments.len() + status.review_comments.len();
        if total_comments > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("\u{f075} Comments ({}):", total_comments),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));

            let mut all_comments: Vec<(
                &str,
                &str,
                &chrono::DateTime<chrono::Utc>,
                Option<&str>,
            )> = Vec::new();
            for c in &status.comments {
                all_comments.push((&c.user.login, &c.body, &c.created_at, None));
            }
            for c in &status.review_comments {
                all_comments.push((&c.user.login, &c.body, &c.created_at, Some(&c.path)));
            }
            all_comments.sort_by_key(|c| c.2);

            for (user, body, time, path) in &all_comments {
                lines.push(Line::from(""));
                let mut header = vec![
                    Span::styled(
                        format!("  {} {} ", NERD_USER, user),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        time.format("%m-%d %H:%M").to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                if let Some(p) = path {
                    let short = p.rsplit('/').next().unwrap_or(p);
                    header.push(Span::styled(
                        format!("  {}", short),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                lines.push(Line::from(header));
                let body_text = html::to_text(body);
                let body_trimmed = body_text.trim();
                for (i, line) in body_trimmed.lines().enumerate() {
                    if i >= 6 {
                        lines.push(Line::from(Span::styled(
                            "  …",
                            Style::default().fg(Color::DarkGray),
                        )));
                        break;
                    }
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::White),
                    )));
                }
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("Updated: {}", pr.updated_at.format("%Y-%m-%d %H:%M UTC")),
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true })
        .scroll((app.details_scroll, 0));
    f.render_widget(paragraph, area);
}



fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let filter_label = if app.show_assigned_only {
        "assigned"
    } else if app.all_repos_loaded {
        "all"
    } else {
        "all…"
    };
    let panel_keys = match app.active_panel {
        Panel::PullRequests => format!(
            "↑↓ navigate │ Tab panel │ a [{filter_label}] │ / filter │ Enter diff │ A approve │ c ai review │ r refresh │ o open │ ? help │ q quit"
        ),
        Panel::Details => format!(
            "↑↓ scroll │ Tab panel │ a comment │ A approve │ Enter diff │ c ai review │ r refresh │ o open │ ? help │ q quit"
        ),
    };

    if app.search_mode {
        let search_line = Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Yellow)),
            Span::styled(&app.search_query, Style::default().fg(Color::White)),
            Span::styled("▎", Style::default().fg(Color::Yellow)),
            Span::styled("  Enter confirm │ Esc clear", Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(search_line), chunks[0]);
    } else if !app.search_query.is_empty() {
        let filter_line = Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Rgb(255, 160, 50))),
            Span::styled(&app.search_query, Style::default().fg(Color::Rgb(255, 160, 50))),
            Span::styled(
                format!(" │ {} │ Esc clear filter", panel_keys),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(filter_line), chunks[0]);
    } else {
        let help = Paragraph::new(Span::styled(
            format!(" {}", panel_keys),
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(help, chunks[0]);
    }

    // Right side: auto-review progress, then the logged-in user
    let (running, queued) = app.auto_progress();
    let mut right: Vec<Span> = Vec::new();
    if running > 0 {
        right.push(Span::styled(
            format!("{} reviewing {} ", spinner(app.frame), running),
            Style::default().fg(Color::Magenta),
        ));
    }
    if queued > 0 {
        right.push(Span::styled(
            format!("\u{f017} {} queued ", queued),
            Style::default().fg(Color::DarkGray),
        ));
    }
    right.push(Span::styled(
        format!(" {} {} ", NERD_USER, app.username),
        Style::default().fg(Color::Cyan),
    ));

    let user_info = Paragraph::new(Line::from(right))
        .alignment(ratatui::layout::Alignment::Right);
    f.render_widget(user_info, chunks[1]);
}

// ── Confirm quit popup ─────────────────────────────────────

fn draw_confirm_quit_popup(f: &mut Frame, app: &App, area: Rect) {
    let (items, prs) = app.unsubmitted_totals();

    let w = 54u16.min(area.width.saturating_sub(4));
    let h = 8u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup_area = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Not submitted yet ")
        .border_style(Style::default().fg(Color::Yellow));

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  {} item{} on {} PR{} not sent to GitHub.",
                items,
                if items == 1 { "" } else { "s" },
                prs,
                if prs == 1 { "" } else { "s" },
            ),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "  They stay saved locally — press S in the diff",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  view to submit them.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  y ", Style::default().fg(Color::Green)),
            Span::styled("quit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("any key ", Style::default().fg(Color::Yellow)),
            Span::styled("stay", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block), popup_area);
}

// ── Approve popup ──────────────────────────────────────────

fn draw_approve_popup(f: &mut Frame, popup: &crate::app::ApprovePopup, area: Rect) {
    let w = 60u16.min(area.width.saturating_sub(4));
    let h = 10u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup_area = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup_area);

    let border_color = if popup.result_msg.is_some() {
        Color::Green
    } else {
        Color::Yellow
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Approve #{} ", popup.pr_number))
        .border_style(Style::default().fg(border_color));

    if let Some(msg) = &popup.result_msg {
        let color = if msg.starts_with('✓') { Color::Green } else { Color::Red };
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(format!("  {}", msg), Style::default().fg(color))),
            Line::from(""),
            Line::from(Span::styled("  Press any key to close", Style::default().fg(Color::DarkGray))),
        ];
        f.render_widget(Paragraph::new(lines).block(block), popup_area);
        return;
    }

    if popup.submitting {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {} Submitting approval...", NERD_REFRESH),
                Style::default().fg(Color::Yellow),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(block), popup_area);
        return;
    }

    // Truncate title
    let max_t = (w as usize).saturating_sub(6);
    let title = if popup.pr_title.len() > max_t {
        format!("{}…", &popup.pr_title[..max_t.saturating_sub(1)])
    } else {
        popup.pr_title.clone()
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("  {}", title),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Comment (optional):",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!("  ▎{}", popup.comment),
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Enter ", Style::default().fg(Color::Green)),
            Span::styled("approve  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc ", Style::default().fg(Color::Red)),
            Span::styled("cancel", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block), popup_area);

    // Place terminal cursor on the input line at the cursor offset
    let cursor_offset = popup.cursor.min(popup.comment.len()) as u16;
    let cursor_x = (popup_area.x + 1 + 3 + cursor_offset)
        .min(popup_area.x + popup_area.width.saturating_sub(2));
    let cursor_y = popup_area.y + 1 + 3; // border + (title, blank, label) → input row
    f.set_cursor_position((cursor_x, cursor_y));
}

fn draw_comment_popup(f: &mut Frame, popup: &crate::app::CommentPopup, area: Rect) {
    let w = 60u16.min(area.width.saturating_sub(4));
    let h = 10u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup_area = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup_area);

    let border_color = if popup.result_msg.is_some() {
        Color::Green
    } else {
        Color::Cyan
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Comment #{} ", popup.pr_number))
        .border_style(Style::default().fg(border_color));

    if let Some(msg) = &popup.result_msg {
        let color = if msg.starts_with('✓') { Color::Green } else { Color::Red };
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(format!("  {}", msg), Style::default().fg(color))),
            Line::from(""),
            Line::from(Span::styled("  Press any key to close", Style::default().fg(Color::DarkGray))),
        ];
        f.render_widget(Paragraph::new(lines).block(block), popup_area);
        return;
    }

    if popup.submitting {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {} Posting comment...", NERD_REFRESH),
                Style::default().fg(Color::Yellow),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(block), popup_area);
        return;
    }

    let max_t = (w as usize).saturating_sub(6);
    let title = if popup.pr_title.len() > max_t {
        format!("{}…", &popup.pr_title[..max_t.saturating_sub(1)])
    } else {
        popup.pr_title.clone()
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("  {}", title),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Comment:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!("  ▎{}", popup.body),
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Enter ", Style::default().fg(Color::Green)),
            Span::styled("submit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc ", Style::default().fg(Color::Red)),
            Span::styled("cancel", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block), popup_area);

    // Place terminal cursor on the input line at the cursor offset
    let cursor_offset = popup.cursor.min(popup.body.len()) as u16;
    let cursor_x = (popup_area.x + 1 + 3 + cursor_offset)
        .min(popup_area.x + popup_area.width.saturating_sub(2));
    let cursor_y = popup_area.y + 1 + 3; // border + (title, blank, label) → input row
    f.set_cursor_position((cursor_x, cursor_y));
}

// ── Diff view (two-pane) ───────────────────────────────────

fn draw_diff_view(f: &mut Frame, app: &App, dv: &DiffView, area: Rect) {
    // Reserve a row at the top for the commit range indicator if we have commits
    let show_range_bar = !app.pr_commits.is_empty();
    let (range_area, body_area) = if show_range_bar {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        (Some(rows[0]), rows[1])
    } else {
        (None, area)
    };

    if let Some(ra) = range_area {
        draw_commit_range_bar(f, app, ra);
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(app.file_pane_width.min(body_area.width / 2)),
            Constraint::Min(0),
        ])
        .split(body_area);

    draw_file_tree(f, app, dv, chunks[0]);
    draw_diff_content(f, app, dv, chunks[1]);

    // Input overlay
    if dv.input_mode.is_some() {
        draw_input_overlay(f, dv, chunks[1]);
    }

    // Claude review output popup
    if dv.loading_review || !dv.review_output.is_empty() {
        let popup_width = (area.width as f32 * 0.8) as u16;
        let popup_height = (area.height as f32 * 0.75) as u16;
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);
        f.render_widget(Clear, popup_area);
        app.hit.borrow_mut().popup = Some(popup_area);

        let title = if dv.loading_review {
            format!(" {} Review [{}] {} ", app.config.ai.name, app.config.ai.command, spinner(app.frame))
        } else {
            format!(" {} Review [{}] (done) ", app.config.ai.name, app.config.ai.command)
        };
        let bottom = if dv.loading_review {
            " ↑↓ scroll │ Esc hide "
        } else {
            " Press any key to close "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(bottom)
            .border_style(Style::default().fg(Color::Magenta));

        let lines: Vec<Line> = dv.review_output
            .lines()
            .map(|l| Line::from(Span::raw(l)))
            .collect();

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((dv.review_scroll, 0));
        f.render_widget(paragraph, popup_area);
    }
}

fn draw_file_tree(f: &mut Frame, app: &App, dv: &DiffView, area: Rect) {
    let items: Vec<ListItem> = dv
        .tree
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let indent = "  ".repeat(node.depth);
            let is_selected = node.file_index == Some(dv.selected_file);
            let style = if is_selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if node.is_dir {
                Style::default().fg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };
            let is_cursor = i == app.tree_index;
            let prefix = if is_cursor { "▸ " } else { "  " };
            let icon = if node.is_dir { "\u{f413} " } else { "\u{f4a5} " };

            // Check for comments on this file
            let mut comment_indicator = Span::raw("");
            if let Some(fi) = node.file_index {
                if let Some(file) = dv.files.get(fi) {
                    let has_unresolved = dv.threads.iter().any(|t| t.path == file.path && !t.is_resolved);
                    let has_resolved_only = !has_unresolved && dv.threads.iter().any(|t| t.path == file.path);
                    let has_claude_pending = dv.claude_comments.iter().any(|c| c.file == file.path && c.accepted.is_none());
                    let has_drafts = dv.draft_comments.iter().any(|d| d.file == file.path);
                    let has_claude_accepted = dv.claude_comments.iter().any(|c| c.file == file.path && c.accepted == Some(true));

                    if has_unresolved {
                        comment_indicator = Span::styled(" \u{f075}", Style::default().fg(Color::Yellow));
                    } else if has_drafts || has_claude_accepted {
                        comment_indicator = Span::styled(" \u{f040}", Style::default().fg(Color::Green)); // pencil — has drafts
                    } else if has_claude_pending {
                        comment_indicator = Span::styled(" \u{f12a}", Style::default().fg(Color::Magenta));
                    } else if has_resolved_only {
                        comment_indicator = Span::styled(" \u{f075}", Style::default().fg(Color::DarkGray));
                    }
                }
            }

            let item = ListItem::new(Line::from(vec![
                Span::raw(prefix),
                Span::styled(format!("{}{}{}", indent, icon, node.display), style),
                comment_indicator,
            ]));
            if is_cursor {
                item.style(Style::default().bg(Color::Rgb(30, 30, 50)))
            } else {
                item
            }
        })
        .collect();

    let border_color = if app.diff_focus == crate::app::DiffFocus::Files {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Files ({}) ", dv.files.len()))
            .border_style(Style::default().fg(border_color)),
    );
    let mut state = ListState::default().with_selected(Some(app.tree_index));
    f.render_stateful_widget(list, area, &mut state);
    let mut hit = app.hit.borrow_mut();
    hit.tree = area;
    hit.tree_offset = state.offset();
}

fn draw_diff_content(f: &mut Frame, app: &App, dv: &DiffView, area: Rect) {
    let content_border = if app.diff_focus == crate::app::DiffFocus::Content {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let Some(file) = dv.current_file() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Diff ")
            .border_style(Style::default().fg(content_border));
        f.render_widget(Paragraph::new(" No file selected").block(block), area);
        app.hit.borrow_mut().content = area;
        return;
    };
    app.hit.borrow_mut().content = area;

    let inner_width = area.width.saturating_sub(2) as usize; // inside borders
    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = dv.scroll; // adjusted by adjust_scroll() before draw

    let mut lines: Vec<Line> = Vec::new();

    // File-level comments (no line or line not in diff)
    if scroll == 0 && !dv.file_level_threads.is_empty() {
        lines.push(Line::from(Span::styled(
            "  \u{f075} Comments on file",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
        // prefix: "  │ " = 4 chars; continuation: "  │   " = 6 chars
        let cont_w = inner_width.saturating_sub(6);
        for &ti in &dv.file_level_threads {
            if let Some(thread) = dv.threads.get(ti) {
                let thread_bg = if thread.is_resolved {
                    Color::Rgb(30, 30, 30)
                } else {
                    Color::Rgb(60, 45, 10)
                };
                let resolved_style = if thread.is_resolved {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let resolve_icon = if thread.is_resolved { " \u{f00c}" } else { "" };
                let line_info = thread.line.map(|l| format!(" line {}", l)).unwrap_or_default();
                lines.push(padded_line(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("┌─ \u{f075} Thread{} ", line_info),
                        resolved_style,
                    ),
                    Span::styled(resolve_icon, Style::default().fg(Color::Green)),
                ], inner_width, thread_bg));
                for comment in &thread.comments {
                    let c_style = if thread.is_resolved {
                        Style::default().fg(Color::Rgb(120, 120, 120))
                    } else {
                        Style::default().fg(Color::Rgb(200, 190, 170))
                    };
                    let author_style = Style::default().fg(Color::Cyan);
                    let is_me = comment.author == app.username;
                    let author_prefix = if is_me {
                        format!("{} (you): ", comment.author)
                    } else {
                        format!("{}: ", comment.author)
                    };
                    let first_w = inner_width.saturating_sub(4 + author_prefix.len());
                    let wrapped = wrap_text_2(&html::to_text(&comment.body), first_w, cont_w);
                    lines.push(padded_line(vec![
                        Span::raw("  "),
                        Span::styled("│ ", resolved_style),
                        Span::styled(author_prefix, author_style),
                        Span::styled(wrapped[0].clone(), c_style),
                    ], inner_width, thread_bg));
                    for wl in &wrapped[1..] {
                        lines.push(padded_line(vec![
                            Span::raw("  "),
                            Span::styled("│   ", resolved_style),
                            Span::styled(wl.clone(), c_style),
                        ], inner_width, thread_bg));
                    }
                }
                lines.push(padded_line(vec![
                    Span::raw("  "),
                    Span::styled("└─", resolved_style),
                ], inner_width, thread_bg));
            }
        }
        lines.push(Line::from(""));
    }

    let ai_label = format!("\u{f12a} {}", app.config.ai.name);
    let severity_color = |s: &str| -> Color {
        match s.to_uppercase().as_str() {
            "CRITICAL" => Color::Red,
            "HIGH" => Color::Rgb(255, 100, 50),
            "MEDIUM" => Color::Yellow,
            "LOW" => Color::Rgb(100, 180, 100),
            "INFO" => Color::Cyan,
            _ => Color::DarkGray,
        }
    };

    // File-level AI comments (line not in diff)
    if scroll == 0 && !dv.file_level_claude.is_empty() {
        let ai_file_label = format!("  \u{f12a} {} comments (file-level)", app.config.ai.name);
        lines.push(Line::from(Span::styled(
            ai_file_label,
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )));
        let comment_bg = Color::Rgb(20, 15, 30);
        let cl_wrap_w = inner_width.saturating_sub(6); // "  │ " = 4, padding
        for &ci in &dv.file_level_claude {
            if let Some(cc) = dv.claude_comments.get(ci) {
                let (label, frame_style) = match cc.accepted {
                    None => (ai_label.as_str(), Style::default().fg(Color::Magenta)),
                    Some(true) => ("\u{f00c} Accepted", Style::default().fg(Color::Green)),
                    Some(false) => ("\u{f00d} Discarded", Style::default().fg(Color::DarkGray)),
                };
                let body_style = match cc.accepted {
                    Some(false) => Style::default().fg(Color::DarkGray),
                    _ => Style::default().fg(Color::Rgb(180, 180, 200)),
                };
                let line_hint = if cc.line > 0 { format!(" (line {})", cc.line) } else { String::new() };
                let mut header_spans = vec![
                    Span::raw("  "),
                    Span::styled(format!("┌─ {}{} ", label, line_hint), frame_style),
                ];
                if let Some(sev) = &cc.severity {
                    header_spans.push(Span::styled(format!("— {} ", sev), Style::default().fg(severity_color(sev))));
                }
                lines.push(padded_line(header_spans, inner_width, comment_bg));
                let wrapped = wrap_text_2(&cc.body, cl_wrap_w, cl_wrap_w);
                for wl in &wrapped {
                    lines.push(padded_line(vec![
                        Span::raw("  "),
                        Span::styled("│ ", frame_style),
                        Span::styled(wl.clone(), body_style),
                    ], inner_width, comment_bg));
                }
                lines.push(padded_line(vec![
                    Span::raw("  "),
                    Span::styled("└─", frame_style),
                ], inner_width, comment_bg));
            }
        }
        lines.push(Line::from(""));
    }

    // Your own comments whose line has dropped out of the diff — shown here
    // rather than silently hidden.
    if scroll == 0 && !dv.file_level_drafts.is_empty() {
        lines.push(Line::from(Span::styled(
            "  \u{f040} Your comments (line not in this diff)",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )));
        let draft_bg = Color::Rgb(15, 25, 15);
        let dw = inner_width.saturating_sub(6);
        for &di in &dv.file_level_drafts {
            if let Some(draft) = dv.draft_comments.get(di) {
                lines.push(padded_line(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("┌─ \u{f040} Draft (line {}) ", draft.line),
                        Style::default().fg(Color::Green),
                    ),
                ], inner_width, draft_bg));
                for wl in &wrap_text_2(&draft.body, dw, dw) {
                    lines.push(padded_line(vec![
                        Span::raw("  "),
                        Span::styled("│ ", Style::default().fg(Color::Green)),
                        Span::styled(wl.clone(), Style::default().fg(Color::Rgb(180, 200, 180))),
                    ], inner_width, draft_bg));
                }
                lines.push(padded_line(vec![
                    Span::raw("  "),
                    Span::styled("└─", Style::default().fg(Color::Green)),
                ], inner_width, draft_bg));
            }
        }
        lines.push(Line::from(""));
    }

    for (li, dl) in file.lines.iter().enumerate().skip(scroll).take(inner_height + 50) {
        let is_cursor = li == dv.cursor_line;
        let has_thread = dv.line_threads.contains_key(&li);
        let has_claude = dv.line_claude.contains_key(&li);
        let has_draft = !dv.drafts_to_render_at(li).is_empty();

        // Line number
        let line_num = match (dl.old_line, dl.new_line) {
            (Some(o), Some(n)) => format!("{:>4} {:>4} ", o, n),
            (Some(o), None) => format!("{:>4}      ", o),
            (None, Some(n)) => format!("     {:>4} ", n),
            _ => "          ".to_string(),
        };

        let (line_num_color, sign_color, content_color, line_bg) = match dl.kind {
            LineKind::Added => (Color::Rgb(80, 160, 80), Color::Rgb(80, 160, 80), Color::White, Color::Rgb(20, 60, 20)),
            LineKind::Removed => (Color::Rgb(180, 80, 80), Color::Rgb(180, 80, 80), Color::White, Color::Rgb(70, 15, 15)),
            LineKind::Hunk => (Color::DarkGray, Color::Cyan, Color::Cyan, Color::Reset),
            LineKind::Meta => (Color::DarkGray, Color::DarkGray, Color::DarkGray, Color::Reset),
            LineKind::Context => (Color::DarkGray, Color::White, Color::White, Color::Reset),
        };

        let bg = if is_cursor {
            Color::Rgb(45, 45, 70)
        } else {
            line_bg
        };

        // Comment indicators in gutter
        let comment_indicator = if has_thread {
            Span::styled("\u{f075}", Style::default().fg(Color::Yellow).bg(bg))
        } else if has_claude {
            Span::styled("\u{f12a}", Style::default().fg(Color::Magenta).bg(bg))
        } else if has_draft {
            Span::styled("\u{f040}", Style::default().fg(Color::Green).bg(bg))
        } else {
            Span::styled(" ", Style::default().bg(bg))
        };

        // Split +/- sign from content; place sign right after line numbers
        let content = &dl.content;
        let code_text = match dl.kind {
            LineKind::Added | LineKind::Removed => {
                if content.is_empty() { "" } else { &content[1..] }
            }
            LineKind::Context => {
                if content.starts_with(' ') { &content[1..] } else { content.as_str() }
            }
            _ => content.as_str(),
        };

        // Look up syntax-highlighted spans for this line by index
        let hl_spans = dv.highlight_cache
            .get(&dv.selected_file)
            .and_then(|hf| hf.get_spans(li));

        let mut spans = Vec::new();
        if is_cursor {
            spans.push(Span::styled("▸", Style::default().fg(Color::Yellow).bg(bg)));
            spans.push(Span::styled(line_num[1..].to_string(), Style::default().fg(line_num_color).bg(bg)));
        } else {
            spans.push(Span::styled(line_num, Style::default().fg(line_num_color).bg(bg)));
        }
        if (dl.kind == LineKind::Added || dl.kind == LineKind::Removed) && !content.is_empty() {
            let (sign, _) = content.split_at(1);
            spans.push(Span::styled(sign, Style::default().fg(sign_color).bg(bg)));
            spans.push(comment_indicator);
            spans.push(Span::styled("  ", Style::default().bg(bg)));
        } else {
            spans.push(Span::styled(" ", Style::default().bg(bg)));
            spans.push(comment_indicator);
            spans.push(Span::styled("  ", Style::default().bg(bg)));
        }

        // Use syntax highlighting if available, otherwise fall back to plain color
        let use_syntax = matches!(dl.kind, LineKind::Added | LineKind::Removed | LineKind::Context);
        if use_syntax {
            if let Some(hl) = hl_spans {
                for (color, text) in hl {
                    spans.push(Span::styled(text.as_str(), Style::default().fg(*color).bg(bg)));
                }
            } else {
                spans.push(Span::styled(code_text, Style::default().fg(content_color).bg(bg)));
            }
        } else {
            spans.push(Span::styled(content.as_str(), Style::default().fg(content_color).bg(bg)));
        }

        // Pad line to fill full width with background
        let used: usize = spans.iter().map(|s| s.content.len()).sum();
        if used < inner_width {
            spans.push(Span::styled(
                " ".repeat(inner_width - used),
                Style::default().bg(bg),
            ));
        }
        if lines.len() < inner_height {
            app.hit.borrow_mut().code_rows.push((area.y + 1 + lines.len() as u16, li));
        }
        lines.push(Line::from(spans));

        // Render inline comments after the line (skip file-level ones, shown at top)
        if let Some(thread_indices) = dv.line_threads.get(&li) {
            // prefix: "          │ " = 12 chars; continuation: "          │   " = 14 chars
            let cont_w = inner_width.saturating_sub(14);
            for &ti in thread_indices {
                if dv.file_level_threads.contains(&ti) {
                    continue;
                }
                if let Some(thread) = dv.threads.get(ti) {
                    let thread_bg = if thread.is_resolved {
                        Color::Rgb(30, 30, 30)
                    } else {
                        Color::Rgb(60, 45, 10)
                    };
                    let resolved_style = if thread.is_resolved {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::Yellow)
                    };
                    let pending_resolve = dv.pending_resolves.contains(&ti);
                    let resolve_label = if thread.is_resolved {
                        " \u{f00c}".to_string()
                    } else if pending_resolve {
                        " (will resolve)".to_string()
                    } else {
                        String::new()
                    };
                    let mut header_spans = vec![
                        Span::raw("          "),
                        Span::styled(format!("┌─ \u{f075} Thread{} ", resolve_label), resolved_style),
                    ];
                    if pending_resolve {
                        header_spans.push(Span::styled("\u{f00c}", Style::default().fg(Color::Yellow)));
                    }
                    lines.push(padded_line(header_spans, inner_width, thread_bg));
                    for comment in &thread.comments {
                        let c_style = if thread.is_resolved {
                            Style::default().fg(Color::Rgb(120, 120, 120))
                        } else {
                            Style::default().fg(Color::Rgb(200, 190, 170))
                        };
                        let author_style = Style::default().fg(Color::Cyan);
                        let is_me = comment.author == app.username;
                        let author_prefix = if is_me {
                            format!("{} (you): ", comment.author)
                        } else {
                            format!("{}: ", comment.author)
                        };
                        let first_w = inner_width.saturating_sub(12 + author_prefix.len());
                        let wrapped = wrap_text_2(&html::to_text(&comment.body), first_w, cont_w);
                        // First line with author
                        lines.push(padded_line(vec![
                            Span::raw("          "),
                            Span::styled("│ ", resolved_style),
                            Span::styled(author_prefix, author_style),
                            Span::styled(wrapped[0].clone(), c_style),
                        ], inner_width, thread_bg));
                        // Continuation lines
                        for wl in &wrapped[1..] {
                            lines.push(padded_line(vec![
                                Span::raw("          "),
                                Span::styled("│   ", resolved_style),
                                Span::styled(wl.clone(), c_style),
                            ], inner_width, thread_bg));
                        }
                    }
                    // Show draft replies
                    for draft in &dv.draft_comments {
                        if draft.in_reply_to_thread == Some(ti) {
                            let draft_label = if draft.resolve { "(draft+resolve) " } else { "(draft) " };
                            let first_w = inner_width.saturating_sub(12 + draft_label.len());
                            let draft_wrapped = wrap_text_2(&draft.body, first_w, cont_w);
                            lines.push(padded_line(vec![
                                Span::raw("          "),
                                Span::styled("│ ", resolved_style),
                                Span::styled(
                                    draft_label,
                                    Style::default().fg(Color::Rgb(180, 140, 60)).add_modifier(Modifier::ITALIC),
                                ),
                                Span::styled(
                                    draft_wrapped[0].clone(),
                                    Style::default().fg(Color::Rgb(200, 190, 170)),
                                ),
                            ], inner_width, thread_bg));
                            for wl in &draft_wrapped[1..] {
                                lines.push(padded_line(vec![
                                    Span::raw("          "),
                                    Span::styled("│   ", resolved_style),
                                    Span::styled(wl.clone(), Style::default().fg(Color::Rgb(200, 190, 170))),
                                ], inner_width, thread_bg));
                            }
                        }
                    }
                    lines.push(padded_line(vec![
                        Span::raw("          "),
                        Span::styled("└─", resolved_style),
                    ], inner_width, thread_bg));
                }
            }
        }

        // Render Claude comments (framed like threads)
        if let Some(claude_indices) = dv.line_claude.get(&li) {
            let cl_wrap = inner_width.saturating_sub(14); // "          │ " = 12 + pad
            for &ci in claude_indices {
                if let Some(cc) = dv.claude_comments.get(ci) {
                    let (label, frame_style) = match cc.accepted {
                        None => (ai_label.as_str(), Style::default().fg(Color::Magenta)),
                        Some(true) => ("\u{f00c} Accepted", Style::default().fg(Color::Green)),
                        Some(false) => ("\u{f00d} Discarded", Style::default().fg(Color::DarkGray)),
                    };
                    let body_style = match cc.accepted {
                        Some(false) => Style::default().fg(Color::DarkGray),
                        _ => Style::default().fg(Color::Rgb(180, 180, 200)),
                    };
                    let comment_bg = Color::Rgb(20, 15, 30);
                    let mut header_spans = vec![
                        Span::raw("          "),
                        Span::styled(format!("┌─ {} ", label), frame_style),
                    ];
                    if let Some(sev) = &cc.severity {
                        header_spans.push(Span::styled(format!("— {} ", sev), Style::default().fg(severity_color(sev))));
                    }
                    lines.push(padded_line(header_spans, inner_width, comment_bg));
                    let wrapped = wrap_text_2(&cc.body, cl_wrap, cl_wrap);
                    for wl in &wrapped {
                        lines.push(padded_line(vec![
                            Span::raw("          "),
                            Span::styled("│ ", frame_style),
                            Span::styled(wl.clone(), body_style),
                        ], inner_width, comment_bg));
                    }
                    match cc.accepted {
                        None => {
                            lines.push(padded_line(vec![
                                Span::raw("          "),
                                Span::styled("└─", frame_style),
                                Span::styled(" a ", Style::default().fg(Color::Yellow)),
                                Span::styled("accept │ ", Style::default().fg(Color::DarkGray)),
                                Span::styled("e ", Style::default().fg(Color::Yellow)),
                                Span::styled("edit │ ", Style::default().fg(Color::DarkGray)),
                                Span::styled("d ", Style::default().fg(Color::Yellow)),
                                Span::styled("discard", Style::default().fg(Color::DarkGray)),
                            ], inner_width, comment_bg));
                        }
                        Some(false) => {
                            lines.push(padded_line(vec![
                                Span::raw("          "),
                                Span::styled("└─", frame_style),
                                Span::styled(" d ", Style::default().fg(Color::Yellow)),
                                Span::styled("bring back", Style::default().fg(Color::DarkGray)),
                            ], inner_width, comment_bg));
                        }
                        Some(true) => {
                            lines.push(padded_line(vec![
                                Span::raw("          "),
                                Span::styled("└─", frame_style),
                                Span::styled(" d ", Style::default().fg(Color::Yellow)),
                                Span::styled("discard", Style::default().fg(Color::DarkGray)),
                            ], inner_width, comment_bg));
                        }
                    }
                }
            }
        }

        // Your own unposted comments on this line
        for di in dv.drafts_to_render_at(li) {
            if let Some(draft) = dv.draft_comments.get(di) {
                let draft_bg = Color::Rgb(15, 25, 15);
                let dw = inner_width.saturating_sub(14);
                lines.push(padded_line(vec![
                    Span::raw("          "),
                    Span::styled("┌─ \u{f040} Draft ", Style::default().fg(Color::Green)),
                ], inner_width, draft_bg));
                let wrapped = wrap_text_2(&draft.body, dw, dw);
                for wl in &wrapped {
                    lines.push(padded_line(vec![
                        Span::raw("          "),
                        Span::styled("│ ", Style::default().fg(Color::Green)),
                        Span::styled(wl.clone(), Style::default().fg(Color::Rgb(180, 200, 180))),
                    ], inner_width, draft_bg));
                }
                lines.push(padded_line(vec![
                    Span::raw("          "),
                    Span::styled("└─", Style::default().fg(Color::Green)),
                    Span::styled(" e ", Style::default().fg(Color::Yellow)),
                    Span::styled("edit │ ", Style::default().fg(Color::DarkGray)),
                    Span::styled("d ", Style::default().fg(Color::Yellow)),
                    Span::styled("remove", Style::default().fg(Color::DarkGray)),
                ], inner_width, draft_bg));
            }
        }
    }

    let title = format!(" {} ", file.path);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(content_border));

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn draw_input_overlay(f: &mut Frame, dv: &DiffView, diff_area: Rect) {
    let h = 6u16;
    let w = diff_area.width.saturating_sub(4).min(80);
    let x = diff_area.x + 2;
    let inner_top = diff_area.y + 1; // border
    let inner_bottom = diff_area.y + diff_area.height.saturating_sub(1);
    let y = if let Some(rendered_line) = dv.input_target_line {
        let target_y = inner_top + rendered_line as u16;
        // Place below the thread, but clamp within the pane
        if target_y + h < inner_bottom {
            target_y
        } else {
            // Not enough room below — place above
            inner_top + rendered_line.saturating_sub(h as usize + 1) as u16
        }
    } else {
        // Fallback: center
        diff_area.y + (diff_area.height.saturating_sub(h)) / 2
    };
    let y = y.max(inner_top).min(inner_bottom.saturating_sub(h));
    let popup_area = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup_area);

    let (title, is_reply, resolve) = match &dv.input_mode {
        Some(crate::diff_view::InputMode::NewComment { .. }) => (" New Comment ", false, false),
        Some(crate::diff_view::InputMode::Reply { resolve, .. }) => (" Reply ", true, *resolve),
        Some(crate::diff_view::InputMode::EditClaude { .. }) => (" Edit AI Comment ", false, false),
        Some(crate::diff_view::InputMode::EditDraft { .. }) => (" Edit Draft Comment ", false, false),
        None => (" Comment ", false, false),
    };

    let bottom_hint = if is_reply {
        if resolve {
            " Enter submit+resolve │ Ctrl+R toggle resolve │ Esc cancel "
        } else {
            " Enter submit │ Ctrl+R resolve │ Esc cancel "
        }
    } else {
        " Enter submit │ Esc cancel "
    };

    let border_color = if resolve { Color::Yellow } else { Color::Green };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(bottom_hint)
        .border_style(Style::default().fg(border_color));

    let mut text_lines: Vec<Line> = Vec::new();
    if resolve {
        text_lines.push(Line::from(Span::styled(
            " \u{f00c} Will resolve thread",
            Style::default().fg(Color::Yellow),
        )));
    }
    text_lines.push(Line::from(format!(" ▎{}", dv.input_buffer)));

    let resolve_lines = if resolve { 1u16 } else { 0 };
    let paragraph = Paragraph::new(text_lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, popup_area);

    // Place terminal cursor at the input cursor offset
    let cursor_offset = dv.input_cursor.min(dv.input_buffer.len()) as u16;
    let cursor_x = popup_area.x + 1 + 2 + cursor_offset; // border + " ▎" + text up to cursor
    let cursor_y = popup_area.y + 1 + resolve_lines; // border + resolve line if present
    let cursor_x = cursor_x.min(popup_area.x + popup_area.width.saturating_sub(2));
    f.set_cursor_position((cursor_x, cursor_y));
}

fn draw_commit_range_bar(f: &mut Frame, app: &App, area: Rect) {
    let commits = &app.pr_commits;
    if commits.is_empty() { return; }
    let (start, end) = app.commit_range.unwrap_or((0, commits.len() - 1));
    let total = commits.len();

    let dim = Style::default().fg(Color::DarkGray);
    let marker_active = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let marker_inactive = Style::default().fg(Color::Rgb(100, 140, 180));

    // Slider visual: one char per commit, up to a cap
    let max_slots = (area.width.saturating_sub(40) as usize).min(40).max(5);
    let slots = total.min(max_slots);
    let scale = |i: usize| -> usize {
        if total <= 1 { 0 } else { i * (slots.saturating_sub(1)) / (total - 1) }
    };
    let start_slot = scale(start);
    let end_slot = scale(end);

    let mut slider_spans = Vec::new();
    for i in 0..slots {
        let in_range = i >= start_slot && i <= end_slot;
        let ch = if in_range { "●" } else { "·" };
        let style = if in_range { marker_active } else { marker_inactive };
        slider_spans.push(Span::styled(format!("{} ", ch), style));
    }

    let full_range = start == 0 && end == total - 1;
    let range_text = if full_range {
        format!("All commits ({})", total)
    } else if start == end {
        format!("Commit {}/{}", start + 1, total)
    } else {
        format!("Commits {}..{}/{}", start + 1, end + 1, total)
    };

    // Short message title from the start or end commit
    let end_msg: String = commits.get(end)
        .map(|c| c.commit.message.lines().next().unwrap_or("").to_string())
        .unwrap_or_default();
    let max_msg = (area.width as usize).saturating_sub(slots + range_text.len() + 20);
    let msg_trunc = if end_msg.len() > max_msg {
        format!("{}…", &end_msg.chars().take(max_msg.saturating_sub(1)).collect::<String>())
    } else {
        end_msg
    };

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(" ", dim));
    spans.extend(slider_spans);
    spans.push(Span::styled(format!("  {} ", range_text), dim));
    if !msg_trunc.is_empty() {
        spans.push(Span::styled(format!("│ {}", msg_trunc), Style::default().fg(Color::White)));
    }

    let bar = Paragraph::new(Line::from(spans));
    f.render_widget(bar, area);
}

fn draw_diff_view_status_bar(f: &mut Frame, dv: &DiffView, frame: usize, area: Rect) {
    let threads = dv.threads.iter().filter(|t| !t.is_resolved).count();
    let ai_pending = dv.claude_comments.iter().filter(|c| c.accepted.is_none()).count();
    let drafts = dv.draft_comments.len();

    if let Some(status) = &dv.submit_status {
        let color = if status.contains("failed") { Color::Red } else { Color::Green };
        let bar = Paragraph::new(Span::styled(
            format!(" {} │ press any key to continue", status),
            Style::default().fg(color),
        ));
        f.render_widget(bar, area);
        return;
    }

    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Rgb(180, 180, 200));
    let sep = Span::styled(" │ ", dim);

    let mut spans: Vec<Span> = Vec::new();

    // Always-visible keys
    spans.push(Span::styled(" ↑↓", key));
    spans.push(Span::styled(" scroll", dim));
    spans.push(sep.clone());
    spans.push(Span::styled("n/N", key));
    spans.push(Span::styled(" comments", dim));
    spans.push(sep.clone());
    spans.push(Span::styled("<>", key));
    spans.push(Span::styled(" resize", dim));
    spans.push(sep.clone());
    spans.push(Span::styled("[]", key));
    spans.push(Span::styled(" start", dim));
    spans.push(sep.clone());
    spans.push(Span::styled("{}", key));
    spans.push(Span::styled(" end", dim));
    spans.push(sep.clone());
    spans.push(Span::styled("a", key));
    spans.push(Span::styled(" add", dim));

    // Context-dependent: only show when applicable
    let on_thread = dv.has_thread_at_cursor();
    let on_unresolved = dv.has_unresolved_thread_at_cursor();

    if on_thread {
        spans.push(sep.clone());
        spans.push(Span::styled("r", key));
        spans.push(Span::styled(" reply", dim));
    }
    if on_unresolved {
        spans.push(sep.clone());
        spans.push(Span::styled("R", key));
        spans.push(Span::styled(" resolve", dim));
    }
    // Key hints come from the same classification the keys themselves use.
    let target = dv.cursor_target();
    let hint = |k: &'static str, label: &'static str, spans: &mut Vec<Span>| {
        spans.push(sep.clone());
        spans.push(Span::styled(k, key));
        spans.push(Span::styled(label, dim));
    };
    match target {
        CursorTarget::PendingAi => {
            hint("a", " accept", &mut spans);
            hint("e", " edit", &mut spans);
            hint("d", " discard", &mut spans);
        }
        CursorTarget::AcceptedAi => {
            hint("e", " reword", &mut spans);
            hint("d", " discard", &mut spans);
        }
        CursorTarget::DiscardedAi => {
            hint("d", " bring back", &mut spans);
        }
        CursorTarget::OwnDraft => {
            hint("e", " edit draft", &mut spans);
            hint("d", " remove draft", &mut spans);
        }
        CursorTarget::None => {}
    }
    if on_thread || target != CursorTarget::None {
        spans.push(sep.clone());
        spans.push(Span::styled("y", key));
        spans.push(Span::styled(" copy", dim));
    }

    spans.push(sep.clone());
    spans.push(Span::styled("c", key));
    spans.push(Span::styled(" ai", dim));

    // Submit only when there are pending drafts/resolves
    let resolves = dv.pending_resolves.len();
    let has_pending = drafts > 0 || resolves > 0;
    if has_pending {
        spans.push(sep.clone());
        spans.push(Span::styled("S", key));
        spans.push(Span::styled(" submit", dim));
    }

    spans.push(sep.clone());
    spans.push(Span::styled("q", key));
    spans.push(Span::styled(" back", dim));

    if dv.loading_review {
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("{} AI reviewing…", spinner(frame)),
            Style::default().fg(Color::Yellow),
        ));
    }

    // Stats
    let resolve_count = if resolves > 0 { format!(" resolve:{}", resolves) } else { String::new() };
    spans.push(Span::styled(
        format!(" │ threads:{} ai:{} drafts:{}{}",
            threads, ai_pending, drafts, resolve_count),
        dim,
    ));

    let bar = Paragraph::new(Line::from(spans));
    f.render_widget(bar, area);
}
