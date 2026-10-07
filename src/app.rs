use crate::config::Config;
use crate::diff_view::{ClaudeComment, DiffView};
use crate::draft::{DraftStore, PrDraft, ReviewState};
use crate::github::{CiState, Commit, GithubClient, PrStatus, PullRequest, RepoInfo};
use crate::highlight::Highlighter;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Animation frame for in-progress indicators.
pub fn spinner_char(frame: usize) -> char {
    SPINNER_FRAMES[(frame / 2) % SPINNER_FRAMES.len()]
}

/// Count of AI findings still to triage — deliberately brighter than the
/// magenta icon beside it so the number reads first.
pub const AI_COUNT_COLOR: Color = Color::Rgb(255, 160, 255);
/// Count of your own comments waiting to be submitted.
pub const DRAFT_COUNT_COLOR: Color = Color::Rgb(140, 255, 140);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Panel {
    PullRequests,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Overview,
    Diff,
}

/// Messages from background tasks back to the UI
pub enum BgMsg {
    UserLoaded(String),
    AssignedLoaded(Vec<RepoInfo>),
    /// Result of a background poll — applied without disturbing the selection.
    AssignedRefreshed(Vec<RepoInfo>),
    AllPrsLoaded(Vec<RepoInfo>),
    Error(String),
    /// A background auto-review finished (or failed) for one PR.
    AutoReviewDone {
        repo: String,
        pr_number: u64,
        head_sha: String,
        result: Result<Vec<ClaudeComment>, String>,
    },
    /// Drafts whose PRs turned out to be closed and were deleted.
    DraftsPruned(Vec<(String, u64)>),
    StatusesLoaded(Vec<((String, u64), PrStatus)>),
    DiffLoaded(String),
    ThreadsLoaded(Vec<crate::github::ReviewThread>),
    ClaudeReviewParsed(Vec<ClaudeComment>),
    ClaudeReviewOutput(String),
    ApproveResult(Result<(), String>),
    CommentResult(Result<(), String>),
    SubmitResult(Result<(usize, String), String>),
    CommitsLoaded((String, u64, Vec<Commit>)),
}

/// A flattened PR entry with its repo name
#[derive(Debug, Clone)]
pub struct FlatPr {
    pub repo_name: String,
    pub repo_short: String,
    pub pr: PullRequest,
}

pub struct App {
    assigned_repos: Vec<RepoInfo>,
    all_repos: Vec<RepoInfo>,
    /// Flat list of all PRs (current filter applied)
    pub flat_prs: Vec<FlatPr>,
    pub show_assigned_only: bool,
    pub all_repos_loaded: bool,
    pub pr_index: usize,
    pub diff_scroll: u16,
    pub details_scroll: u16,
    pub active_panel: Panel,
    pub active_tab: Tab,
    pub loading: bool,
    pub loading_diff: bool,
    pub error: Option<String>,
    pub current_diff: Option<String>,
    pub diff_pr_key: Option<(String, u64)>,
    pub pr_statuses: HashMap<(String, u64), PrStatus>,
    pub username: String,
    pub client: GithubClient,
    pub should_quit: bool,
    pub bg_rx: mpsc::UnboundedReceiver<BgMsg>,
    pub bg_tx: mpsc::UnboundedSender<BgMsg>,
    status_requested: std::collections::HashSet<String>,
    pub show_help: bool,
    pub diff_view: Option<DiffView>,
    pub tree_index: usize,
    pub diff_focus: DiffFocus,
    /// Buffered threads/claude results waiting for diff_view to be created
    pending_threads: Option<Vec<crate::github::ReviewThread>>,
    pending_claude: Option<Vec<ClaudeComment>>,
    pub file_pane_width: u16,
    pub approve_popup: Option<ApprovePopup>,
    pub comment_popup: Option<CommentPopup>,
    pub highlighter: Highlighter,
    pub config: Config,
    /// Search filter mode: if Some, user is typing a filter
    pub search_mode: bool,
    pub search_query: String,
    /// Index in flat_prs where the "approved by me" section starts (None if no split)
    pub approved_separator: Option<usize>,
    /// Frame counter for spinner animation
    pub frame: usize,
    /// Pending quit confirmation (shows when there are unsaved drafts)
    pub confirm_quit: Option<ConfirmQuit>,
    /// Commits of the current PR (chronological, oldest first)
    pub pr_commits: Vec<Commit>,
    /// Selected commit range: (start_idx, end_idx) inclusive into pr_commits
    /// Full range = (0, commits.len() - 1)
    pub commit_range: Option<(usize, usize)>,
    /// Locally stored review drafts, keyed by (repo, pr number)
    pub drafts: DraftStore,
    /// PRs waiting for a background AI review
    auto_queue: VecDeque<(String, PullRequest)>,
    /// PRs whose AI review is running right now
    auto_running: HashSet<(String, u64)>,
    /// When the assigned-PR list was last polled
    last_poll: Instant,
    /// Whether a poll request is currently in flight
    polling: bool,
    /// Draft to load into the diff view once its diff arrives
    pending_draft: Option<PrDraft>,
    /// Where panes landed in the last frame, for mapping mouse events
    pub hit: RefCell<HitMap>,
    /// Text being selected with the mouse
    pub selection: Option<Selection>,
    /// Selected text the next loop iteration should put on the clipboard
    pub clipboard_out: Option<String>,
    /// One-off notice shown in the corner until the next input
    pub flash: Option<String>,
}

/// Screen geometry recorded while drawing. Rebuilt every frame; empty rects
/// mean the pane wasn't drawn.
#[derive(Debug, Clone, Default)]
pub struct HitMap {
    pub screen: Rect,
    pub tree: Rect,
    pub tree_offset: usize,
    pub content: Rect,
    /// Screen rows showing a diff line (rather than a comment): (y, line index)
    pub code_rows: Vec<(u16, usize)>,
    pub prs: Rect,
    pub prs_offset: usize,
    /// Items of the PR list in order: the PR index, or None for the separator
    pub pr_items: Vec<Option<usize>>,
    pub details: Rect,
    /// A popup that owns the mouse while open
    pub popup: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct Selection {
    /// Inner area of the pane the drag started in; the selection never
    /// leaves it
    pub pane: Rect,
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    pub dragged: bool,
    /// Copy on the next draw, while the buffer holds what's on screen
    pub copy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfirmQuit {
    /// Quit the entire app
    App,
}

pub struct ApprovePopup {
    pub repo_name: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub comment: String,
    pub cursor: usize,
    pub submitting: bool,
    pub result_msg: Option<String>,
}

pub struct CommentPopup {
    pub repo_name: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub body: String,
    pub cursor: usize,
    pub submitting: bool,
    pub result_msg: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffFocus {
    Files,
    Content,
}



impl App {
    pub fn new(client: GithubClient, config: Config) -> Self {
        let (bg_tx, bg_rx) = mpsc::unbounded_channel();

        // Apply the severity threshold to findings stored by earlier runs.
        let mut drafts = DraftStore::load();
        if let Some(min) = config.min_severity_rank() {
            drafts.drop_below_severity(min);
        }

        Self {
            assigned_repos: Vec::new(),
            all_repos: Vec::new(),
            flat_prs: Vec::new(),
            show_assigned_only: true,
            all_repos_loaded: false,
            pr_index: 0,
            diff_scroll: 0,
            details_scroll: 0,
            active_panel: Panel::PullRequests,
            active_tab: Tab::Overview,
            loading: true,
            loading_diff: false,
            error: None,
            current_diff: None,
            diff_pr_key: None,
            pr_statuses: HashMap::new(),
            username: String::new(),
            client,
            should_quit: false,
            bg_rx,
            bg_tx,
            status_requested: std::collections::HashSet::new(),
            show_help: false,
            diff_view: None,
            tree_index: 0,
            diff_focus: DiffFocus::Content,
            pending_threads: None,
            pending_claude: None,
            file_pane_width: 30,
            approve_popup: None,
            comment_popup: None,
            highlighter: Highlighter::new(),
            config,
            search_mode: false,
            search_query: String::new(),
            approved_separator: None,
            frame: 0,
            confirm_quit: None,
            pr_commits: Vec::new(),
            commit_range: None,
            drafts,
            auto_queue: VecDeque::new(),
            auto_running: HashSet::new(),
            last_poll: Instant::now(),
            polling: false,
            pending_draft: None,
            hit: RefCell::new(HitMap::default()),
            selection: None,
            clipboard_out: None,
            flash: None,
        }
    }

    /// Flatten repos into a single PR list
    fn apply_filter(&mut self) {
        let repos = if self.show_assigned_only {
            &self.assigned_repos
        } else {
            &self.all_repos
        };

        let query = self.search_query.to_lowercase();
        self.flat_prs = repos
            .iter()
            .flat_map(|repo| {
                let short = repo
                    .full_name
                    .split('/')
                    .last()
                    .unwrap_or(&repo.full_name)
                    .to_string();
                repo.pull_requests.iter().map(move |pr| FlatPr {
                    repo_name: repo.full_name.clone(),
                    repo_short: short.clone(),
                    pr: pr.clone(),
                })
            })
            .filter(|fpr| {
                if query.is_empty() {
                    return true;
                }
                // Match against PR title, number, repo name, author
                let num_str = fpr.pr.number.to_string();
                fpr.pr.title.to_lowercase().contains(&query)
                    || num_str.contains(&query)
                    || fpr.repo_short.to_lowercase().contains(&query)
                    || fpr.pr.user.login.to_lowercase().contains(&query)
            })
            .collect();

        // Sort by created_at descending (newest first)
        self.flat_prs.sort_by(|a, b| b.pr.created_at.cmp(&a.pr.created_at));

        // Partition: not-approved-by-me first, then approved-by-me
        let all_prs: Vec<FlatPr> = self.flat_prs.drain(..).collect();
        let mut not_approved = Vec::new();
        let mut approved = Vec::new();
        for fpr in all_prs {
            if self.is_approved_by_me(&fpr.repo_name, fpr.pr.number) {
                approved.push(fpr);
            } else {
                not_approved.push(fpr);
            }
        }

        self.approved_separator = if !not_approved.is_empty() && !approved.is_empty() {
            Some(not_approved.len())
        } else {
            None
        };

        self.flat_prs = not_approved;
        self.flat_prs.extend(approved);

        self.pr_index = self.pr_index.min(self.flat_prs.len().saturating_sub(1));
        self.current_diff = None;
        self.diff_pr_key = None;
    }

    /// Re-partition flat_prs into not-approved / approved sections without rebuilding
    fn recompute_approved_separator(&mut self) {
        let all_prs: Vec<FlatPr> = self.flat_prs.drain(..).collect();
        let mut not_approved = Vec::new();
        let mut approved = Vec::new();
        for fpr in all_prs {
            if self.is_approved_by_me(&fpr.repo_name, fpr.pr.number) {
                approved.push(fpr);
            } else {
                not_approved.push(fpr);
            }
        }

        // Preserve created_at descending within each group
        not_approved.sort_by(|a, b| b.pr.created_at.cmp(&a.pr.created_at));
        approved.sort_by(|a, b| b.pr.created_at.cmp(&a.pr.created_at));

        self.approved_separator = if !not_approved.is_empty() && !approved.is_empty() {
            Some(not_approved.len())
        } else {
            None
        };

        self.flat_prs = not_approved;
        self.flat_prs.extend(approved);
    }

    /// Public method to re-apply filter (used by search)
    pub fn apply_filter_public(&mut self) {
        self.apply_filter();
        self.request_all_statuses();
    }

    pub fn toggle_assigned(&mut self) {
        self.show_assigned_only = !self.show_assigned_only;
        if !self.show_assigned_only && !self.all_repos_loaded {
            self.fetch_all_repo_prs();
        }
        self.pr_index = 0;
        self.apply_filter();
        self.request_all_statuses();
    }

    fn fetch_all_repo_prs(&self) {
        let repo_names: Vec<String> = self
            .assigned_repos
            .iter()
            .map(|r| r.full_name.clone())
            .collect();
        if repo_names.is_empty() {
            return;
        }
        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match client.fetch_all_prs_for_repos(&repo_names).await {
                Ok(repos) => {
                    let _ = tx.send(BgMsg::AllPrsLoaded(repos));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::Error(format!("Failed to fetch all PRs: {}", e)));
                }
            }
        });
    }

