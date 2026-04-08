mod app;
mod diff_view;
mod github;
mod highlight;
mod ui;

use anyhow::{Context, Result};
use app::App;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use github::GithubClient;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .or_else(|_| {
            // Fallback: use `gh auth token`
            std::process::Command::new("gh")
                .args(["auth", "token"])
                .output()
                .map_err(|_| std::env::VarError::NotPresent)
                .and_then(|out| {
                    if out.status.success() {
                        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if t.is_empty() {
                            Err(std::env::VarError::NotPresent)
                        } else {
                            Ok(t)
                        }
                    } else {
                        Err(std::env::VarError::NotPresent)
                    }
                })
        })
        .context(
            "No GitHub token found. Set GITHUB_TOKEN, GH_TOKEN, or install gh CLI and run `gh auth login`.",
        )?;

    let client = GithubClient::new(token)?;
    let mut app = App::new(client);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    app.start_loading();

    let result = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }

    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        app.process_bg_messages();

        terminal.draw(|f| ui::draw(f, &mut *app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    return Ok(());
                }

                // --- Modal layers (highest priority first) ---

                // Help popup
                if app.show_help {
                    app.show_help = false;
                    continue;
                }

                // Approve popup
                if app.approve_popup.is_some() {
                    let has_result = app.approve_popup.as_ref().map_or(false, |p| p.result_msg.is_some());
                    if has_result {
                        // Any key closes after result
                        app.approve_popup = None;
                    } else {
                        match key.code {
                            KeyCode::Enter => app.submit_approve(),
                            KeyCode::Esc | KeyCode::Char('q') => { app.approve_popup = None; }
                            KeyCode::Backspace => {
                                if let Some(p) = &mut app.approve_popup { p.comment.pop(); }
                            }
                            KeyCode::Char(c) => {
                                if let Some(p) = &mut app.approve_popup {
                                    if !p.submitting { p.comment.push(c); }
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }

                // --- Diff view mode ---
                if let Some(dv) = &mut app.diff_view {
                    // Clear submit status on any key
                    dv.submit_status = None;

                    // Review output popup — captures keys while visible
                    if dv.loading_review || !dv.review_output.is_empty() {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                dv.review_output.clear();
                                if dv.loading_review {
                                    // Can't cancel the process, but hide the popup
                                    // Output will still be processed in background
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                dv.review_scroll = dv.review_scroll.saturating_sub(3);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                dv.review_scroll = dv.review_scroll.saturating_add(3);
                            }
                            _ => {
                                if !dv.loading_review {
                                    // Review done, any other key closes
                                    dv.review_output.clear();
                                }
                            }
                        }
                        continue;
                    }

                    // Input mode (typing a comment)
                    if dv.input_mode.is_some() {
                        match key.code {
                            KeyCode::Esc => dv.cancel_input(),
                            KeyCode::Enter => dv.submit_input(),
                            KeyCode::Backspace => { dv.input_buffer.pop(); }
                            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                dv.toggle_resolve();
                            }
                            KeyCode::Char(c) => dv.input_buffer.push(c),
                            _ => {}
                        }
                        continue;
                    }

                    let focus = app.diff_focus;
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            app.diff_view = None;
                            app.active_tab = app::Tab::Overview;
                        }
                        KeyCode::Tab => {
                            app.diff_focus = match app.diff_focus {
                                app::DiffFocus::Files => app::DiffFocus::Content,
                                app::DiffFocus::Content => app::DiffFocus::Files,
                            };
                        }
                        // Navigation — depends on focus
                        KeyCode::Up | KeyCode::Char('k') => {
                            if focus == app::DiffFocus::Files {
                                // Skip directories, find prev file
                                let mut idx = app.tree_index;
                                loop {
                                    if idx == 0 { break; }
                                    idx -= 1;
                                    if !dv.tree[idx].is_dir {
                                        app.tree_index = idx;
                                        dv.tree_select(idx);
                                        break;
                                    }
                                }
                            } else {
                                dv.scroll_up();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if focus == app::DiffFocus::Files {
                                // Skip directories, find next file
                                let max = dv.tree.len().saturating_sub(1);
                                let mut idx = app.tree_index;
                                loop {
                                    if idx >= max { break; }
                                    idx += 1;
                                    if !dv.tree[idx].is_dir {
                                        app.tree_index = idx;
                                        dv.tree_select(idx);
                                        break;
                                    }
                                }
                            } else {
                                dv.scroll_down();
                            }
                        }
                        KeyCode::Enter => {
                            if focus == app::DiffFocus::Files {
                                app.diff_focus = app::DiffFocus::Content;
                            }
                        }
                        // Ctrl+D / Ctrl+U — page scroll
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            dv.page_down(20);
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            dv.page_up(20);
                        }
                        // { / } — resize panes
                        KeyCode::Char('{') => {
                            app.file_pane_width = app.file_pane_width.saturating_sub(3).max(15);
                        }
                        KeyCode::Char('}') => {
                            app.file_pane_width = (app.file_pane_width + 3).min(80);
                        }
                        // Comment navigation — works in both panes
                        KeyCode::Char('n') => {
                            if let Some(ti) = dv.jump_next_comment_or_file() {
                                app.tree_index = ti;
                            }
                        }
                        KeyCode::Char('N') => {
                            if let Some(ti) = dv.jump_prev_comment_or_file() {
                                app.tree_index = ti;
                            }
                        }
                        KeyCode::Char('a') => dv.start_new_comment(),
                        KeyCode::Char('r') => dv.start_reply(),
                        KeyCode::Char('y') => dv.accept_claude_at_cursor(),
                        KeyCode::Char('x') => dv.discard_claude_at_cursor(),
                        KeyCode::Char('e') => dv.edit_claude_at_cursor(),
                        KeyCode::Char('c') => {
                            // Run structured Claude review into diff view
                            if let (Some(repo), Some(pr)) = (app.selected_repo_name(), app.selected_pr().cloned()) {
                                if let Some(dv) = &mut app.diff_view {
                                    dv.loading_review = true;
                                }
                                app.run_structured_review_bg(&repo, &pr);
                            }
                        }
                        KeyCode::Char('S') => {
                            app.submit_drafts();
                        }
                        KeyCode::Char('R') => {
                            app.resolve_thread_at_cursor();
                        }
                        KeyCode::Char('?') => { app.show_help = true; }
                        _ => {}
                    }
                    continue;
                }

                // --- Normal mode (PR list) ---
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('?') => { app.show_help = true; }
                    KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                    KeyCode::Tab | KeyCode::BackTab => app.next_panel(),
                    KeyCode::Enter => {
                        app.open_diff_view(false);
                    }
                    KeyCode::Char('A') => app.show_approve_popup(),
                    KeyCode::Char('a') => app.toggle_assigned(),
                    KeyCode::Char('r') => app.refresh(),
                    KeyCode::Char('o') => {
                        if let Some(pr) = app.selected_pr() {
                            let url = pr.html_url.clone();
                            let _ = std::process::Command::new("open")
                                .arg(&url)
                                .spawn()
                                .or_else(|_| {
                                    std::process::Command::new("xdg-open").arg(&url).spawn()
                                });
                        }
                    }
                    _ => {}
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
