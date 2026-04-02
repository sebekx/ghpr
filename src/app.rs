use crate::diff_view::{ClaudeComment, DiffView};
use crate::github::{CiState, GithubClient, PrStatus, PullRequest, RepoInfo};
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
    ReviewLoaded(String),
    ReviewProgress(String),
    ThreadsLoaded(Vec<crate::github::ReviewThread>),
    ClaudeReviewParsed(Vec<ClaudeComment>),
    ApproveResult(Result<(), String>),
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
    pub review_popup: Option<ReviewPopup>,
    pub show_help: bool,
    pub diff_view: Option<DiffView>,
    pub tree_index: usize,
    pub diff_focus: DiffFocus,
    /// Buffered threads/claude results waiting for diff_view to be created
    pending_threads: Option<Vec<crate::github::ReviewThread>>,
    pending_claude: Option<Vec<ClaudeComment>>,
    pub file_pane_width: u16,
    pub approve_popup: Option<ApprovePopup>,
}

pub struct ApprovePopup {
    pub repo_name: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub comment: String,
    pub submitting: bool,
    pub result_msg: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffFocus {
    Files,
    Content,
}


pub struct ReviewPopup {
    pub title: String,
    pub content: String,
    pub scroll: u16,
    pub loading: bool,
}

impl App {
    pub fn new(client: GithubClient) -> Self {
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
            review_popup: None,
            show_help: false,
            diff_view: None,
            tree_index: 0,
            diff_focus: DiffFocus::Content,
            pending_threads: None,
            pending_claude: None,
            file_pane_width: 30,
            approve_popup: None,
        }
    }