    pub fn selected_flat_pr(&self) -> Option<&FlatPr> {
        self.flat_prs.get(self.pr_index)
    }

    pub fn selected_pr(&self) -> Option<&PullRequest> {
        self.selected_flat_pr().map(|f| &f.pr)
    }

    pub fn selected_repo_name(&self) -> Option<String> {
        self.selected_flat_pr().map(|f| f.repo_name.clone())
    }

    pub fn pr_status(&self, repo: &str, pr_number: u64) -> Option<&PrStatus> {
        self.pr_statuses.get(&(repo.to_string(), pr_number))
    }

    pub fn is_approved_by_me(&self, repo: &str, pr_number: u64) -> bool {
        if let Some(status) = self.pr_status(repo, pr_number) {
            status
                .reviews
                .iter()
                .any(|r| r.user.login == self.username && r.state == "APPROVED")
        } else {
            false
        }
    }

    pub fn review_icon(&self, repo: &str, pr: &PullRequest) -> &str {
        if let Some(status) = self.pr_status(repo, pr.number) {
            let mut latest: HashMap<&str, &str> = HashMap::new();
            for review in &status.reviews {
                latest.insert(&review.user.login, &review.state);
            }

            let has_my_approval = latest
                .get(self.username.as_str())
                .map_or(false, |s| *s == "APPROVED");
            let has_changes_requested = latest.values().any(|s| *s == "CHANGES_REQUESTED");
            let has_any_approval = latest.values().any(|s| *s == "APPROVED");

            if has_my_approval {
                "\u{f164} " // nf-fa-thumbs_up
            } else if has_changes_requested {
                "\u{f467} " // nf-oct-request_changes
            } else if has_any_approval {
                "\u{f164} " // nf-fa-thumbs_up (cyan in UI)
            } else {
                "\u{f4a1} " // nf-oct-code_review
            }
        } else {
            ""
        }
    }

    /// Icons for the PR list describing the local draft: whether a review is
    /// running, waiting, failed, has undecided findings, or holds changes that
    /// haven't been submitted yet.
    pub fn draft_indicators(&self, repo: &str, pr_number: u64) -> Vec<(String, Color)> {
        let mut out = Vec::new();

        if self.is_reviewing(repo, pr_number) {
            out.push((
                format!("{} ", spinner_char(self.frame)),
                Color::Rgb(200, 120, 255),
            ));
        }

        let Some(draft) = self.drafts.get(repo, pr_number) else { return out };

        if !self.is_reviewing(repo, pr_number) {
            match draft.review_state {
                // Waiting its turn in the review queue
                ReviewState::Queued => out.push(("\u{f017} ".to_string(), Color::DarkGray)),
                ReviewState::Failed => out.push(("\u{f071} ".to_string(), Color::Red)),
                _ => {}
            }
        }

        // Findings still to triage and your own unsubmitted comments are
        // independent facts about a PR, so both are shown. The counts are
        // brighter than their icons so they stand out at a glance.
        let pending = draft.pending_ai_count();
        if pending > 0 {
            out.push(("\u{f12a}".to_string(), Color::Magenta));
            out.push((format!("{} ", pending), AI_COUNT_COLOR));
        }

        let unsubmitted = draft.unsubmitted_count();
        if unsubmitted > 0 {
            out.push(("\u{f040}".to_string(), Color::Green));
            out.push((format!("{} ", unsubmitted), DRAFT_COUNT_COLOR));
        }

        out
    }

