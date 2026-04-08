use crate::app::{ci_icon, App, Panel};
use crate::diff_view::{DiffView, LineKind};
use crate::github::CiState;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

const NERD_PR: &str = "\u{f407}";
const NERD_DRAFT: &str = "\u{f444}";
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
                let break_at = remaining[..cur_w]
                    .rfind(' ')
                    .unwrap_or(cur_w);
                let break_at = break_at.max(1); // never produce empty chunk
                wrapped.push(remaining[..break_at].to_string());
                remaining = remaining[break_at..].trim_start();
                cur_w = rw; // after first chunk, use rest_width
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
        draw_diff_view_status_bar(f, dv, chunks[1]);
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

    if app.show_help {
        draw_help_popup(f, size);
    }
}


fn draw_help_popup(f: &mut Frame, area: Rect) {
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 28u16.min(area.height.saturating_sub(4));
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
        h("\u{f407}", Color::Green, "Open pull request"),
        h("\u{f444}", Color::DarkGray, "Draft pull request"),
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
        h("\u{f06a}", Color::Red, "Changes requested / conflict"),
        h("\u{f075}", Color::Yellow, "Has review comments"),
        h("\u{f407}", Color::Green, "Ready to merge"),
        Line::from(""),
        s("Keys"),
        h("↑↓/jk", Color::DarkGray, "Navigate / scroll"),
        h("Tab", Color::DarkGray, "Switch panel"),
        h("a", Color::DarkGray, "Toggle assigned / all"),
        h("c", Color::DarkGray, "Claude review"),
        h("r", Color::DarkGray, "Refresh"),
        h("o", Color::DarkGray, "Open in browser"),
        h("Enter", Color::DarkGray, "Load diff"),
        h("q", Color::DarkGray, "Quit"),
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
    if app.loading {
        let loading = Paragraph::new(format!(" {} Loading pull requests...", NERD_REFRESH))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} Pull Requests ", NERD_PR))
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

    let items: Vec<ListItem> = app
        .flat_prs
        .iter()
        .enumerate()
        .map(|(i, fpr)| {
            let pr = &fpr.pr;
            let style = if i == app.pr_index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == app.pr_index { "▸ " } else { "  " };

            let pr_icon = if pr.draft { NERD_DRAFT } else { NERD_PR };
            let pr_icon_color = if pr.draft {
                Color::DarkGray
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

            // Merge status indicators
            match pr.mergeable_state.as_deref() {
                Some("dirty") | Some("blocked") => {
                    // Conflict or blocked
                    indicators.push(Span::styled(
                        "\u{f06a} ", // exclamation circle
                        Style::default().fg(Color::Red),
                    ));
                }
                _ => {
                    if pr.mergeable == Some(false) {
                        indicators.push(Span::styled(
                            "\u{f06a} ",
                            Style::default().fg(Color::Red),
                        ));
                    } else if pr.mergeable_state.as_deref() == Some("clean") && pr.mergeable == Some(true) {
                        indicators.push(Span::styled(
                            "\u{f00c} ", // check — ready to merge
                            Style::default().fg(Color::Rgb(0, 200, 100)),
                        ));
                    }
                }
            }

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
            if i == app.pr_index {
                item.style(Style::default().bg(Color::Rgb(25, 25, 40)))
            } else {
                item
            }
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " {} Pull Requests ({}) ",
                NERD_PR,
                app.flat_prs.len()
            ))
            .border_style(panel_border_style(app, Panel::PullRequests)),
    );
    let mut state = ListState::default().with_selected(Some(app.pr_index));
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_details_panel(f: &mut Frame, app: &App, area: Rect) {
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
    let pr_icon = if pr.draft { NERD_DRAFT } else { NERD_PR };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} #{} ", pr_icon, pr.number),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if pr.draft { "[DRAFT]" } else { "" },
                Style::default().fg(Color::DarkGray),
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
        let body_trimmed = body.trim();
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
                let body_trimmed = body.trim();
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
    let nav_help = format!(
        " ↑↓ navigate │ Tab panel │ a [{filter_label}] │ Enter diff │ A approve │ r refresh │ o open │ ? help │ q quit "
    );

    let help = Paragraph::new(Span::styled(
        &nav_help,
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(help, chunks[0]);

    let user_info = Paragraph::new(Span::styled(
        format!(" {} {} ", NERD_USER, app.username),
        Style::default().fg(Color::Cyan),
    ))
    .alignment(ratatui::layout::Alignment::Right);
    f.render_widget(user_info, chunks[1]);
}

// ── Confirm popup ──────────────────────────────────────────


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
}

// ── Diff view (two-pane) ───────────────────────────────────

fn draw_diff_view(f: &mut Frame, app: &App, dv: &DiffView, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(app.file_pane_width.min(area.width / 2)),
            Constraint::Min(0),
        ])
        .split(area);

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

        let title = if dv.loading_review {
            format!(" {} Claude Review ", NERD_REFRESH)
        } else {
            " Claude Review (done) ".to_string()
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
        return;
    };

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
                    let wrapped = wrap_text_2(&comment.body, first_w, cont_w);
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

    // File-level Claude comments (line not in diff)
    if scroll == 0 && !dv.file_level_claude.is_empty() {
        lines.push(Line::from(Span::styled(
            "  \u{f12a} Claude comments (file-level)",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )));
        let comment_bg = Color::Rgb(20, 15, 30);
        let cl_wrap_w = inner_width.saturating_sub(6); // "  │ " = 4, padding
        for &ci in &dv.file_level_claude {
            if let Some(cc) = dv.claude_comments.get(ci) {
                let (label, frame_style) = match cc.accepted {
                    None => ("\u{f12a} Claude", Style::default().fg(Color::Magenta)),
                    Some(true) => ("\u{f00c} Accepted", Style::default().fg(Color::Green)),
                    Some(false) => ("\u{f00d} Discarded", Style::default().fg(Color::DarkGray)),
                };
                let body_style = match cc.accepted {
                    Some(false) => Style::default().fg(Color::DarkGray),
                    _ => Style::default().fg(Color::Rgb(180, 180, 200)),
                };
                let line_hint = if cc.line > 0 { format!(" (line {})", cc.line) } else { String::new() };
                lines.push(padded_line(vec![
                    Span::raw("  "),
                    Span::styled(format!("┌─ {}{} ", label, line_hint), frame_style),
                ], inner_width, comment_bg));
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

    for (li, dl) in file.lines.iter().enumerate().skip(scroll).take(inner_height + 50) {
        let is_cursor = li == dv.cursor_line;
        let has_thread = dv.line_threads.contains_key(&li);
        let has_claude = dv.line_claude.contains_key(&li);

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
                        let wrapped = wrap_text_2(&comment.body, first_w, cont_w);
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
                        None => ("\u{f12a} Claude", Style::default().fg(Color::Magenta)),
                        Some(true) => ("\u{f00c} Accepted", Style::default().fg(Color::Green)),
                        Some(false) => ("\u{f00d} Discarded", Style::default().fg(Color::DarkGray)),
                    };
                    let body_style = match cc.accepted {
                        Some(false) => Style::default().fg(Color::DarkGray),
                        _ => Style::default().fg(Color::Rgb(180, 180, 200)),
                    };
                    let comment_bg = Color::Rgb(20, 15, 30);
                    lines.push(padded_line(vec![
                        Span::raw("          "),
                        Span::styled(format!("┌─ {} ", label), frame_style),
                    ], inner_width, comment_bg));
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
                                Span::styled(" y ", Style::default().fg(Color::Yellow)),
                                Span::styled("accept │ ", Style::default().fg(Color::DarkGray)),
                                Span::styled("e ", Style::default().fg(Color::Yellow)),
                                Span::styled("edit │ ", Style::default().fg(Color::DarkGray)),
                                Span::styled("x ", Style::default().fg(Color::Yellow)),
                                Span::styled("discard", Style::default().fg(Color::DarkGray)),
                            ], inner_width, comment_bg));
                        }
                        _ => {
                            lines.push(padded_line(vec![
                                Span::raw("          "),
                                Span::styled("└─", frame_style),
                            ], inner_width, comment_bg));
                        }
                    }
                }
            }
        }

        // Draft new comments (not replies)
        for draft in &dv.draft_comments {
            if draft.in_reply_to_thread.is_none() && draft.file == file.path {
                let matches = dl.new_line == Some(draft.line)
                    || dl.old_line == Some(draft.line);
                if matches {
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
                    ], inner_width, draft_bg));
                }
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
        Some(crate::diff_view::InputMode::EditClaude { .. }) => (" Edit Claude Comment ", false, false),
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

    // Place terminal cursor at end of input text
    let cursor_x = popup_area.x + 1 + 2 + dv.input_buffer.len() as u16; // border + " ▎" + text
    let cursor_y = popup_area.y + 1 + resolve_lines; // border + resolve line if present
    let cursor_x = cursor_x.min(popup_area.x + popup_area.width.saturating_sub(2));
    f.set_cursor_position((cursor_x, cursor_y));
}

fn draw_diff_view_status_bar(f: &mut Frame, dv: &DiffView, area: Rect) {
    let loading = if dv.loading_review { " \u{f450} Claude reviewing… │" } else { "" };
    let threads = dv.threads.iter().filter(|t| !t.is_resolved).count();
    let claude_pending = dv.claude_comments.iter().filter(|c| c.accepted.is_none()).count();
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

    let resolves = dv.pending_resolves.len();
    let has_pending = drafts > 0 || resolves > 0;
    let submit_hint = if has_pending { " │ S submit" } else { "" };
    let resolve_count = if resolves > 0 { format!(" resolve:{}", resolves) } else { String::new() };
    let info = format!(
        " ↑↓ scroll │ n/N comments │ a add │ r reply │ R resolve │ c claude │ y/e/x accept/edit/discard{} │ q back {}│ threads:{} claude:{} drafts:{}{}",
        submit_hint, loading, threads, claude_pending, drafts, resolve_count
    );

    let bar = Paragraph::new(Span::styled(info, Style::default().fg(Color::DarkGray)));
    f.render_widget(bar, area);
}