    /// Flatten repos into a single PR list
    fn apply_filter(&mut self) {
        let repos = if self.show_assigned_only {
            &self.assigned_repos
        } else {
            &self.all_repos
        };

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
            .collect();

        // Sort by updated_at descending
        self.flat_prs.sort_by(|a, b| b.pr.updated_at.cmp(&a.pr.updated_at));

        self.pr_index = self.pr_index.min(self.flat_prs.len().saturating_sub(1));
        self.current_diff = None;
        self.diff_pr_key = None;
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

    pub fn request_claude_review(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else {
            return;
        };
        let Some(pr) = self.selected_pr() else {
            return;
        };
        let pr_number = pr.number;
        let title = format!("Claude Review — #{} {}", pr.number, pr.title);

        self.review_popup = Some(ReviewPopup {
            title,
            content: "Running claude /review-pr …\n".to_string(),
            scroll: 0,
            loading: true,
        });

        let tx = self.bg_tx.clone();
        let repo = repo_name.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            use tokio::process::Command;

            let pr_url = format!("https://github.com/{}/pull/{}", repo, pr_number);
            let prompt = format!("/review-pr {}", pr_url);

            let result = Command::new("claude")
                .args([
                    "-p",
                    &prompt,
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();

            match result {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        let reader = BufReader::new(stdout);
                        let mut lines = reader.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                                match val.get("type").and_then(|t| t.as_str()) {
                                    Some("stream_event") => {
                                        if let Some(delta_text) = val
                                            .pointer("/event/delta/text")
                                            .and_then(|t| t.as_str())
                                        {
                                            let _ = tx.send(BgMsg::ReviewProgress(
                                                delta_text.to_string(),
                                            ));
                                        }
                                        if let Some(tool_name) = val
                                            .pointer("/event/content_block/name")
                                            .and_then(|t| t.as_str())
                                        {
                                            let _ = tx.send(BgMsg::ReviewProgress(format!(
                                                "\n⚙ {} ",
                                                tool_name
                                            )));
                                        }
                                        if let Some(input_json) = val
                                            .pointer("/event/delta/partial_json")
                                            .and_then(|t| t.as_str())
                                        {
                                            let _ = tx.send(BgMsg::ReviewProgress(
                                                input_json.to_string(),
                                            ));
                                        }
                                    }
                                    Some("result") => {
                                        if let Some(result_text) =
                                            val.get("result").and_then(|r| r.as_str())
                                        {
                                            let _ = tx.send(BgMsg::ReviewLoaded(
                                                result_text.to_string(),
                                            ));
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    let _ = child.wait().await;
                    let _ = tx.send(BgMsg::ReviewLoaded(String::new()));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::ReviewLoaded(format!(
                        "Failed to run `claude`: {}\n\nMake sure Claude Code CLI is installed:\n  npm install -g @anthropic-ai/claude-code",
                        e
                    )));
                }
            }
        });
    }

    pub fn close_review_popup(&mut self) {
        self.review_popup = None;
    }

    pub fn show_approve_popup(&mut self) {
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr() else { return };
        self.approve_popup = Some(ApprovePopup {
            repo_name,
            pr_number: pr.number,
            pr_title: pr.title.clone(),
            comment: String::new(),
            submitting: false,
            result_msg: None,
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

    /// Open diff view (loads diff, threads, optionally Claude review)
    pub fn open_diff_view(&mut self, with_claude: bool) {
        self.diff_view = None;
        self.pending_threads = None;
        self.pending_claude = None;
        self.tree_index = 0;
        self.diff_focus = DiffFocus::Content;
        let Some(repo_name) = self.selected_repo_name() else { return };
        let Some(pr) = self.selected_pr().cloned() else { return };

        // Force re-fetch diff
        self.current_diff = None;
        self.diff_pr_key = None;
        self.request_diff();

        // Fetch threads in background (GraphQL with REST fallback)
        let parts: Vec<&str> = repo_name.split('/').collect();
        if parts.len() == 2 {
            let client = self.client.clone();
            let tx = self.bg_tx.clone();
            let owner = parts[0].to_string();
            let repo_short = parts[1].to_string();
            let repo_full = repo_name.clone();
            let pr_num = pr.number;
            let pr_clone = pr.clone();
            tokio::spawn(async move {
                // Try GraphQL first
                let threads = match client.fetch_review_threads(&owner, &repo_short, pr_num).await {
                    Ok(t) if !t.is_empty() => t,
                    _ => {
                        // Fallback: build threads from REST review comments
                        client.build_threads_from_rest(&repo_full, &pr_clone).await
                    }
                };
                let _ = tx.send(BgMsg::ThreadsLoaded(threads));
            });
        }

        // Run Claude structured review if requested
        if with_claude {
            self.run_structured_claude_review(&repo_name, &pr);
        }

        // DiffView will be created when DiffLoaded arrives
        self.active_tab = Tab::Diff;
    }

    fn run_structured_claude_review(&self, repo_name: &str, pr: &PullRequest) {
        let tx = self.bg_tx.clone();
        let pr_url = format!("https://github.com/{}/pull/{}", repo_name, pr.number);

        tokio::spawn(async move {
            use tokio::process::Command;

            let prompt = format!(
                r#"Review this GitHub pull request: {}
Analyze the code changes and provide review comments.
Output ONLY a JSON array where each element has:
- "file": the file path
- "line": line number in the new file
- "body": your review comment
Example: [{{"file":"src/main.rs","line":42,"body":"Consider handling the error case here"}}]
If no issues found, output an empty array: []"#,
                pr_url
            );

            let schema = r#"{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"line":{"type":"integer"},"body":{"type":"string"}},"required":["file","line","body"]}}"#;

            let result = Command::new("claude")
                .args(["-p", &prompt, "--json-schema", schema, "--output-format", "json"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .await;

            match result {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    // Parse the JSON result
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
                        // The --output-format json wraps in {"result": "..."}
                        let json_str = val.get("result")
                            .and_then(|r| r.as_str())
                            .unwrap_or(&stdout);

                        if let Ok(comments) = serde_json::from_str::<Vec<serde_json::Value>>(json_str) {
                            let parsed: Vec<ClaudeComment> = comments
                                .iter()
                                .filter_map(|c| {
                                    Some(ClaudeComment {
                                        file: c.get("file")?.as_str()?.to_string(),
                                        line: c.get("line")?.as_u64()?,
                                        body: c.get("body")?.as_str()?.to_string(),
                                        accepted: None,
                                    })
                                })
                                .collect();
                            let _ = tx.send(BgMsg::ClaudeReviewParsed(parsed));
                            return;
                        }
                    }
                    // Fallback: try direct parse
                    if let Ok(comments) = serde_json::from_str::<Vec<serde_json::Value>>(&stdout) {
                        let parsed: Vec<ClaudeComment> = comments
                            .iter()
                            .filter_map(|c| {
                                Some(ClaudeComment {
                                    file: c.get("file")?.as_str()?.to_string(),
                                    line: c.get("line")?.as_u64()?,
                                    body: c.get("body")?.as_str()?.to_string(),
                                    accepted: None,
                                })
                            })
                            .collect();
                        let _ = tx.send(BgMsg::ClaudeReviewParsed(parsed));
                    } else {
                        let _ = tx.send(BgMsg::ClaudeReviewParsed(Vec::new()));
                    }
                }
                Err(_) => {
                    let _ = tx.send(BgMsg::ClaudeReviewParsed(Vec::new()));
                }
            }
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
                }
                BgMsg::DiffLoaded(diff) => {
                    // If we're entering diff view, create the DiffView
                    if self.active_tab == Tab::Diff && self.diff_view.is_none() {
                        let repo = self.selected_repo_name().unwrap_or_default();
                        let pr_num = self.selected_pr().map(|p| p.number).unwrap_or(0);
                        let mut dv = DiffView::new(&diff, repo, pr_num);
                        // Apply any buffered threads/claude comments
                        if let Some(threads) = self.pending_threads.take() {
                            dv.set_threads(threads);
                        }
                        if let Some(comments) = self.pending_claude.take() {
                            dv.set_claude_comments(comments);
                        }
                        // Set tree_index to first file (skip dirs)
                        self.tree_index = dv.tree.iter()
                            .position(|n| !n.is_dir)
                            .unwrap_or(0);
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
                BgMsg::ClaudeReviewParsed(comments) => {
                    if let Some(dv) = &mut self.diff_view {
                        dv.set_claude_comments(comments);
                        dv.loading_review = false;
                    } else {
                        self.pending_claude = Some(comments);
                    }
                }
                BgMsg::ReviewProgress(chunk) => {
                    if let Some(popup) = &mut self.review_popup {
                        if popup.content.starts_with("Running claude") {
                            popup.content.clear();
                        }
                        popup.content.push_str(&chunk);
                    }
                }
                BgMsg::ReviewLoaded(result) => {
                    if let Some(popup) = &mut self.review_popup {
                        if !result.is_empty() {
                            popup.content = result;
                            popup.scroll = 0;
                        }
                        popup.loading = false;
                    }
                }
                BgMsg::ApproveResult(result) => {
                    if let Some(popup) = &mut self.approve_popup {
                        popup.submitting = false;
                        match result {
                            Ok(()) => popup.result_msg = Some("✓ PR approved!".to_string()),
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

pub fn ci_icon(state: &CiState) -> &str {
    match state {
        CiState::Success => "\u{f00c}",
        CiState::Failure => "\u{f00d}",
        CiState::Pending => "\u{f110}",
        CiState::Unknown => "\u{f128}",
    }
}