    pub fn select_pr(&mut self, index: usize) {
        if index < self.flat_prs.len() && index != self.pr_index {
            self.pr_index = index;
            self.current_diff = None;
            self.diff_pr_key = None;
            self.details_scroll = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.active_tab == Tab::Diff {
            self.diff_scroll = self.diff_scroll.saturating_sub(3);
            return;
        }
        match self.active_panel {
            Panel::PullRequests => {
                if self.pr_index > 0 {
                    self.pr_index -= 1;
                    self.current_diff = None;
                    self.diff_pr_key = None;
                    self.details_scroll = 0;
                }
            }
            Panel::Details => {
                self.details_scroll = self.details_scroll.saturating_sub(3);
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.active_tab == Tab::Diff {
            self.diff_scroll = self.diff_scroll.saturating_add(3);
            return;
        }
        match self.active_panel {
            Panel::PullRequests => {
                if !self.flat_prs.is_empty()
                    && self.pr_index < self.flat_prs.len().saturating_sub(1)
                {
                    self.pr_index += 1;
                    self.current_diff = None;
                    self.diff_pr_key = None;
                    self.details_scroll = 0;
                }
            }
            Panel::Details => {
                self.details_scroll = self.details_scroll.saturating_add(3);
            }
        }
    }

    pub fn next_panel(&mut self) {
        self.active_panel = match self.active_panel {
            Panel::PullRequests => Panel::Details,
            Panel::Details => Panel::PullRequests,
        };
    }

    pub fn start_loading(&self) {
        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let (user_res, prs_res) = tokio::join!(
                client.get_authenticated_user(),
                client.fetch_my_prs()
            );

            match user_res {
                Ok(user) => {
                    let _ = tx.send(BgMsg::UserLoaded(user));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::Error(format!("Auth failed: {}", e)));
                    return;
                }
            }

            match prs_res {
                Ok(repos) => {
                    let _ = tx.send(BgMsg::AssignedLoaded(repos));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::Error(format!("Failed to fetch PRs: {}", e)));
                }
            }
        });
    }

    /// Comments and resolves saved locally but not yet posted to GitHub,
    /// across every PR, and the number of PRs holding them.
    pub fn unsubmitted_totals(&self) -> (usize, usize) {
        self.drafts.unsubmitted_totals()
    }

    /// Whether anything is waiting to be submitted anywhere
    pub fn has_pending_drafts(&self) -> bool {
        self.unsubmitted_totals().0 > 0
    }

    /// Whether background data is still being fetched (statuses, all repos, etc.)
    pub fn is_fetching(&self) -> bool {
        if !self.show_assigned_only && !self.all_repos_loaded {
            return true;
        }
        self.flat_prs.iter().any(|fpr| {
            !self.pr_statuses.contains_key(&(fpr.repo_name.clone(), fpr.pr.number))
        })
    }

    /// Request statuses for all visible PRs (batched by repo, deduped)
    pub fn request_all_statuses(&mut self) {
        // Collect all unique repos in current view
        let mut by_repo: HashMap<String, Vec<PullRequest>> = HashMap::new();
        for fpr in &self.flat_prs {
            let key = (fpr.repo_name.clone(), fpr.pr.number);
            if !self.pr_statuses.contains_key(&key) {
                by_repo
                    .entry(fpr.repo_name.clone())
                    .or_default()
                    .push(fpr.pr.clone());
            }
        }

        for (repo_name, prs) in by_repo {
            if self.status_requested.contains(&repo_name) {
                continue;
            }
            self.status_requested.insert(repo_name.clone());

            let items: Vec<_> = prs.into_iter().map(|pr| (repo_name.clone(), pr)).collect();
            let client = self.client.clone();
            let tx = self.bg_tx.clone();
            tokio::spawn(async move {
                let results = client.fetch_statuses_batch(items).await;
                let _ = tx.send(BgMsg::StatusesLoaded(results));
            });
        }
    }

    pub fn request_diff(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else {
            return;
        };
        let Some(pr) = self.selected_pr() else {
            return;
        };
        let pr_number = pr.number;
        let key = (repo_name.clone(), pr_number);

        if self.diff_pr_key.as_ref() == Some(&key) && self.current_diff.is_some() {
            return;
        }

        self.loading_diff = true;
        self.current_diff = None;
        self.diff_pr_key = Some(key);
        self.diff_scroll = 0;

        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match client.fetch_pr_diff(&repo_name, pr_number).await {
                Ok(diff) => {
                    let _ = tx.send(BgMsg::DiffLoaded(diff));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::DiffLoaded(format!("Error loading diff: {}", e)));
                }
            }
        });
    }

    /// Re-fetch diff using the current commit range
    pub fn request_diff_for_range(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr() else { return };
        let pr_number = pr.number;

        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        self.loading_diff = true;
        self.current_diff = None;

        // If no range or full range, use PR diff
        let use_full = match self.commit_range {
            None => true,
            Some((s, e)) => s == 0 && e + 1 == self.pr_commits.len(),
        };

        if use_full || self.pr_commits.is_empty() {
            tokio::spawn(async move {
                match client.fetch_pr_diff(&repo_name, pr_number).await {
                    Ok(diff) => { let _ = tx.send(BgMsg::DiffLoaded(diff)); }
                    Err(e) => { let _ = tx.send(BgMsg::DiffLoaded(format!("Error loading diff: {}", e))); }
                }
            });
            return;
        }

        let (start, end) = self.commit_range.unwrap();
        // Base = parent of start commit (if exists), otherwise start commit itself
        let base_sha = self.pr_commits[start]
            .parents
            .first()
            .map(|p| p.sha.clone())
            .unwrap_or_else(|| self.pr_commits[start].sha.clone());
        let head_sha = self.pr_commits[end].sha.clone();

        tokio::spawn(async move {
            match client.fetch_compare_diff(&repo_name, &base_sha, &head_sha).await {
                Ok(diff) => { let _ = tx.send(BgMsg::DiffLoaded(diff)); }
                Err(e) => { let _ = tx.send(BgMsg::DiffLoaded(format!("Error loading diff: {}", e))); }
            }
        });
    }

    /// Fetch commits for the PR
    pub fn fetch_commits(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr() else { return };
        let pr_number = pr.number;
        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        let repo = repo_name.clone();
        tokio::spawn(async move {
            if let Ok(commits) = client.fetch_pr_commits(&repo, pr_number).await {
                let _ = tx.send(BgMsg::CommitsLoaded((repo, pr_number, commits)));
            }
        });
    }

    /// Adjust commit range and re-fetch diff
    pub fn move_range_start(&mut self, delta: i32) {
        if self.pr_commits.is_empty() { return; }
        let Some((s, e)) = self.commit_range else { return };
        let new_s = (s as i32 + delta).max(0).min(e as i32) as usize;
        if new_s != s {
            self.commit_range = Some((new_s, e));
            self.request_diff_for_range();
        }
    }

    pub fn move_range_end(&mut self, delta: i32) {
        if self.pr_commits.is_empty() { return; }
        let Some((s, e)) = self.commit_range else { return };
        let max = self.pr_commits.len() as i32 - 1;
        let new_e = (e as i32 + delta).max(s as i32).min(max) as usize;
        if new_e != e {
            self.commit_range = Some((s, new_e));
            self.request_diff_for_range();
        }
    }

    pub fn show_approve_popup(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr() else { return };
        let result_msg = if pr.draft {
            Some("✗ Cannot approve a draft PR".to_string())
        } else if pr.user.login == self.username {
            Some("✗ Cannot approve your own PR".to_string())
        } else if self.is_approved_by_me(&repo_name, pr.number) {
            Some("✗ Already approved by you".to_string())
        } else {
            None
        };
        self.approve_popup = Some(ApprovePopup {
            repo_name,
            pr_number: pr.number,
            pr_title: pr.title.clone(),
            comment: String::new(),
            cursor: 0,
            submitting: false,
            result_msg,
        });
    }

    pub fn submit_approve(&mut self) {
        let Some(popup) = &self.approve_popup else { return };
        if popup.submitting || popup.result_msg.is_some() { return; }

        let repo = popup.repo_name.clone();
        let pr_number = popup.pr_number;
        let comment = popup.comment.clone();
        let client = self.client.clone();
        let tx = self.bg_tx.clone();

        if let Some(p) = &mut self.approve_popup {
            p.submitting = true;
        }

        tokio::spawn(async move {
            let result = client.approve_pr(&repo, pr_number, &comment).await;
            match result {
                Ok(()) => { let _ = tx.send(BgMsg::ApproveResult(Ok(()))); }
                Err(e) => { let _ = tx.send(BgMsg::ApproveResult(Err(e.to_string()))); }
            }
        });
    }

    pub fn show_comment_popup(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr() else { return };
        self.comment_popup = Some(CommentPopup {
            repo_name,
            pr_number: pr.number,
            pr_title: pr.title.clone(),
            body: String::new(),
            cursor: 0,
            submitting: false,
            result_msg: None,
        });
    }

    pub fn submit_comment(&mut self) {
        let Some(popup) = &self.comment_popup else { return };
        if popup.submitting || popup.result_msg.is_some() { return; }
        if popup.body.trim().is_empty() { return; }

        let repo = popup.repo_name.clone();
        let pr_number = popup.pr_number;
        let body = popup.body.clone();
        let client = self.client.clone();
        let tx = self.bg_tx.clone();

        if let Some(p) = &mut self.comment_popup {
            p.submitting = true;
        }

        tokio::spawn(async move {
            let result = client.post_comment(&repo, pr_number, &body).await;
            match result {
                Ok(()) => { let _ = tx.send(BgMsg::CommentResult(Ok(()))); }
                Err(e) => { let _ = tx.send(BgMsg::CommentResult(Err(e.to_string()))); }
            }
        });
    }

    /// Submit all draft comments and pending resolves to GitHub
    pub fn submit_drafts(&mut self) {
        let Some(dv) = &self.diff_view else { return };
        if dv.draft_comments.is_empty() && dv.pending_resolves.is_empty() { return; }

        let drafts = dv.draft_comments.clone();
        let threads = dv.threads.clone();
        let standalone_resolves: Vec<String> = dv.pending_resolves.iter()
            .filter_map(|&ti| threads.get(ti)?.node_id.clone())
            .collect();
        let repo = dv.repo_name.clone();
        let pr_number = dv.pr_number;
        let client = self.client.clone();
        let tx = self.bg_tx.clone();

        let count = drafts.len();

        tokio::spawn(async move {
            // Separate new comments from replies, collect threads to resolve
            let mut new_comments: Vec<(String, u64, String)> = Vec::new();
            let mut replies: Vec<(u64, String)> = Vec::new();
            let mut resolve_ids: Vec<String> = standalone_resolves;

            for draft in &drafts {
                if let Some(thread_idx) = draft.in_reply_to_thread {
                    if let Some(thread) = threads.get(thread_idx) {
                        if let Some(first) = thread.comments.first() {
                            replies.push((first.id, draft.body.clone()));
                        }
                        if draft.resolve {
                            if let Some(node_id) = &thread.node_id {
                                if !resolve_ids.contains(node_id) {
                                    resolve_ids.push(node_id.clone());
                                }
                            }
                        }
                    }
                } else {
                    new_comments.push((draft.file.clone(), draft.line, draft.body.clone()));
                }
            }

            // Submit comments if any
            if !new_comments.is_empty() || !replies.is_empty() {
                if let Err(e) = client.submit_review(&repo, pr_number, new_comments, replies).await {
                    let _ = tx.send(BgMsg::SubmitResult(Err(e.to_string())));
                    return;
                }
            }

            // Resolve threads
            let mut resolve_errors = Vec::new();
            for node_id in &resolve_ids {
                if let Err(e) = client.resolve_thread(node_id).await {
                    resolve_errors.push(e.to_string());
                }
            }

            let resolved_count = resolve_ids.len() - resolve_errors.len();
            let mut parts = Vec::new();
            if count > 0 {
                parts.push(format!("{} comment{}", count, if count == 1 { "" } else { "s" }));
            }
            if resolved_count > 0 {
                parts.push(format!("{} resolved", resolved_count));
            }
            let msg = if resolve_errors.is_empty() {
                format!("Submitted: {}", parts.join(", "))
            } else {
                format!("Submitted: {} (resolve failed: {})", parts.join(", "), resolve_errors.join("; "))
            };
            let _ = tx.send(BgMsg::SubmitResult(Ok((count, msg))));
        });
    }

    /// Toggle draft resolve on the thread nearest to cursor
    pub fn resolve_thread_at_cursor(&mut self) {
        let Some(dv) = &mut self.diff_view else { return };
        // Find nearest thread within ±3 lines
        let mut found_ti = None;
        for offset in 0..=3usize {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![dv.cursor_line]
            } else {
                vec![dv.cursor_line.saturating_sub(offset), dv.cursor_line + offset]
            };
            for li in lines_to_check {
                if let Some(thread_indices) = dv.line_threads.get(&li) {
                    if let Some(&ti) = thread_indices.first() {
                        found_ti = Some(ti);
                        break;
                    }
                }
            }
            if found_ti.is_some() { break; }
        }

        let Some(ti) = found_ti else { return };
        if let Some(thread) = dv.threads.get(ti) {
            if thread.is_resolved { return; }
        }
        // Toggle: add or remove from pending_resolves
        if let Some(pos) = dv.pending_resolves.iter().position(|&x| x == ti) {
            dv.pending_resolves.remove(pos);
        } else {
            dv.pending_resolves.push(ti);
        }
        dv.dirty = true;
    }

    /// Fetch review threads in background
    fn fetch_threads(&self, repo_name: &str, pr: &PullRequest) {
        let parts: Vec<&str> = repo_name.split('/').collect();
        if parts.len() == 2 {
            let client = self.client.clone();
            let tx = self.bg_tx.clone();
            let owner = parts[0].to_string();
            let repo_short = parts[1].to_string();
            let repo_full = repo_name.to_string();
            let pr_num = pr.number;
            let pr_clone = pr.clone();
            tokio::spawn(async move {
                let threads = match client.fetch_review_threads(&owner, &repo_short, pr_num).await {
                    Ok(t) if !t.is_empty() => t,
                    _ => {
                        client.build_threads_from_rest(&repo_full, &pr_clone).await
                    }
                };
                let _ = tx.send(BgMsg::ThreadsLoaded(threads));
            });
        }
    }

    /// Open diff view (loads diff, threads, optionally Claude review)
    pub fn open_diff_view(&mut self, with_ai: bool) {
        self.diff_view = None;
        self.pending_threads = None;
        self.pending_claude = None;
        self.tree_index = 0;
        self.diff_focus = DiffFocus::Files;
        self.pr_commits.clear();
        self.commit_range = None;
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr().cloned() else { return };

        // Stored draft is applied once the diff has been parsed.
        self.pending_draft = self.drafts.get(&repo_name, pr.number).cloned();

        // Force re-fetch diff
        self.current_diff = None;
        self.diff_pr_key = None;
        self.request_diff();

        // Fetch commits in parallel so the range can be adjusted later
        self.fetch_commits();

        self.fetch_threads(&repo_name, &pr);

        // Run Claude structured review if requested
        if with_ai {
            self.run_structured_ai_review(&repo_name, &pr);
        }

        // DiffView will be created when DiffLoaded arrives
        self.active_tab = Tab::Diff;
    }

    /// Public wrapper to trigger AI review from diff view
    pub fn run_ai_review_bg(&self, repo_name: &str, pr: &PullRequest) {
        self.run_structured_ai_review(repo_name, pr);
    }

    fn run_structured_ai_review(&self, repo_name: &str, pr: &PullRequest) {
        let tx = self.bg_tx.clone();
        let repo = repo_name.to_string();
        let pr_number = pr.number;
        let config = self.config.clone();

        tokio::spawn(async move {
            let pr_url = format!("https://github.com/{}/pull/{}", repo, pr_number);
            let _ = tx.send(BgMsg::ClaudeReviewOutput(format!(
                "Running {} review on {}...\n\n",
                config.ai.name, pr_url
            )));

            match run_review_process(&config, &pr_url, Some(&tx)).await {
                Ok(parsed) => {
                    let _ = tx.send(BgMsg::ClaudeReviewOutput(format!(
                        "\n\n--- Found {} inline comments ---\n",
                        parsed.len()
                    )));
                    let _ = tx.send(BgMsg::ClaudeReviewParsed(parsed));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::ClaudeReviewOutput(format!("Error: {}\n", e)));
                    let _ = tx.send(BgMsg::ClaudeReviewParsed(Vec::new()));
                }
            }
        });
    }

    // ── Background polling & automatic review ──────────────────────────────

    /// Called every UI frame. Kicks off a poll when the interval has elapsed
    /// and keeps the review queue moving.
    pub fn tick_auto(&mut self) {
        if !self.config.auto.enabled {
            return;
        }
        let interval = Duration::from_secs(self.config.auto.poll_interval_secs.max(15));
        if !self.polling && !self.loading && self.last_poll.elapsed() >= interval {
            self.last_poll = Instant::now();
            self.spawn_poll();
        }
        self.pump_auto_queue();
    }

    /// Re-fetch the assigned PR list in the background.
    fn spawn_poll(&mut self) {
        self.polling = true;
        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            match client.fetch_my_prs().await {
                Ok(repos) => {
                    let _ = tx.send(BgMsg::AssignedRefreshed(repos));
                }
                // A failed poll is not worth interrupting the user for; the
                // next tick will try again.
                Err(_) => {
                    let _ = tx.send(BgMsg::AssignedRefreshed(Vec::new()));
                }
            }
        });
    }

    /// Apply a polled PR list, keeping the current selection on the same PR.
    fn apply_refresh(&mut self, repos: Vec<RepoInfo>) {
        // Drop cached statuses for PRs that changed so their CI/review icons
        // refresh, but leave untouched PRs alone to avoid icon flicker.
        let mut previous: HashMap<(String, u64), chrono::DateTime<chrono::Utc>> = HashMap::new();
        for repo in &self.assigned_repos {
            for pr in &repo.pull_requests {
                previous.insert((repo.full_name.clone(), pr.number), pr.updated_at);
            }
        }
        for repo in &repos {
            for pr in &repo.pull_requests {
                let key = (repo.full_name.clone(), pr.number);
                if previous.get(&key).map_or(false, |&was| was != pr.updated_at) {
                    self.pr_statuses.remove(&key);
                }
            }
        }

        let selected_key = self
            .selected_flat_pr()
            .map(|f| (f.repo_name.clone(), f.pr.number));

        self.assigned_repos = repos;

        if self.show_assigned_only {
            let old_index = self.pr_index;
            self.rebuild_list_preserving(selected_key, old_index);
        }

        // Allow status fetches for newly appeared PRs.
        self.status_requested.clear();
        self.request_all_statuses();

        self.schedule_auto_reviews();
        self.spawn_draft_prune();
    }

    /// Rebuild `flat_prs` from the current repos, then put the cursor back on
    /// the PR it was on. Unlike `apply_filter` this leaves the loaded diff
    /// alone, so a poll can't yank the view out from under an open review.
    fn rebuild_list_preserving(
        &mut self,
        selected_key: Option<(String, u64)>,
        fallback_index: usize,
    ) {
        let saved_diff = (self.current_diff.take(), self.diff_pr_key.take());
        self.apply_filter();
        self.current_diff = saved_diff.0;
        self.diff_pr_key = saved_diff.1;

        self.pr_index = selected_key
            .and_then(|(repo, num)| {
                self.flat_prs
                    .iter()
                    .position(|f| f.repo_name == repo && f.pr.number == num)
            })
            .unwrap_or(fallback_index)
            .min(self.flat_prs.len().saturating_sub(1));
    }

    /// Should this PR be reviewed automatically?
    fn is_auto_review_target(&self, repo: &str, pr: &PullRequest) -> bool {
        self.auto_skip_reason(repo, pr).is_none()
    }

    /// Why this PR is not auto-reviewed, or `None` if it is eligible.
    fn auto_skip_reason(&self, repo: &str, pr: &PullRequest) -> Option<&'static str> {
        // Never review drafts, closed PRs, or your own work.
        if pr.draft {
            return Some("draft PR");
        }
        if pr.state != "open" {
            return Some("not open");
        }
        if pr.user.login == self.username {
            return Some("your own PR");
        }
        // Approval means you're done with it — no point spending a review on
        // it. Press `c` if you deliberately want another look.
        if self.is_approved_by_me(repo, pr.number) {
            return Some("already approved by you");
        }
        // Approval isn't known until the PR's status has loaded, so hold off
        // rather than reviewing something that turns out to be approved.
        // `StatusesLoaded` re-runs the scheduler once it arrives.
        if self.pr_status(repo, pr.number).is_none() {
            return Some("waiting for review status");
        }
        // A PR you already have a local draft for stays in scope, so pushes
        // that follow your comments get re-reviewed even after GitHub drops
        // you from the requested reviewers.
        if self
            .drafts
            .get(repo, pr.number)
            .map_or(false, |d| d.is_engaged())
        {
            return None;
        }
        if !self.config.auto.only_assigned {
            return None;
        }
        let assigned = pr.requested_reviewers.iter().any(|u| u.login == self.username)
            || pr.assignees.iter().any(|u| u.login == self.username);
        if assigned {
            None
        } else {
            Some("not assigned to you")
        }
    }

    /// Queue reviews for PRs that are new, changed, or never got a result.
    pub fn schedule_auto_reviews(&mut self) {
        if !self.config.auto.enabled || self.username.is_empty() {
            return;
        }

        let mut candidates: Vec<(String, PullRequest)> = Vec::new();
        for repo in &self.assigned_repos {
            for pr in &repo.pull_requests {
                if !self.is_auto_review_target(&repo.full_name, pr) {
                    continue;
                }
                let key = (repo.full_name.clone(), pr.number);
                if self.auto_running.contains(&key) {
                    continue;
                }
                if self
                    .auto_queue
                    .iter()
                    .any(|(r, p)| *r == key.0 && p.number == pr.number)
                {
                    continue;
                }
                let needs_review = match self.drafts.get(&repo.full_name, pr.number) {
                    None => true,
                    Some(d) => {
                        d.head_sha != pr.head.sha
                            || matches!(
                                d.review_state,
                                ReviewState::NotRun
                                    | ReviewState::Queued
                                    | ReviewState::Running
                                    | ReviewState::Failed
                            )
                    }
                };
                if needs_review {
                    candidates.push((repo.full_name.clone(), pr.clone()));
                }
            }
        }

        for (repo, pr) in candidates {
            let draft = self
                .drafts
                .get_or_create(&repo, pr.number, &pr.title, &pr.head.sha);
            draft.pr_title = pr.title.clone();
            draft.review_state = ReviewState::Queued;
            self.drafts.save(&repo, pr.number);
            self.auto_queue.push_back((repo, pr));
        }

        self.pump_auto_queue();
    }

    /// Start reviews from the queue up to the configured concurrency.
    fn pump_auto_queue(&mut self) {
        let max = self.config.auto.max_concurrent.max(1);
        while self.auto_running.len() < max {
            let Some((repo, pr)) = self.auto_queue.pop_front() else { break };
            self.auto_running.insert((repo.clone(), pr.number));

            let draft = self
                .drafts
                .get_or_create(&repo, pr.number, &pr.title, &pr.head.sha);
            draft.review_state = ReviewState::Running;
            self.drafts.save(&repo, pr.number);

            let tx = self.bg_tx.clone();
            let config = self.config.clone();
            let head_sha = pr.head.sha.clone();
            let pr_number = pr.number;
            let repo_name = repo.clone();
            tokio::spawn(async move {
                let pr_url = format!("https://github.com/{}/pull/{}", repo_name, pr_number);
                let result = run_review_process(&config, &pr_url, None).await;
                let _ = tx.send(BgMsg::AutoReviewDone {
                    repo: repo_name,
                    pr_number,
                    head_sha,
                    result,
                });
            });
        }
    }

    /// Look for stored drafts whose PR has since been closed and delete them.
    fn spawn_draft_prune(&self) {
        let open: HashSet<(String, u64)> = self
            .assigned_repos
            .iter()
            .flat_map(|r| {
                r.pull_requests
                    .iter()
                    .map(move |pr| (r.full_name.clone(), pr.number))
            })
            .collect();

        let candidates: Vec<(String, u64)> = self
            .drafts
            .keys()
            .into_iter()
            .filter(|k| !open.contains(k))
            .collect();

        if candidates.is_empty() {
            return;
        }

        let client = self.client.clone();
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let mut closed = Vec::new();
            for (repo, pr_number) in candidates {
                // Only delete when GitHub confirms the PR is no longer open —
                // a PR missing from the list is not proof on its own.
                if let Ok(state) = client.fetch_pr_state(&repo, pr_number).await {
                    if state != "open" {
                        closed.push((repo, pr_number));
                    }
                }
            }
            if !closed.is_empty() {
                let _ = tx.send(BgMsg::DraftsPruned(closed));
            }
        });
    }

    /// Queue an AI review for the selected PR, whatever the automatic rules
    /// say. This is the way to retry a failed review, or to take another look
    /// at a PR you've already approved.
    pub fn rerun_ai_review(&mut self) {
        let Some(fpr) = self.selected_flat_pr() else { return };
        let repo = fpr.repo_name.clone();
        let pr = fpr.pr.clone();
        let key = (repo.clone(), pr.number);

        // Already in flight — nothing to do.
        if self.auto_running.contains(&key)
            || self
                .auto_queue
                .iter()
                .any(|(r, p)| *r == key.0 && p.number == pr.number)
        {
            return;
        }

        let draft = self
            .drafts
            .get_or_create(&repo, pr.number, &pr.title, &pr.head.sha);
        draft.pr_title = pr.title.clone();
        draft.head_sha = pr.head.sha.clone();
        draft.review_state = ReviewState::Queued;
        draft.review_error = None;
        self.drafts.save(&repo, pr.number);

        self.auto_queue.push_back((repo, pr));
        self.pump_auto_queue();
    }

    /// Reviews running right now, and reviews waiting to run.
    pub fn auto_progress(&self) -> (usize, usize) {
        (self.auto_running.len(), self.auto_queue.len())
    }

    /// Is an AI review running for this PR at this moment?
    pub fn is_reviewing(&self, repo: &str, pr_number: u64) -> bool {
        self.auto_running.contains(&(repo.to_string(), pr_number))
    }

    /// Write the diff view's draft state to disk if it changed.
    pub fn persist_draft_if_dirty(&mut self) {
        let Some(dv) = &self.diff_view else { return };
        if !dv.dirty {
            return;
        }
        let repo = dv.repo_name.clone();
        let pr_number = dv.pr_number;
        let title = self
            .selected_pr()
            .filter(|p| p.number == pr_number)
            .map(|p| p.title.clone())
            .unwrap_or_default();
        let head_sha = self
            .selected_pr()
            .filter(|p| p.number == pr_number)
            .map(|p| p.head.sha.clone())
            .unwrap_or_default();

        let draft = self
            .drafts
            .get_or_create(&repo, pr_number, &title, &head_sha);
        if let Some(dv) = &self.diff_view {
            crate::draft::capture_from_view(dv, draft);
        }
        self.drafts.save(&repo, pr_number);

        if let Some(dv) = &mut self.diff_view {
            dv.dirty = false;
        }
    }

    /// Is anything on screen currently moving? Drives how hard the main loop
    /// works: when nothing is animating it can sit idle instead of redrawing.
    pub fn is_animating(&self) -> bool {
        self.loading
            || self.loading_diff
            || !self.auto_running.is_empty()
            || !self.auto_queue.is_empty()
            || self.is_fetching()
            || self
                .diff_view
                .as_ref()
                .map_or(false, |dv| dv.loading_review)
            || self.approve_popup.as_ref().map_or(false, |p| p.submitting)
            || self.comment_popup.as_ref().map_or(false, |p| p.submitting)
    }

    /// Returns whether any background message was handled, so the caller
    /// knows the screen needs repainting.
    pub fn process_bg_messages(&mut self) -> bool {
        let mut handled = false;
        while let Ok(msg) = self.bg_rx.try_recv() {
            handled = true;
            match msg {
                BgMsg::UserLoaded(user) => {
                    self.username = user;
                    // The username gates review eligibility, so anything that
                    // arrived before it is worth re-checking now.
                    self.schedule_auto_reviews();
                }
                BgMsg::AssignedLoaded(repos) => {
                    self.assigned_repos = repos;
                    self.apply_filter();
                    self.loading = false;
                    self.request_all_statuses();
                    self.last_poll = Instant::now();
                    self.schedule_auto_reviews();
                    self.spawn_draft_prune();
                }
                BgMsg::AssignedRefreshed(repos) => {
                    self.polling = false;
                    if !repos.is_empty() {
                        self.apply_refresh(repos);
                    }
                }
                BgMsg::AutoReviewDone {
                    repo,
                    pr_number,
                    head_sha,
                    result,
                } => {
                    self.auto_running.remove(&(repo.clone(), pr_number));

                    let title = self
                        .flat_prs
                        .iter()
                        .find(|f| f.repo_name == repo && f.pr.number == pr_number)
                        .map(|f| f.pr.title.clone())
                        .unwrap_or_default();

                    let draft = self
                        .drafts
                        .get_or_create(&repo, pr_number, &title, &head_sha);
                    match result {
                        Ok(comments) => draft.apply_review(comments, head_sha),
                        Err(e) => {
                            draft.head_sha = head_sha;
                            draft.review_state = ReviewState::Failed;
                            draft.review_error = Some(e);
                        }
                    }
                    let updated = draft.clone();
                    self.drafts.save(&repo, pr_number);

                    // If the reviewed PR is the one on screen, show the
                    // findings straight away.
                    if let Some(dv) = &mut self.diff_view {
                        if dv.repo_name == repo && dv.pr_number == pr_number {
                            crate::draft::apply_to_view(&updated, dv);
                        }
                    }

                    self.pump_auto_queue();
                }
                BgMsg::DraftsPruned(closed) => {
                    for (repo, pr_number) in closed {
                        self.drafts.remove(&repo, pr_number);
                    }
                }
                BgMsg::AllPrsLoaded(repos) => {
                    self.all_repos = repos;
                    self.all_repos_loaded = true;
                    if !self.show_assigned_only {
                        self.apply_filter();
                        self.request_all_statuses();
                    }
                }
                BgMsg::Error(e) => {
                    self.error = Some(e);
                    self.loading = false;
                }
                BgMsg::StatusesLoaded(statuses) => {
                    for (key, status) in statuses {
                        self.pr_statuses.insert(key, status);
                    }
                    // Re-partition PRs now that approval info is available
                    self.recompute_approved_separator();
                    // Eligibility depends on approval, which we only just
                    // learned, so PRs held back earlier can be scheduled now.
                    self.schedule_auto_reviews();
                }
                BgMsg::CommitsLoaded((repo, pr_num, commits)) => {
                    // Only apply if this is still for the selected PR
                    if self.selected_repo_name().as_deref() == Some(&repo)
                        && self.selected_pr().map(|p| p.number) == Some(pr_num)
                    {
                        if !commits.is_empty() {
                            let end = commits.len() - 1;
                            self.pr_commits = commits;
                            self.commit_range = Some((0, end));
                        }
                    }
                }
                BgMsg::DiffLoaded(diff) => {
                    if self.active_tab == Tab::Diff {
                        let repo = self.selected_repo_name().unwrap_or_default();
                        let pr_num = self.selected_pr().map(|p| p.number).unwrap_or(0);
                        // Preserve existing threads/claude comments and drafts when rebuilding
                        let (existing_threads, existing_claude, existing_drafts, existing_resolves) =
                            if let Some(dv) = &self.diff_view {
                                (
                                    dv.threads.clone(),
                                    dv.claude_comments.clone(),
                                    dv.draft_comments.clone(),
                                    dv.pending_resolves.clone(),
                                )
                            } else {
                                (Vec::new(), Vec::new(), Vec::new(), Vec::new())
                            };
                        let mut dv = DiffView::new(&diff, repo, pr_num);
                        // Apply any buffered threads/claude comments (from initial load)
                        if let Some(threads) = self.pending_threads.take() {
                            dv.set_threads(threads);
                        } else if !existing_threads.is_empty() {
                            dv.set_threads(existing_threads);
                        }
                        if let Some(comments) = self.pending_claude.take() {
                            dv.set_claude_comments(comments);
                        } else if !existing_claude.is_empty() {
                            dv.set_claude_comments(existing_claude);
                        }
                        dv.draft_comments = existing_drafts;
                        dv.pending_resolves = existing_resolves;
                        // Fresh open: restore whatever was saved locally for
                        // this PR (AI findings, decisions, own draft comments).
                        if let Some(saved) = self.pending_draft.take() {
                            crate::draft::apply_to_view(&saved, &mut dv);
                        }
                        // Set tree_index to first file (skip dirs) and sync
                        self.tree_index = dv.tree.iter()
                            .position(|n| !n.is_dir)
                            .unwrap_or(0);
                        dv.tree_select(self.tree_index);
                        dv.ensure_highlighted(&self.highlighter);
                        self.diff_view = Some(dv);
                    }
                    self.current_diff = Some(diff);
                    self.loading_diff = false;
                }
                BgMsg::ThreadsLoaded(threads) => {
                    if let Some(dv) = &mut self.diff_view {
                        dv.set_threads(threads);
                    } else {
                        self.pending_threads = Some(threads);
                    }
                }
                BgMsg::ClaudeReviewOutput(chunk) => {
                    if let Some(dv) = &mut self.diff_view {
                        dv.review_output.push_str(&chunk);
                        // Auto-scroll to bottom — use saturating large value
                        let line_count = dv.review_output.lines().count() as u16;
                        dv.review_scroll = line_count;
                    }
                }
                BgMsg::ClaudeReviewParsed(comments) => {
                    if let Some(dv) = &mut self.diff_view {
                        // Keep decisions made on findings the re-review
                        // reported again.
                        let merged =
                            crate::draft::merge_ai_comments(&dv.claude_comments, comments);
                        dv.set_claude_comments(merged);
                        dv.loading_review = false;
                        dv.dirty = true;
                    } else {
                        self.pending_claude = Some(comments);
                    }
                    self.persist_draft_if_dirty();
                }
                BgMsg::SubmitResult(result) => {
                    match result {
                        Ok((_count, msg)) => {
                            // Everything pending is on GitHub now. Accepted
                            // findings come back as real threads, so drop them
                            // and keep only the discarded ones.
                            let submitted = self.diff_view.as_ref().map(|dv| (dv.repo_name.clone(), dv.pr_number));
                            if let Some(dv) = &mut self.diff_view {
                                dv.draft_comments.clear();
                                dv.pending_resolves.clear();
                                dv.pending_resolve_ids.clear();
                                dv.claude_comments.retain(|c| c.accepted == Some(false));
                                dv.remap_thread_refs();
                                dv.submit_status = Some(msg);
                                dv.dirty = false;
                            }
                            if let Some((repo, pr_number)) = submitted {
                                if let Some(draft) = self.drafts.get(&repo, pr_number).cloned() {
                                    let mut draft = draft;
                                    draft.mark_submitted();
                                    self.drafts.upsert(draft);
                                }
                            }
                            if let (Some(repo), Some(pr)) = (self.selected_repo_name(), self.selected_pr().cloned()) {
                                self.fetch_threads(&repo, &pr);
                            }
                        }
                        Err(e) => {
                            if let Some(dv) = &mut self.diff_view {
                                dv.submit_status = Some(format!("Submit failed: {}", e));
                            }
                        }
                    }
                }
                BgMsg::ApproveResult(result) => {
                    if let Some(popup) = &mut self.approve_popup {
                        popup.submitting = false;
                        match result {
                            Ok(()) => {
                                popup.result_msg = Some("✓ PR approved!".to_string());
                                // Re-fetch status so the PR list updates
                                let repo = popup.repo_name.clone();
                                let pr_number = popup.pr_number;
                                self.status_requested.remove(&repo);
                                if let Some(key) = self.pr_statuses.keys()
                                    .find(|(r, n)| r == &repo && *n == pr_number)
                                    .cloned()
                                {
                                    self.pr_statuses.remove(&key);
                                }
                                self.request_all_statuses();
                            }
                            Err(e) => popup.result_msg = Some(format!("✗ Failed: {}", e)),
                        }
                    }
                }
                BgMsg::CommentResult(result) => {
                    if let Some(popup) = &mut self.comment_popup {
                        popup.submitting = false;
                        match result {
                            Ok(()) => {
                                popup.result_msg = Some("✓ Comment posted!".to_string());
                            }
                            Err(e) => popup.result_msg = Some(format!("✗ Failed: {}", e)),
                        }
                    }
                }
            }
        }
        handled
    }

    pub fn refresh(&mut self) {
        self.loading = true;
        self.error = None;
        self.pr_statuses.clear();
        self.status_requested.clear();
        self.current_diff = None;
        self.diff_pr_key = None;
        self.all_repos_loaded = false;
        self.assigned_repos.clear();
        self.all_repos.clear();
        self.start_loading();
    }
}

