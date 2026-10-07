mod app;
mod config;
mod diff_view;
mod draft;
mod github;
mod highlight;
mod html;
mod input;
mod mouse;
mod ui;

use anyhow::{Context, Result};
use app::App;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use github::GithubClient;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

struct Args {
    /// Background refresh interval in seconds. `None` = never refresh.
    refresh: Option<u64>,
    check_auto: bool,
}

const HELP: &str = "\
ghpr — GitHub pull request review TUI

USAGE:
    ghpr [OPTIONS]

OPTIONS:
    -r, --refresh <INTERVAL>  Check for new and updated PRs every INTERVAL and
                              run the AI review on them. Without this flag ghpr
                              never refreshes on its own and never starts a
                              review in the background — press r to reload the
                              list and c to review a PR.
                              Accepts 30s, 5m, 2h, or a plain number of seconds.
        --check-auto          Print which PRs would be auto-reviewed, then exit.
    -h, --help                Show this help.

Leaving --refresh off keeps ghpr idle: no polling, and no `claude` subprocesses
started behind your back. Worth doing on battery.
";

fn parse_duration(s: &str) -> Result<u64, String> {
    let text = s.trim();
    let invalid = || format!("invalid interval '{}' — try 30s, 5m or 2h", s);

    let mut chars = text.chars();
    let (digits, multiplier) = match chars.next_back() {
        Some('s') | Some('S') => (chars.as_str(), 1),
        Some('m') | Some('M') => (chars.as_str(), 60),
        Some('h') | Some('H') => (chars.as_str(), 3600),
        // No suffix — treat the whole thing as seconds.
        Some(c) if c.is_ascii_digit() => (text, 1),
        _ => return Err(invalid()),
    };

    let n: u64 = digits.trim().parse().map_err(|_| invalid())?;
    if n == 0 {
        return Err("interval must be greater than zero".to_string());
    }
    // Saturating here would turn an absurd value into a refresh that never
    // fires while still reporting itself as enabled. Reject it instead.
    n.checked_mul(multiplier)
        .ok_or_else(|| format!("interval '{}' is too large", s))
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args { refresh: None, check_auto: false };

    while let Some(arg) = args.next() {
        if arg == "--check-auto" {
            out.check_auto = true;
        } else if arg == "-h" || arg == "--help" {
            print!("{}", HELP);
            std::process::exit(0);
        } else if arg == "-r" || arg == "--refresh" {
            let value = args
                .next()
                .ok_or_else(|| "-r needs an interval, e.g. -r 5m".to_string())?;
            out.refresh = Some(parse_duration(&value)?);
        } else if let Some(v) = arg.strip_prefix("--refresh=") {
            out.refresh = Some(parse_duration(v)?);
        } else if let Some(v) = arg.strip_prefix("-r").filter(|v| !v.is_empty()) {
            out.refresh = Some(parse_duration(v)?);
        } else {
            return Err(format!("unknown argument '{}'", arg));
        }
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("ghpr: {}", e);
        eprintln!("try 'ghpr --help'");
        std::process::exit(2);
    });

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

    // Load or create config
    let mut cfg = match config::Config::load()? {
        Some(c) => c,
        None => {
            let path = config::Config::write_default()?;
            eprintln!("Created default config at: {}", path.display());
            eprintln!("Edit it to configure your AI review agent.\n");
            config::Config::default()
        }
    };

    // `-r` turns background refresh on and sets its pace. It wins over the
    // config so the decision is visible in the command you typed.
    if let Some(secs) = args.refresh {
        cfg.auto.enabled = true;
        cfg.auto.poll_interval_secs = secs;
    }

    let client = GithubClient::new(token)?;

    // Headless: report what the background reviewer would do, then exit.
    if args.check_auto {
        return app::dry_run_auto(client, cfg).await;
    }

    let mut app = App::new(client, cfg);

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
    // Repaint on input, on background activity, or while something is
    // animating — not on a fixed 20 Hz timer. An idle ghpr should cost
    // nothing, which matters on battery.
    let mut needs_redraw = true;

    loop {
        let had_messages = app.process_bg_messages();
        app.tick_auto();
        app.persist_draft_if_dirty();

        let animating = app.is_animating();
        if needs_redraw || had_messages || animating {
            terminal.draw(|f| ui::draw(f, &mut *app))?;
            needs_redraw = false;
        }

        // The draw just captured the selected text from the screen buffer.
        if let Some(text) = app.clipboard_out.take() {
            app.flash = Some(match mouse::copy_to_clipboard(&text) {
                Ok(()) => format!("Copied {} chars", text.chars().count()),
                Err(e) => format!("Copy failed: {}", e),
            });
            needs_redraw = true;
            continue;
        }

        // Tight enough for a smooth spinner while work is in flight; idle
        // otherwise, waking only to let tick_auto check the clock.
        let poll_ms = if animating { 100 } else { 1000 };

        if event::poll(Duration::from_millis(poll_ms))? {
            let ev = event::read()?;
            // Mouse movement alone is not worth a repaint.
            if matches!(ev, Event::Key(_) | Event::Resize(_, _)) {
                needs_redraw = true;
            }
            if let Event::Mouse(m) = ev {
                if mouse::handle(app, m) {
                    needs_redraw = true;
                }
            }
            if matches!(ev, Event::Key(_)) {
                app.selection = None;
                app.flash = None;
            }
            if let Event::Key(key) = ev {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    return Ok(());
                }

                // --- Modal layers (highest priority first) ---

                // Confirm quit popup
                if app.confirm_quit.is_some() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            match app.confirm_quit.take().unwrap() {
                                app::ConfirmQuit::App => return Ok(()),
                            }
                        }
                        _ => { app.confirm_quit = None; }
                    }
                    continue;
                }

                // Help popup
                if app.show_help {
                    app.show_help = false;
                    continue;
                }

                // Comment popup
                if app.comment_popup.is_some() {
                    let has_result = app.comment_popup.as_ref().map_or(false, |p| p.result_msg.is_some());
                    if has_result {
                        app.comment_popup = None;
                    } else {
                        match key.code {
                            KeyCode::Enter => app.submit_comment(),
                            KeyCode::Esc => { app.comment_popup = None; }
                            _ => {
                                if let Some(p) = &mut app.comment_popup {
                                    if !p.submitting {
                                        input::handle_text_key(&mut p.body, &mut p.cursor, &key);
                                    }
                                }
                            }
                        }
                    }
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
                            KeyCode::Esc => { app.approve_popup = None; }
                            _ => {
                                if let Some(p) = &mut app.approve_popup {
                                    if !p.submitting {
                                        input::handle_text_key(&mut p.comment, &mut p.cursor, &key);
                                    }
                                }
                            }
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
                            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                dv.toggle_resolve();
                            }
                            _ => {
                                input::handle_text_key(
                                    &mut dv.input_buffer,
                                    &mut dv.input_cursor,
                                    &key,
                                );
                            }
                        }
                        continue;
                    }

                    let focus = app.diff_focus;
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            // Draft state is on disk, so going back costs
                            // nothing and needs no confirmation.
                            app.persist_draft_if_dirty();
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
                        // < / > — resize panes
                        KeyCode::Char('<') => {
                            app.file_pane_width = app.file_pane_width.saturating_sub(3).max(15);
                        }
                        KeyCode::Char('>') => {
                            app.file_pane_width = (app.file_pane_width + 3).min(80);
                        }
                        // [ / ] — move commit range start (narrow / widen left edge)
                        KeyCode::Char('[') => app.move_range_start(-1),
                        KeyCode::Char(']') => app.move_range_start(1),
                        // { / } — move commit range end (widen / narrow right edge)
                        KeyCode::Char('{') => app.move_range_end(-1),
                        KeyCode::Char('}') => app.move_range_end(1),
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
                        KeyCode::Char('a') => {
                            // Accept AI comment if near one, otherwise start new comment
                            if dv.has_pending_ai_at_cursor() {
                                dv.accept_claude_at_cursor();
                            } else {
                                dv.start_new_comment();
                            }
                        }
                        KeyCode::Char('r') => dv.start_reply(),
                        KeyCode::Char('d') => dv.discard_at_cursor(),
                        KeyCode::Char('e') => dv.edit_at_cursor(),
                        KeyCode::Char('c') => {
                            // Run AI review into diff view
                            if let (Some(repo), Some(pr)) = (app.selected_repo_name(), app.selected_pr().cloned()) {
                                if let Some(dv) = &mut app.diff_view {
                                    dv.loading_review = true;
                                }
                                app.run_ai_review_bg(&repo, &pr);
                            }
                        }
                        KeyCode::Char('S') => {
                            app.submit_drafts();
                        }
                        KeyCode::Char('R') => {
                            app.resolve_thread_at_cursor();
                        }
                        KeyCode::Char('y') => {
                            dv.copy_at_cursor();
                        }
                        KeyCode::Char('?') => { app.show_help = true; }
                        _ => {}
                    }
                    continue;
                }

                // --- Search mode (PR filter) ---
                if app.search_mode {
                    match key.code {
                        KeyCode::Esc => {
                            app.search_mode = false;
                            app.search_query.clear();
                            app.apply_filter_public();
                        }
                        KeyCode::Enter => {
                            app.search_mode = false;
                            // Keep the filter active
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                            app.apply_filter_public();
                        }
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                            app.apply_filter_public();
                        }
                        _ => {}
                    }
                    continue;
                }

                // --- Normal mode (PR list) ---
                match key.code {
                    KeyCode::Char('q') => {
                        if app.has_pending_drafts() {
                            app.confirm_quit = Some(app::ConfirmQuit::App);
                        } else {
                            return Ok(());
                        }
                    }
                    KeyCode::Esc => {
                        if !app.search_query.is_empty() {
                            app.search_query.clear();
                            app.apply_filter_public();
                        }
                    }
                    KeyCode::Char('?') => { app.show_help = true; }
                    KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                    KeyCode::Tab | KeyCode::BackTab => app.next_panel(),
                    KeyCode::Enter => {
                        app.open_diff_view(false);
                    }
                    KeyCode::Char('A') => app.show_approve_popup(),
                    KeyCode::Char('a') => {
                        match app.active_panel {
                            app::Panel::Details => app.show_comment_popup(),
                            app::Panel::PullRequests => app.toggle_assigned(),
                        }
                    }
                    KeyCode::Char('/') => { app.search_mode = true; }
                    KeyCode::Char('c') => app.rerun_ai_review(),
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

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn intervals_accept_suffixes_and_bare_seconds() {
        assert_eq!(parse_duration("30s"), Ok(30));
        assert_eq!(parse_duration("5m"), Ok(300));
        assert_eq!(parse_duration("2h"), Ok(7200));
        assert_eq!(parse_duration("90"), Ok(90), "bare number means seconds");
        assert_eq!(parse_duration("5M"), Ok(300), "suffix is case-insensitive");
        assert_eq!(parse_duration(" 5m "), Ok(300), "padding is tolerated");
        assert_eq!(parse_duration("5 m"), Ok(300), "so is a space before the unit");
    }

    #[test]
    fn nonsense_intervals_are_rejected_not_guessed() {
        // Silently defaulting here would leave the user with a refresh rate
        // they did not ask for.
        for bad in ["", "m", "abc", "5x", "-1m", "1.5m", "🙂", "5mm"] {
            assert!(
                parse_duration(bad).is_err(),
                "'{}' should be rejected, got {:?}",
                bad,
                parse_duration(bad)
            );
        }
    }

    #[test]
    fn zero_is_rejected_so_it_cannot_busy_loop() {
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("0m").is_err());
    }

    #[test]
    fn a_huge_interval_does_not_overflow() {
        assert!(parse_duration(&format!("{}h", u64::MAX)).is_err());
        assert_eq!(parse_duration("100000h"), Ok(360_000_000));
    }
}
