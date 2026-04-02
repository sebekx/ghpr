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

pub fn draw(f: &mut Frame, app: &App) {
    let size = f.area();

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

    if let Some(popup) = &app.review_popup {
        draw_review_popup(f, popup, size);
    }

    if let Some(popup) = &app.approve_popup {
        draw_approve_popup(f, popup, size);
    }

    if app.show_help {
        draw_help_popup(f, size);
    }
}

fn draw_review_popup(f: &mut Frame, popup: &crate::app::ReviewPopup, area: Rect) {
    let popup_width = (area.width as f32 * 0.8) as u16;
    let popup_height = (area.height as f32 * 0.8) as u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let title = if popup.loading {
        format!(" {} {} ", NERD_REFRESH, popup.title)
    } else {
        format!(" {} ", popup.title)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(" Esc close │ ↑↓ scroll ")
        .border_style(Style::default().fg(Color::Magenta));

    let lines: Vec<Line> = popup
        .content
        .lines()
        .map(|line| Line::from(Span::raw(line)))
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((popup.scroll, 0));

    f.render_widget(paragraph, popup_area);
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
        h("\u{f06a}", Color::Red, "Changes requested"),
        h("\u{f075}", Color::Yellow, "Has review comments"),
        Line::from(""),
        s("Keys"),
        h("↑↓/jk", Color::DarkGray, "Navigate / scroll"),
        h("Tab", Color::DarkGray, "Switch panel"),
        h("1/2", Color::DarkGray, "Overview / Diff tab"),
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
        " ↑↓ navigate │ Tab panel │ a [{filter_label}] │ Enter diff │ A approve │ c review │ r refresh │ o open │ ? help │ q quit "
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
        draw_input_overlay(f, dv, area);
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
                    let has_claude = dv.claude_comments.iter().any(|c| c.file == file.path && c.accepted.is_none());

                    if has_unresolved {
                        comment_indicator = Span::styled(" \u{f075}", Style::default().fg(Color::Yellow));
                    } else if has_claude {
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

    let inner_height = area.height.saturating_sub(2) as usize;
    // Auto-scroll to keep cursor visible
    let scroll = if dv.cursor_line >= dv.scroll + inner_height {
        dv.cursor_line.saturating_sub(inner_height - 1)
    } else if dv.cursor_line < dv.scroll {
        dv.cursor_line
    } else {
        dv.scroll
    };

    let mut lines: Vec<Line> = Vec::new();

    // File-level comments (no line or line not in diff)
    if scroll == 0 && !dv.file_level_threads.is_empty() {
        lines.push(Line::from(Span::styled(
            "  \u{f075} Comments on file",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
        for &ti in &dv.file_level_threads {
            if let Some(thread) = dv.threads.get(ti) {
                let resolved_style = if thread.is_resolved {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let resolve_icon = if thread.is_resolved { " \u{f00c}" } else { "" };
                let line_info = thread.line.map(|l| format!(" line {}", l)).unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  ┌─ \u{f075} Thread{} ", line_info),
                        resolved_style,
                    ),
                    Span::styled(
                        resolve_icon,
                        Style::default().fg(Color::Green),
                    ),
                ]));
                for comment in &thread.comments {
                    let c_style = if thread.is_resolved {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    lines.push(Line::from(vec![
                        Span::styled("  │ ", resolved_style),
                        Span::styled(
                            format!("{}: ", comment.author),
                            if thread.is_resolved { Style::default().fg(Color::DarkGray) } else { Style::default().fg(Color::Cyan) },
                        ),
                        Span::styled(
                            comment.body.lines().next().unwrap_or(""),
                            c_style,
                        ),
                    ]));
                    for extra in comment.body.lines().skip(1).take(4) {
                        lines.push(Line::from(vec![
                            Span::styled("  │   ", resolved_style),
                            Span::styled(extra, c_style),
                        ]));
                    }
                }
                lines.push(Line::from(Span::styled("  └─", resolved_style)));
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

        let line_style = match dl.kind {
            LineKind::Added => Style::default().fg(Color::Green),
            LineKind::Removed => Style::default().fg(Color::Red),
            LineKind::Hunk => Style::default().fg(Color::Cyan),
            LineKind::Meta => Style::default().fg(Color::DarkGray),
            LineKind::Context => Style::default().fg(Color::White),
        };

        let bg = if is_cursor {
            Color::Rgb(30, 30, 50)
        } else {
            Color::Reset
        };

        let mut spans = vec![
            Span::styled(line_num, Style::default().fg(Color::DarkGray).bg(bg)),
        ];

        // Comment indicators in gutter
        if has_thread {
            spans.push(Span::styled("\u{f075}", Style::default().fg(Color::Yellow).bg(bg)));
        } else if has_claude {
            spans.push(Span::styled("\u{f12a}", Style::default().fg(Color::Magenta).bg(bg)));
        } else {
            spans.push(Span::styled(" ", Style::default().bg(bg)));
        }
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        spans.push(Span::styled(&dl.content, line_style.bg(bg)));

        lines.push(Line::from(spans));

        // Render inline comments after the line (skip file-level ones, shown at top)
        if let Some(thread_indices) = dv.line_threads.get(&li) {
            for &ti in thread_indices {
                if dv.file_level_threads.contains(&ti) {
                    continue;
                }
                if let Some(thread) = dv.threads.get(ti) {
                    let resolved_style = if thread.is_resolved {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default().fg(Color::Yellow)
                    };
                    let resolve_icon = if thread.is_resolved { " \u{f00c}" } else { "" };
                    lines.push(Line::from(vec![
                        Span::raw("          "),
                        Span::styled("┌─ \u{f075} Thread ", resolved_style),
                        Span::styled(resolve_icon, Style::default().fg(Color::Green)),
                    ]));
                    for comment in &thread.comments {
                        let c_style = if thread.is_resolved {
                            Style::default().fg(Color::DarkGray)
                        } else {
                            Style::default().fg(Color::White)
                        };
                        lines.push(Line::from(vec![
                            Span::raw("          "),
                            Span::styled("│ ", resolved_style),
                            Span::styled(
                                format!("{}: ", comment.author),
                                if thread.is_resolved {
                                    Style::default().fg(Color::DarkGray)
                                } else {
                                    Style::default().fg(Color::Cyan)
                                },
                            ),
                            Span::styled(
                                comment.body.lines().next().unwrap_or(""),
                                c_style,
                            ),
                        ]));
                        // Show additional lines
                        for extra in comment.body.lines().skip(1).take(3) {
                            lines.push(Line::from(vec![
                                Span::raw("          "),
                                Span::styled("│   ", resolved_style),
                                Span::styled(extra, c_style),
                            ]));
                        }
                    }
                    // Show draft replies
                    for draft in &dv.draft_comments {
                        if draft.in_reply_to_thread == Some(ti) {
                            lines.push(Line::from(vec![
                                Span::raw("          "),
                                Span::styled("│ ", Style::default().fg(Color::Green)),
                                Span::styled(
                                    format!("(draft) {}", draft.body),
                                    Style::default().fg(Color::Green),
                                ),
                            ]));
                        }
                    }
                    lines.push(Line::from(vec![
                        Span::raw("          "),
                        Span::styled("└─", resolved_style),
                    ]));
                }
            }
        }

        // Render Claude comments
        if let Some(claude_indices) = dv.line_claude.get(&li) {
            for &ci in claude_indices {
                if let Some(cc) = dv.claude_comments.get(ci) {
                    let (label, style) = match cc.accepted {
                        None => ("\u{f12a} Claude", Style::default().fg(Color::Magenta)),
                        Some(true) => ("\u{f00c} Accepted", Style::default().fg(Color::Green)),
                        Some(false) => ("\u{f00d} Discarded", Style::default().fg(Color::DarkGray)),
                    };
                    lines.push(Line::from(vec![
                        Span::raw("          "),
                        Span::styled(format!("  {} ", label), style),
                    ]));
                    let body_style = match cc.accepted {
                        Some(false) => Style::default().fg(Color::DarkGray),
                        _ => Style::default().fg(Color::White),
                    };
                    for body_line in cc.body.lines().take(5) {
                        lines.push(Line::from(vec![
                            Span::raw("          "),
                            Span::styled(format!("  {}", body_line), body_style),
                        ]));
                    }
                }
            }
        }

        // Draft new comments (not replies)
        for draft in &dv.draft_comments {
            if draft.in_reply_to_thread.is_none() && draft.file == file.path {
                if let Some(new_ln) = dl.new_line {
                    if draft.line == new_ln {
                        lines.push(Line::from(vec![
                            Span::raw("          "),
                            Span::styled(
                                format!("  \u{f040} (draft) {}", draft.body),
                                Style::default().fg(Color::Green),
                            ),
                        ]));
                    }
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

fn draw_input_overlay(f: &mut Frame, dv: &DiffView, area: Rect) {
    let w = 60u16.min(area.width.saturating_sub(4));
    let h = 5u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = area.height.saturating_sub(h + 2);
    let popup_area = Rect::new(x, y, w, h);

    f.render_widget(Clear, popup_area);

    let title = match &dv.input_mode {
        Some(crate::diff_view::InputMode::NewComment { .. }) => " New Comment ",
        Some(crate::diff_view::InputMode::Reply { .. }) => " Reply ",
        None => " Comment ",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(" Enter submit │ Esc cancel ")
        .border_style(Style::default().fg(Color::Green));

    let text = format!("▎{}", dv.input_buffer);
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, popup_area);
}

fn draw_diff_view_status_bar(f: &mut Frame, dv: &DiffView, area: Rect) {
    let loading = if dv.loading_review { " \u{f450} Claude reviewing… │" } else { "" };
    let threads = dv.threads.iter().filter(|t| !t.is_resolved).count();
    let claude_pending = dv.claude_comments.iter().filter(|c| c.accepted.is_none()).count();
    let drafts = dv.draft_comments.len();

    let info = format!(
        " ↑↓ scroll │ Tab focus │ {{}}/{{}} resize │ n/N comments │ a add │ r reply │ y/x claude │ q back {}│ threads:{} claude:{} drafts:{}",
        loading, threads, claude_pending, drafts
    );

    let bar = Paragraph::new(Span::styled(info, Style::default().fg(Color::DarkGray)));
    f.render_widget(bar, area);
}
