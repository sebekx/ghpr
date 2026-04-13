use crate::config::Config;
use crate::diff_view::{ClaudeComment, DiffView};
use crate::github::{CiState, Commit, GithubClient, PrStatus, PullRequest, RepoInfo};
use crate::highlight::Highlighter;
use std::collections::HashMap;
use tokio::sync::mpsc;

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
    AllPrsLoaded(Vec<RepoInfo>),
    Error(String),
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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfirmQuit {
    /// Quit the entire app
    App,
    /// Close diff view back to PR list
    CloseDiff,
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

    /// Whether the diff view has unsaved draft comments or pending resolves
    pub fn has_pending_drafts(&self) -> bool {
        if let Some(dv) = &self.diff_view {
            !dv.draft_comments.is_empty() || !dv.pending_resolves.is_empty()
        } else {
            false
        }
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
            use tokio::io::{AsyncBufReadExt, BufReader};
            use tokio::process::Command;

            let pr_url = format!("https://github.com/{}/pull/{}", repo, pr_number);
            let ai_name = &config.ai.name;
            let _ = tx.send(BgMsg::ClaudeReviewOutput(
                format!("Running {} review on {}...\n\n", ai_name, pr_url),
            ));

            let expanded_args = config.expand_args(&pr_url);

            let result = Command::new(&config.ai.command)
                .args(&expanded_args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();

            let mut review_text = String::new();

            match result {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        let reader = BufReader::new(stdout);
                        let mut lines = reader.lines();

                        if config.ai.output_mode == "stream-json" {
                            // Claude CLI streaming JSON mode
                            while let Ok(Some(line)) = lines.next_line().await {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                                    match val.get("type").and_then(|t| t.as_str()) {
                                        Some("stream_event") => {
                                            if let Some(delta) = val
                                                .pointer("/event/delta/text")
                                                .and_then(|t| t.as_str())
                                            {
                                                review_text.push_str(delta);
                                                let _ = tx.send(BgMsg::ClaudeReviewOutput(delta.to_string()));
                                            }
                                            if let Some(tool) = val
                                                .pointer("/event/content_block/name")
                                                .and_then(|t| t.as_str())
                                            {
                                                let _ = tx.send(BgMsg::ClaudeReviewOutput(
                                                    format!("\n-> {} ", tool),
                                                ));
                                            }
                                            if let Some(input) = val
                                                .pointer("/event/delta/partial_json")
                                                .and_then(|t| t.as_str())
                                            {
                                                let _ = tx.send(BgMsg::ClaudeReviewOutput(input.to_string()));
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
                                            if let Some(content) = val.pointer("/message/content") {
                                                if let Some(arr) = content.as_array() {
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
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        } else {
                            // Text mode: collect all stdout
                            while let Ok(Some(line)) = lines.next_line().await {
                                review_text.push_str(&line);
                                review_text.push('\n');
                                let _ = tx.send(BgMsg::ClaudeReviewOutput(format!("{}\n", line)));
                            }
                        }
                    }
                    let _ = child.wait().await;
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::ClaudeReviewOutput(format!("Error: {}\n", e)));
                    let _ = tx.send(BgMsg::ClaudeReviewParsed(Vec::new()));
                    return;
                }
            }

            // Parse JSON from the review output (after marker)
            let marker = &config.ai.json_marker;
            let parsed = if let Some(pos) = review_text.find(marker) {
                let json_str = review_text[pos + marker.len()..].trim();
                parse_claude_comments(json_str)
            } else {
                parse_claude_comments_from_text(&review_text)
            };

            let count = parsed.len();
            let _ = tx.send(BgMsg::ClaudeReviewOutput(
                format!("\n\n--- Found {} inline comments ---\n", count),
            ));
            let _ = tx.send(BgMsg::ClaudeReviewParsed(parsed));
        });
    }

    pub fn process_bg_messages(&mut self) {
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BgMsg::UserLoaded(user) => {
                    self.username = user;
                }
                BgMsg::AssignedLoaded(repos) => {
                    self.assigned_repos = repos;
                    self.apply_filter();
                    self.loading = false;
                    self.request_all_statuses();
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
                        dv.set_claude_comments(comments);
                        dv.loading_review = false;
                    } else {
                        self.pending_claude = Some(comments);
                    }
                }
                BgMsg::SubmitResult(result) => {
                    match result {
                        Ok((_count, msg)) => {
                            // Clear drafts/resolves and re-fetch threads
                            if let Some(dv) = &mut self.diff_view {
                                dv.draft_comments.clear();
                                dv.pending_resolves.clear();
                                dv.submit_status = Some(msg);
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
