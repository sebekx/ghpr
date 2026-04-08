use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub user: GhUser,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub requested_reviewers: Vec<GhUser>,
    pub head: GitRef,
    pub base: GitRef,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changed_files: u64,
    #[serde(default)]
    pub mergeable: Option<bool>,
    #[serde(default)]
    pub mergeable_state: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhUser {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub user: GhUser,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<CheckRun>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CombinedStatus {
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub user: GhUser,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ReviewComment {
    #[serde(default)]
    pub id: u64,
    pub user: GhUser,
    pub body: String,
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub original_line: Option<u64>,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrFile {
    pub filename: String,
    pub status: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
}

#[derive(Debug, Clone)]
pub struct PrStatus {
    pub reviews: Vec<Review>,
    pub ci_state: CiState,
    pub comments: Vec<Comment>,
    pub review_comments: Vec<ReviewComment>,
    pub files: Vec<PrFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CiState {
    Pending,
    Success,
    Failure,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub full_name: String,
    pub pull_requests: Vec<PullRequest>,
}

#[derive(Clone)]
pub struct GithubClient {
    client: reqwest::Client,
    token: Arc<String>,
}

impl GithubClient {
    pub fn new(token: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self {
            client,
            token: Arc::new(token),
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let resp = self
            .client
            .get(url)
            .header(USER_AGENT, "ghpr-tui")
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github.v3+json")
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, body);
        }

        resp.json::<T>().await.context("Failed to parse response")
    }

    async fn get_text(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .header(USER_AGENT, "ghpr-tui")
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github.v3.diff")
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, body);
        }

        resp.text().await.context("Failed to read response")
    }

    /// Fetch all open PRs the authenticated user is involved in (parallelized)
    pub async fn fetch_my_prs(&self) -> Result<Vec<RepoInfo>> {
        // Fire both search queries concurrently
        let (prs_res, review_res) = tokio::join!(
            self.get::<SearchResult>(
                "https://api.github.com/search/issues?q=is:pr+is:open+involves:@me&per_page=100&sort=updated"
            ),
            self.get::<SearchResult>(
                "https://api.github.com/search/issues?q=is:pr+is:open+review-requested:@me&per_page=100&sort=updated"
            )
        );

        let prs = prs_res.map(|r| r.items).unwrap_or_default();
        let review_prs = review_res.map(|r| r.items).unwrap_or_default();

        // Merge and deduplicate
        let mut seen = std::collections::HashSet::new();
        let mut all_prs = Vec::new();
        for pr in prs.into_iter().chain(review_prs) {
            let key = (pr.repository_url.clone(), pr.number);
            if seen.insert(key) {
                all_prs.push(pr);
            }
        }

        // Group by repository
        let mut repos: HashMap<String, Vec<SearchPr>> = HashMap::new();
        for pr in all_prs {
            let repo_name = pr
                .repository_url
                .trim_start_matches("https://api.github.com/repos/")
                .to_string();
            repos.entry(repo_name).or_default().push(pr);
        }

        // Fetch all PR details in parallel (all repos, all PRs at once)
        let mut futures = Vec::new();
        for (repo_name, prs) in &repos {
            for pr in prs {
                let client = self.clone();
                let url = format!(
                    "https://api.github.com/repos/{}/pulls/{}",
                    repo_name, pr.number
                );
                let repo_name = repo_name.clone();
                futures.push(tokio::spawn(async move {
                    let result = client.get::<PullRequest>(&url).await;
                    (repo_name, result)
                }));
            }
        }

        // Collect results
        let mut repo_prs: HashMap<String, Vec<PullRequest>> = HashMap::new();
        for handle in futures {
            if let Ok((repo_name, Ok(pr))) = handle.await {
                repo_prs.entry(repo_name).or_default().push(pr);
            }
        }

        let mut result: Vec<RepoInfo> = repo_prs
            .into_iter()
            .map(|(name, mut prs)| {
                prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                RepoInfo {
                    full_name: name,
                    pull_requests: prs,
                }
            })
            .collect();

        result.sort_by(|a, b| a.full_name.cmp(&b.full_name));
        Ok(result)
    }

    /// Fetch ALL open PRs for a list of repos (parallelized)
    pub async fn fetch_all_prs_for_repos(&self, repo_names: &[String]) -> Result<Vec<RepoInfo>> {
        let futures: Vec<_> = repo_names
            .iter()
            .map(|repo_name| {
                let client = self.clone();
                let repo_name = repo_name.clone();
                tokio::spawn(async move {
                    let url = format!(
                        "https://api.github.com/repos/{}/pulls?state=open&per_page=100&sort=updated",
                        repo_name
                    );
                    let prs: Vec<PullRequest> = client.get(&url).await.unwrap_or_default();
                    (repo_name, prs)
                })
            })
            .collect();

        let mut result = Vec::new();
        for handle in futures {
            if let Ok((repo_name, prs)) = handle.await {
                if !prs.is_empty() {
                    result.push(RepoInfo {
                        full_name: repo_name,
                        pull_requests: prs,
                    });
                }
            }
        }
        result.sort_by(|a, b| a.full_name.cmp(&b.full_name));
        Ok(result)
    }

    pub async fn fetch_pr_status(&self, repo: &str, pr: &PullRequest) -> Result<PrStatus> {
        // Fire reviews and check-runs concurrently
        let reviews_url = format!(
            "https://api.github.com/repos/{}/pulls/{}/reviews",
            repo, pr.number
        );
        let checks_url = format!(
            "https://api.github.com/repos/{}/commits/{}/check-runs",
            repo, pr.head.sha
        );

        let comments_url = format!(
            "https://api.github.com/repos/{}/issues/{}/comments?per_page=100",
            repo, pr.number
        );

        let (reviews_res, checks_res, comments_res) = tokio::join!(
            self.get::<Vec<Review>>(&reviews_url),
            self.get::<CheckRunsResponse>(&checks_url),
            self.get::<Vec<Comment>>(&comments_url),
        );

        let reviews = reviews_res.unwrap_or_default();
        let comments = comments_res.unwrap_or_default();
        // These are populated lazily when opening diff view
        let review_comments = Vec::new();
        let files = Vec::new();

        let ci_state = match checks_res {
            Ok(checks) => {
                if checks.check_runs.is_empty() {
                    // Fall back to status API
                    match self
                        .get::<CombinedStatus>(&format!(
                            "https://api.github.com/repos/{}/commits/{}/status",
                            repo, pr.head.sha
                        ))
                        .await
                    {
                        Ok(status) => match status.state.as_str() {
                            "success" => CiState::Success,
                            "failure" | "error" => CiState::Failure,
                            "pending" => CiState::Pending,
                            _ => CiState::Unknown,
                        },
                        Err(_) => CiState::Unknown,
                    }
                } else {
                    let all_complete = checks.check_runs.iter().all(|c| c.status == "completed");
                    if !all_complete {
                        CiState::Pending
                    } else {
                        let any_failure = checks.check_runs.iter().any(|c| {
                            c.conclusion.as_deref() == Some("failure")
                                || c.conclusion.as_deref() == Some("timed_out")
                        });
                        if any_failure {
                            CiState::Failure
                        } else {
                            CiState::Success
                        }
                    }
                }
            }
            Err(_) => CiState::Unknown,
        };

        Ok(PrStatus { reviews, ci_state, comments, review_comments, files })
    }

    /// Fetch statuses for multiple PRs in parallel
    pub async fn fetch_statuses_batch(
        &self,
        items: Vec<(String, PullRequest)>,
    ) -> Vec<((String, u64), PrStatus)> {
        let futures: Vec<_> = items
            .into_iter()
            .map(|(repo, pr)| {
                let client = self.clone();
                let pr_number = pr.number;
                let repo_clone = repo.clone();
                tokio::spawn(async move {
                    let result = client.fetch_pr_status(&repo_clone, &pr).await;
                    ((repo, pr_number), result)
                })
            })
            .collect();

        let mut results = Vec::new();
        for handle in futures {
            if let Ok((key, Ok(status))) = handle.await {
                results.push((key, status));
            }
        }
        results
    }

    pub async fn fetch_pr_diff(&self, repo: &str, pr_number: u64) -> Result<String> {
        self.get_text(&format!(
            "https://api.github.com/repos/{}/pulls/{}",
            repo, pr_number
        ))
        .await
    }

    /// Build review threads from REST API comments (fallback when GraphQL fails)
    pub async fn approve_pr(&self, repo: &str, pr_number: u64, comment: &str) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/pulls/{}/reviews",
            repo, pr_number
        );
        let mut body = serde_json::json!({ "event": "APPROVE" });
        if !comment.is_empty() {
            body["body"] = serde_json::Value::String(comment.to_string());
        }
        let resp = self
            .client
            .post(&url)
            .header(reqwest::header::USER_AGENT, "ghpr-tui")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(reqwest::header::ACCEPT, "application/vnd.github.v3+json")
            .json(&body)
            .send()
            .await
            .context("Failed to submit approval")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, text);
        }
        Ok(())
    }

    /// Submit a review with new comments and reply to existing threads
    pub async fn submit_review(
        &self,
        repo: &str,
        pr_number: u64,
        new_comments: Vec<(String, u64, String)>, // (path, line, body)
        replies: Vec<(u64, String)>,               // (comment_id, body)
    ) -> Result<()> {
        // Submit new comments as a review
        if !new_comments.is_empty() {
            let url = format!(
                "https://api.github.com/repos/{}/pulls/{}/reviews",
                repo, pr_number
            );
            let comments: Vec<serde_json::Value> = new_comments
                .iter()
                .map(|(path, line, body)| {
                    serde_json::json!({
                        "path": path,
                        "line": line,
                        "body": body,
                    })
                })
                .collect();
            let body = serde_json::json!({
                "event": "COMMENT",
                "comments": comments,
            });
            let resp = self
                .client
                .post(&url)
                .header(reqwest::header::USER_AGENT, "ghpr-tui")
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.token))
                .header(reqwest::header::ACCEPT, "application/vnd.github.v3+json")
                .json(&body)
                .send()
                .await
                .context("Failed to submit review")?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("GitHub API error {}: {}", status, text);
            }
        }

        // Submit replies individually
        for (comment_id, body) in &replies {
            let url = format!(
                "https://api.github.com/repos/{}/pulls/{}/comments/{}/replies",
                repo, pr_number, comment_id
            );
            let payload = serde_json::json!({ "body": body });
            let resp = self
                .client
                .post(&url)
                .header(reqwest::header::USER_AGENT, "ghpr-tui")
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.token))
                .header(reqwest::header::ACCEPT, "application/vnd.github.v3+json")
                .json(&payload)
                .send()
                .await
                .context("Failed to submit reply")?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("GitHub API error {}: {}", status, text);
            }
        }

        Ok(())
    }

    /// Resolve a review thread via GraphQL
    pub async fn resolve_thread(&self, thread_node_id: &str) -> Result<()> {
        let query = format!(
            r#"{{ "query": "mutation {{ resolveReviewThread(input: {{ threadId: \"{thread_node_id}\" }}) {{ thread {{ isResolved }} }} }}" }}"#
        );
        let resp = self
            .client
            .post("https://api.github.com/graphql")
            .header(reqwest::header::USER_AGENT, "ghpr-tui")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(query)
            .send()
            .await
            .context("GraphQL resolve failed")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, text);
        }
        let data: serde_json::Value = resp.json().await?;
        if let Some(errors) = data.get("errors") {
            anyhow::bail!("GraphQL error: {}", errors);
        }
        Ok(())
    }

    pub async fn build_threads_from_rest(
        &self,
        repo: &str,
        pr: &PullRequest,
    ) -> Vec<ReviewThread> {
        let url = format!(
            "https://api.github.com/repos/{}/pulls/{}/comments?per_page=100",
            repo, pr.number
        );
        let comments: Vec<ReviewComment> = self.get(&url).await.unwrap_or_default();

        // Group into threads: top-level comments (in_reply_to_id=None) start threads,
        // replies attach to their parent
        let mut thread_map: HashMap<u64, Vec<&ReviewComment>> = HashMap::new();
        let mut top_level: Vec<&ReviewComment> = Vec::new();

        for c in &comments {
            if c.in_reply_to_id.is_some() {
                thread_map
                    .entry(c.in_reply_to_id.unwrap())
                    .or_default()
                    .push(c);
            } else {
                top_level.push(c);
            }
        }

        top_level
            .iter()
            .map(|root| {
                let mut thread_comments = vec![ThreadComment {
                    id: root.id,
                    author: root.user.login.clone(),
                    body: root.body.clone(),
                    created_at: root.created_at.to_rfc3339(),
                }];
                if let Some(replies) = thread_map.get(&root.id) {
                    for r in replies {
                        thread_comments.push(ThreadComment {
                            id: r.id,
                            author: r.user.login.clone(),
                            body: r.body.clone(),
                            created_at: r.created_at.to_rfc3339(),
                        });
                    }
                }
                // Determine side: if line is set, it's the new file (RIGHT);
                // if only original_line, it's the old file (LEFT)
                let (target_line, side) = if root.line.is_some() {
                    (root.line, DiffSide::Right)
                } else {
                    (root.original_line, DiffSide::Left)
                };
                ReviewThread {
                    path: root.path.clone(),
                    line: target_line,
                    side,
                    is_resolved: false,
                    comments: thread_comments,
                    node_id: None,
                }
            })
            .collect()
    }

    pub async fn get_authenticated_user(&self) -> Result<String> {
        let user: GhUser = self.get("https://api.github.com/user").await?;
        Ok(user.login)
    }

    /// Fetch review threads with resolved status via GraphQL
    pub async fn fetch_review_threads(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<ReviewThread>> {
        let query = format!(
            r#"{{ "query": "query {{ repository(owner: \"{owner}\", name: \"{repo}\") {{ pullRequest(number: {pr_number}) {{ reviewThreads(first: 100) {{ nodes {{ id isResolved path line originalLine startLine originalStartLine diffSide comments(first: 50) {{ nodes {{ id databaseId body author {{ login }} createdAt }} }} }} }} }} }} }}" }}"#
        );

        let resp = self
            .client
            .post("https://api.github.com/graphql")
            .header(reqwest::header::USER_AGENT, "ghpr-tui")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(query)
            .send()
            .await
            .context("GraphQL request failed")?;

        let data: serde_json::Value = resp.json().await.context("Failed to parse GraphQL response")?;

        let threads = data
            .pointer("/data/repository/pullRequest/reviewThreads/nodes")
            .and_then(|n| n.as_array())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|node| {
                        let path = node.get("path")?.as_str()?.to_string();
                        let line = node.get("line").and_then(|l| l.as_u64())
                            .or_else(|| node.get("originalLine").and_then(|l| l.as_u64()))
                            .or_else(|| node.get("startLine").and_then(|l| l.as_u64()))
                            .or_else(|| node.get("originalStartLine").and_then(|l| l.as_u64()));
                        let side = match node.get("diffSide").and_then(|s| s.as_str()) {
                            Some("LEFT") => DiffSide::Left,
                            _ => DiffSide::Right,
                        };
                        let is_resolved = node
                            .get("isResolved")
                            .and_then(|r| r.as_bool())
                            .unwrap_or(false);
                        let comments = node
                            .pointer("/comments/nodes")
                            .and_then(|c| c.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|c| {
                                        Some(ThreadComment {
                                            id: c.get("databaseId").and_then(|i| i.as_u64()).unwrap_or(0),
                                            author: c
                                                .pointer("/author/login")
                                                .and_then(|a| a.as_str())
                                                .unwrap_or("unknown")
                                                .to_string(),
                                            body: c.get("body")?.as_str()?.to_string(),
                                            created_at: c
                                                .get("createdAt")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let node_id = node.get("id").and_then(|i| i.as_str()).map(|s| s.to_string());
                        Some(ReviewThread {
                            path,
                            line,
                            side,
                            is_resolved,
                            comments,
                            node_id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(threads)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffSide {
    Left,  // old file (removed lines)
    Right, // new file (added/context lines)
}

#[derive(Debug, Clone)]
pub struct ReviewThread {
    pub path: String,
    pub line: Option<u64>,
    pub side: DiffSide,
    pub is_resolved: bool,
    pub comments: Vec<ThreadComment>,
    /// GraphQL node ID for resolving the thread
    pub node_id: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ThreadComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    items: Vec<SearchPr>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct SearchPr {
    pub number: u64,
    pub repository_url: String,
    pub title: String,
}