/// Run the configured AI review command for one PR and parse its findings.
///
/// When `progress` is given the raw output is streamed back as
/// `ClaudeReviewOutput` for the review popup; background auto-reviews pass
/// `None` so they stay silent.
async fn run_review_process(
    config: &Config,
    pr_url: &str,
    progress: Option<&mpsc::UnboundedSender<BgMsg>>,
) -> Result<Vec<ClaudeComment>, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    let emit = |chunk: String| {
        if let Some(tx) = progress {
            let _ = tx.send(BgMsg::ClaudeReviewOutput(chunk));
        }
    };

    let expanded_args = config.expand_args(pr_url);

    let mut child = Command::new(&config.ai.command)
        .args(&expanded_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to run `{}`: {}", config.ai.command, e))?;

    let mut review_text = String::new();

    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();

        if config.ai.output_mode == "stream-json" {
            // Claude CLI streaming JSON mode
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                match val.get("type").and_then(|t| t.as_str()) {
                    Some("stream_event") => {
                        if let Some(delta) = val.pointer("/event/delta/text").and_then(|t| t.as_str())
                        {
                            review_text.push_str(delta);
                            emit(delta.to_string());
                        }
                        if let Some(tool) = val
                            .pointer("/event/content_block/name")
                            .and_then(|t| t.as_str())
                        {
                            emit(format!("\n-> {} ", tool));
                        }
                        if let Some(input) = val
                            .pointer("/event/delta/partial_json")
                            .and_then(|t| t.as_str())
                        {
                            emit(input.to_string());
                        }
                    }
                    Some("result") => {
                        if let Some(r) = val.get("result").and_then(|r| r.as_str()) {
                            if !r.is_empty() {
                                review_text = r.to_string();
                            }
                        }
                    }
                    Some("assistant") => {
                        if let Some(arr) = val.pointer("/message/content").and_then(|c| c.as_array())
                        {
                            for block in arr {
                                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                        if !text.is_empty() {
                                            review_text = text.to_string();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        } else {
            // Text mode: collect all stdout
            while let Ok(Some(line)) = lines.next_line().await {
                review_text.push_str(&line);
                review_text.push('\n');
                emit(format!("{}\n", line));
            }
        }
    }

    let status = child.wait().await;

    // Parse JSON from the review output (after the marker)
    let marker = &config.ai.json_marker;
    let parsed = if let Some(pos) = review_text.find(marker) {
        parse_claude_comments(review_text[pos + marker.len()..].trim())
    } else {
        parse_claude_comments_from_text(&review_text)
    };

    // No findings and a failed process means the review didn't happen, which
    // must not be mistaken for a clean review.
    if parsed.is_empty() {
        match status {
            Ok(s) if !s.success() => {
                return Err(format!("{} exited with {}", config.ai.command, s));
            }
            Err(e) => return Err(format!("{} failed: {}", config.ai.command, e)),
            _ => {}
        }
        if review_text.trim().is_empty() {
            return Err(format!("{} produced no output", config.ai.command));
        }
    }

    // Drop low-severity noise last, so an emptied list can't be mistaken for
    // a crashed agent by the checks above.
    Ok(filter_by_severity(parsed, config.min_severity_rank()))
}

/// Keep only findings at or above `min_rank`. Findings with no recognisable
/// severity are kept — better a stray comment than a silently dropped bug.
pub fn filter_by_severity(
    comments: Vec<ClaudeComment>,
    min_rank: Option<u8>,
) -> Vec<ClaudeComment> {
    let Some(min) = min_rank else { return comments };
    comments
        .into_iter()
        .filter(|c| {
            crate::config::severity_rank(c.severity.as_deref()).map_or(true, |r| r >= min)
        })
        .collect()
}

/// Print which PRs the background reviewer would pick up and why, without
/// starting the TUI or running any review. Invoked with `ghpr --check-auto`.
pub async fn dry_run_auto(client: GithubClient, config: Config) -> anyhow::Result<()> {
    let auto = config.auto.clone();
    let mut app = App::new(client.clone(), config);

    app.username = client.get_authenticated_user().await?;
    app.assigned_repos = client.fetch_my_prs().await?;

    // Eligibility depends on whether you've approved a PR, so the statuses
    // have to be loaded before the answers mean anything.
    let items: Vec<(String, PullRequest)> = app
        .assigned_repos
        .iter()
        .flat_map(|r| {
            r.pull_requests
                .iter()
                .map(|pr| (r.full_name.clone(), pr.clone()))
        })
        .collect();
    for (key, status) in client.fetch_statuses_batch(items).await {
        app.pr_statuses.insert(key, status);
    }

    println!("user: {}", app.username);
    if auto.enabled {
        println!(
            "auto: every {}s, {} at a time, only_assigned={}",
            auto.poll_interval_secs.max(15),
            auto.max_concurrent,
            auto.only_assigned
        );
    } else {
        println!("auto: OFF — nothing below will be reviewed automatically.");
        println!("      start ghpr with -r 5m to enable it, or press c on a PR.");
    }
    println!("drafts: {} stored in {}", app.drafts.keys().len(), DraftStore::dir_path().display());
    println!();

    let mut eligible = 0;
    for repo in &app.assigned_repos {
        for pr in &repo.pull_requests {
            let reason = app.auto_skip_reason(&repo.full_name, pr);
            let (mark, note) = match reason {
                None => {
                    eligible += 1;
                    let note = match app.drafts.get(&repo.full_name, pr.number) {
                        Some(d) if d.review_state == ReviewState::Failed => format!(
                            "FAILED: {}",
                            d.review_error.as_deref().unwrap_or("no reason recorded")
                        ),
                        Some(d) if d.head_sha == pr.head.sha => format!(
                            "reviewed ({:?}, {} findings, {} unsubmitted)",
                            d.review_state,
                            d.ai_comments.len(),
                            d.unsubmitted_count()
                        ),
                        Some(_) => "PR changed — will re-review".to_string(),
                        None => "will review".to_string(),
                    };
                    ("+", note)
                }
                Some(r) => ("-", format!("skipped: {}", r)),
            };
            println!(
                "{} {}#{:<6} {:<60} {}",
                mark,
                repo.full_name,
                pr.number,
                pr.title.chars().take(60).collect::<String>(),
                note
            );
        }
    }

    println!();
    println!("{} PR(s) in scope for automatic review", eligible);
    Ok(())
}

fn parse_claude_comments(stdout: &str) -> Vec<ClaudeComment> {
    let extract = |val: &serde_json::Value| -> Vec<ClaudeComment> {
        val.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        // Support both "file"/"filename" and "body"/"comment"
                        let file = c.get("filename").or_else(|| c.get("file"))?.as_str()?.to_string();
                        let line = c.get("line")?.as_u64()?;
                        let body = c.get("comment").or_else(|| c.get("body"))?.as_str()?.to_string();
                        let severity = c.get("severity").and_then(|s| s.as_str()).map(|s| s.to_string());
                        Some(ClaudeComment {
                            file,
                            line,
                            body,
                            severity,
                            accepted: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let trimmed = stdout.trim();

    // Helper: try to extract from any value that might contain comments
    let try_extract = |val: &serde_json::Value| -> Vec<ClaudeComment> {
        // Direct array
        let r = extract(val);
        if !r.is_empty() { return r; }
        // {"comments": [...]}
        if let Some(c) = val.get("comments") {
            let r = extract(c);
            if !r.is_empty() { return r; }
        }
        Vec::new()
    };

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // {"result": "..."} — string wrapper from --output-format json
        if let Some(result_str) = val.get("result").and_then(|r| r.as_str()) {
            if let Ok(inner) = serde_json::from_str::<serde_json::Value>(result_str) {
                let r = try_extract(&inner);
                if !r.is_empty() { return r; }
            }
        }
        // {"result": {...}} or {"result": [...]}
        if let Some(result_val) = val.get("result") {
            let r = try_extract(result_val);
            if !r.is_empty() { return r; }
        }
        // Top-level
        let r = try_extract(&val);
        if !r.is_empty() { return r; }
    }

    // Try to find JSON array in the text (maybe mixed with other output)
    if let Some(start) = trimmed.find('[') {
        // Find matching closing bracket
        let mut depth = 0;
        let mut end = start;
        for (i, ch) in trimmed[start..].char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        if end > start {
            let json_str = &trimmed[start..=end];
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                return extract(&val);
            }
        }
    }

    Vec::new()
}

/// Try to find and parse a JSON array from freeform text output
#[allow(dead_code)]
fn parse_claude_comments_from_text(text: &str) -> Vec<ClaudeComment> {
    if let Some(start) = text.rfind('[') {
        if let Some(end) = text[start..].rfind(']') {
            let json_str = &text[start..start + end + 1];
            return parse_claude_comments(json_str);
        }
    }
    Vec::new()
}

pub fn ci_icon(state: &CiState) -> &str {
    match state {
        CiState::Success => "\u{f00c}",
        CiState::Failure => "\u{f00d}",
        CiState::Pending => "\u{f110}",
        CiState::Unknown => "\u{f128}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, AutoConfig};

    /// Config whose "AI command" is a shell snippet we control.
    fn stub_config(script: &str) -> Config {
        Config {
            ai: AiConfig {
                name: "Stub".into(),
                command: "sh".into(),
                args: vec!["-c".into(), script.into()],
                json_marker: "---GHPR_JSON---".into(),
                output_mode: "text".into(),
                min_severity: "MEDIUM".into(),
            },
            auto: AutoConfig::default(),
        }
    }

    /// A PR with the given author/draft/reviewer shape.
    fn make_pr(number: u64, author: &str, draft: bool, reviewers: &[&str]) -> PullRequest {
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": "t",
            "state": "open",
            "draft": draft,
            "user": { "login": author },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "html_url": "https://example/pr",
            "requested_reviewers": reviewers
                .iter()
                .map(|r| serde_json::json!({ "login": r }))
                .collect::<Vec<_>>(),
            "assignees": [],
            "head": { "ref": "h", "sha": "sha1" },
            "base": { "ref": "b", "sha": "sha0" },
        }))
        .expect("valid PR fixture")
    }

    fn status_with_review(user: &str, state: &str) -> crate::github::PrStatus {
        crate::github::PrStatus {
            reviews: vec![crate::github::Review {
                user: crate::github::GhUser { login: user.into() },
                state: state.into(),
            }],
            ci_state: crate::github::CiState::Success,
            comments: Vec::new(),
            review_comments: Vec::new(),
            files: Vec::new(),
        }
    }

    /// An App with an isolated draft directory, so tests never touch the
    /// user's real drafts.
    fn test_app() -> App {
        let dir = std::env::temp_dir().join(format!("ghpr-app-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("GHPR_DRAFT_DIR", &dir);

        let client = GithubClient::new("test-token".into()).unwrap();
        let mut app = App::new(client, Config::default());
        app.username = "me".into();
        app
    }

    #[test]
    fn approved_prs_are_not_auto_reviewed() {
        let mut app = test_app();
        let pr = make_pr(1, "someone", false, &["me"]);
        let key = ("o/r".to_string(), 1);

        // Not yet approved -> eligible.
        app.pr_statuses
            .insert(key.clone(), status_with_review("someone-else", "APPROVED"));
        assert_eq!(app.auto_skip_reason("o/r", &pr), None);

        // Approved by me -> skipped.
        app.pr_statuses
            .insert(key, status_with_review("me", "APPROVED"));
        assert_eq!(
            app.auto_skip_reason("o/r", &pr),
            Some("already approved by you")
        );
    }

    #[test]
    fn scheduling_waits_until_approval_is_known() {
        let mut app = test_app();
        let pr = make_pr(2, "someone", false, &["me"]);

        // No status loaded yet: hold off rather than risk reviewing something
        // that turns out to be approved.
        assert_eq!(
            app.auto_skip_reason("o/r", &pr),
            Some("waiting for review status")
        );
        app.assigned_repos = vec![RepoInfo {
            full_name: "o/r".into(),
            pull_requests: vec![pr.clone()],
        }];
        app.schedule_auto_reviews();
        assert_eq!(app.auto_progress(), (0, 0), "nothing scheduled yet");

        // Once the status arrives it becomes eligible.
        app.pr_statuses.insert(
            ("o/r".to_string(), 2),
            status_with_review("other", "COMMENTED"),
        );
        assert_eq!(app.auto_skip_reason("o/r", &pr), None);
    }

    #[test]
    fn a_prior_draft_does_not_override_approval() {
        let mut app = test_app();
        let pr = make_pr(3, "someone", false, &["me"]);
        app.pr_statuses
            .insert(("o/r".to_string(), 3), status_with_review("me", "APPROVED"));

        // An engaged draft normally keeps a PR in scope; approval still wins.
        let draft = app.drafts.get_or_create("o/r", 3, "t", "sha1");
        draft.review_state = crate::draft::ReviewState::Done;
        assert_eq!(
            app.auto_skip_reason("o/r", &pr),
            Some("already approved by you")
        );
    }

    #[tokio::test]
    async fn rerun_queues_a_review_the_rules_would_skip() {
        let mut app = test_app();
        // Starting a review spawns the configured command; keep it harmless.
        app.config = stub_config("true");
        // Approved, so automatic scheduling refuses it.
        let pr = make_pr(4, "someone", false, &["me"]);
        app.pr_statuses
            .insert(("o/r".to_string(), 4), status_with_review("me", "APPROVED"));
        app.assigned_repos = vec![RepoInfo {
            full_name: "o/r".into(),
            pull_requests: vec![pr],
        }];
        app.apply_filter();
        app.pr_index = 0;

        app.schedule_auto_reviews();
        assert_eq!(app.auto_progress(), (0, 0), "not scheduled automatically");

        // Explicitly asking for it overrides that, and clears a stale error.
        let draft = app.drafts.get_or_create("o/r", 4, "t", "sha1");
        draft.review_state = crate::draft::ReviewState::Failed;
        draft.review_error = Some("boom".into());

        app.rerun_ai_review();
        let (running, queued) = app.auto_progress();
        assert_eq!(running + queued, 1, "explicit rerun is queued");
        let draft = app.drafts.get("o/r", 4).unwrap();
        assert!(draft.review_error.is_none(), "stale error cleared");
        assert_ne!(draft.review_state, crate::draft::ReviewState::Failed);
    }

fn finding(sev: Option<&str>) -> ClaudeComment {
        ClaudeComment {
            file: "a.rs".into(),
            line: 1,
            body: format!("{:?}", sev),
            severity: sev.map(|s| s.to_string()),
            accepted: None,
        }
    }

    fn kept(comments: Vec<ClaudeComment>, min: Option<u8>) -> Vec<String> {
        filter_by_severity(comments, min)
            .into_iter()
            .map(|c| c.severity.unwrap_or_else(|| "<none>".into()))
            .collect()
    }

    #[test]
    fn medium_threshold_drops_low_and_info() {
        let min = Config::default().min_severity_rank();
        assert_eq!(min, Some(2), "default threshold is MEDIUM");

        let all = vec![
            finding(Some("CRITICAL")),
            finding(Some("HIGH")),
            finding(Some("MEDIUM")),
            finding(Some("LOW")),
            finding(Some("INFO")),
        ];
        assert_eq!(kept(all, min), vec!["CRITICAL", "HIGH", "MEDIUM"]);
    }

    #[test]
    fn severity_matching_ignores_case_and_padding() {
        let min = Some(2);
        let all = vec![finding(Some(" high ")), finding(Some("low"))];
        assert_eq!(kept(all, min), vec![" high "]);
    }

    #[test]
    fn unlabelled_findings_are_never_dropped() {
        // An agent that omits severity must not have every finding discarded.
        let all = vec![finding(None), finding(Some("wat")), finding(Some("LOW"))];
        assert_eq!(kept(all, Some(2)), vec!["<none>", "wat"]);
    }

    #[test]
    fn info_threshold_keeps_everything() {
        let mut config = Config::default();
        config.ai.min_severity = "INFO".into();
        let all = vec![finding(Some("INFO")), finding(Some("LOW"))];
        assert_eq!(kept(all, config.min_severity_rank()), vec!["INFO", "LOW"]);
    }

    #[test]
    fn unparseable_threshold_disables_the_filter() {
        let mut config = Config::default();
        config.ai.min_severity = "banana".into();
        assert_eq!(config.min_severity_rank(), None);
        let all = vec![finding(Some("INFO"))];
        assert_eq!(kept(all, config.min_severity_rank()), vec!["INFO"]);
    }

    #[test]
    fn the_agent_is_told_which_severities_to_report() {
        let prompt = Config::default().system_prompt();
        assert!(prompt.contains("CRITICAL, HIGH, MEDIUM"), "{}", prompt);
        assert!(
            !prompt.contains("CRITICAL, HIGH, MEDIUM, LOW"),
            "LOW must not be offered at the default threshold: {}",
            prompt
        );
        assert!(prompt.contains("less important than MEDIUM"), "{}", prompt);

        let mut config = Config::default();
        config.ai.min_severity = "INFO".into();
        let prompt = config.system_prompt();
        assert!(prompt.contains("CRITICAL, HIGH, MEDIUM, LOW, INFO"));
        assert!(
            !prompt.contains("less important than"),
            "no threshold wording when nothing is filtered"
        );
    }

    #[tokio::test]
    async fn low_severity_findings_never_reach_the_review_result() {
        let config = stub_config(
            "echo '---GHPR_JSON---'; \
             echo '[{\"filename\":\"a.rs\",\"line\":1,\"severity\":\"LOW\",\"comment\":\"nit\"},\
                    {\"filename\":\"a.rs\",\"line\":2,\"severity\":\"HIGH\",\"comment\":\"bug\"}]'",
        );
        let found = run_review_process(&config, "https://example/pr/1", None)
            .await
            .expect("review should succeed");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].body, "bug");
    }

    #[tokio::test]
    async fn a_crash_still_reports_an_error_after_filtering() {
        // Filtering must not turn a failed run into a silent clean review.
        let mut config = stub_config("exit 3");
        config.ai.min_severity = "MEDIUM".into();
        let err = run_review_process(&config, "https://example/pr/1", None)
            .await
            .expect_err("crash must surface");
        assert!(err.contains("exited with"), "{}", err);
    }

    #[tokio::test]
    async fn review_process_parses_findings_after_marker() {
        let config = stub_config(
            "echo 'thinking out loud'; \
             echo '---GHPR_JSON---'; \
             echo '[{\"filename\":\"src/a.rs\",\"line\":12,\"severity\":\"HIGH\",\"comment\":\"leak\"},\
                    {\"filename\":\"src/b.rs\",\"line\":3,\"severity\":\"MEDIUM\",\"comment\":\"nit\"}]'",
        );
        let found = run_review_process(&config, "https://example/pr/1", None)
            .await
            .expect("stub review should succeed");

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].file, "src/a.rs");
        assert_eq!(found[0].line, 12);
        assert_eq!(found[0].severity.as_deref(), Some("HIGH"));
        assert_eq!(found[0].body, "leak");
        assert!(found[0].accepted.is_none(), "findings start undecided");
    }

    #[tokio::test]
    async fn review_process_accepts_empty_finding_list() {
        let config = stub_config("echo '---GHPR_JSON---'; echo '[]'");
        let found = run_review_process(&config, "https://example/pr/1", None)
            .await
            .expect("a clean review is not an error");
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn review_process_reports_failure_instead_of_a_clean_review() {
        // A crashed agent must not look like "no problems found".
        let config = stub_config("exit 3");
        let err = run_review_process(&config, "https://example/pr/1", None)
            .await
            .expect_err("non-zero exit with no findings must fail");
        assert!(err.contains("exited with"), "unexpected error: {}", err);
    }

    #[tokio::test]
    async fn review_process_reports_missing_command() {
        let mut config = stub_config("true");
        config.ai.command = "ghpr-no-such-binary".into();
        let err = run_review_process(&config, "https://example/pr/1", None)
            .await
            .expect_err("missing binary must fail");
        assert!(err.contains("failed to run"), "unexpected error: {}", err);
    }
}
