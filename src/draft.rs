//! Local, on-disk review drafts.
//!
//! A draft holds everything about a PR review that has not been sent to GitHub
//! yet: the AI review findings, which of them you accepted or discarded, your
//! own pending comments and the threads you plan to resolve. It lives in
//! `~/.ghpr/drafts/<owner>__<repo>__<number>.json` so the state survives
//! restarts, and it is removed once the review is submitted or the PR closes.

use crate::diff_view::{ClaudeComment, DiffView, DraftComment};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the auto-review of a PR currently stands.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Draft exists but no AI review has run (e.g. created by a manual comment).
    NotRun,
    Queued,
    Running,
    Done,
    Failed,
}

impl Default for ReviewState {
    fn default() -> Self {
        ReviewState::NotRun
    }
}

/// An AI review finding plus your accept/discard decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAiComment {
    pub file: String,
    pub line: u64,
    pub body: String,
    #[serde(default)]
    pub severity: Option<String>,
    /// `None` = undecided, `Some(true)` = accepted, `Some(false)` = discarded.
    #[serde(default)]
    pub accepted: Option<bool>,
}

/// A comment of yours that has not been posted yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDraftComment {
    pub file: String,
    pub line: u64,
    pub body: String,
    /// Root comment id of the thread this replies to (`None` = new comment).
    /// Comment ids are stable across restarts, thread indices are not.
    #[serde(default)]
    pub reply_to_comment_id: Option<u64>,
    #[serde(default)]
    pub resolve: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrDraft {
    pub repo: String,
    pub pr_number: u64,
    #[serde(default)]
    pub pr_title: String,
    /// Head SHA the AI review ran against — a different SHA means the PR moved
    /// on and the review needs to run again.
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub review_state: ReviewState,
    #[serde(default)]
    pub review_error: Option<String>,
    #[serde(default)]
    pub reviewed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub submitted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ai_comments: Vec<SavedAiComment>,
    #[serde(default)]
    pub draft_comments: Vec<SavedDraftComment>,
    #[serde(default)]
    pub pending_resolve_ids: Vec<u64>,
}

impl PrDraft {
    pub fn new(repo: String, pr_number: u64, pr_title: String, head_sha: String) -> Self {
        Self {
            repo,
            pr_number,
            pr_title,
            head_sha,
            review_state: ReviewState::NotRun,
            review_error: None,
            reviewed_at: None,
            submitted_at: None,
            ai_comments: Vec::new(),
            draft_comments: Vec::new(),
            pending_resolve_ids: Vec::new(),
        }
    }

    /// AI findings you have not decided on yet.
    pub fn pending_ai_count(&self) -> usize {
        self.ai_comments.iter().filter(|c| c.accepted.is_none()).count()
    }

    /// Comments and resolves that would be sent on the next submit.
    /// Accepted AI findings already have a matching entry in `draft_comments`,
    /// so they are counted there rather than twice.
    pub fn unsubmitted_count(&self) -> usize {
        self.draft_comments.len() + self.pending_resolve_ids.len()
    }

    pub fn has_unsubmitted(&self) -> bool {
        self.unsubmitted_count() > 0
    }

    /// Does this draft represent real engagement with the PR, rather than a
    /// placeholder left by a review that never produced anything? Only an
    /// engaged draft keeps a PR in auto-review scope after GitHub drops you
    /// from its reviewers — otherwise an abandoned queue entry would opt a PR
    /// in forever.
    pub fn is_engaged(&self) -> bool {
        !self.ai_comments.is_empty()
            || !self.draft_comments.is_empty()
            || matches!(self.review_state, ReviewState::Done | ReviewState::Failed)
    }

    /// Is this draft worth keeping on disk?
    pub fn is_empty(&self) -> bool {
        self.ai_comments.is_empty()
            && self.draft_comments.is_empty()
            && self.pending_resolve_ids.is_empty()
            && self.review_state == ReviewState::NotRun
    }

    /// Fold a fresh AI review into the draft, keeping decisions you already made
    /// about findings that came back unchanged.
    pub fn apply_review(&mut self, new_comments: Vec<ClaudeComment>, head_sha: String) {
        let old: Vec<ClaudeComment> = self.ai_comments.iter().map(to_live_ai).collect();
        let merged = merge_ai_comments(&old, new_comments);
        self.ai_comments = merged.iter().map(to_saved_ai).collect();
        self.head_sha = head_sha;
        self.review_state = ReviewState::Done;
        self.review_error = None;
        self.reviewed_at = Some(Utc::now());
    }

    /// Everything pending has just been posted to GitHub. Accepted findings are
    /// real threads now, so drop them; keep the discarded ones so the same
    /// findings don't get re-suggested for this head SHA.
    pub fn mark_submitted(&mut self) {
        self.ai_comments.retain(|c| c.accepted == Some(false));
        self.draft_comments.clear();
        self.pending_resolve_ids.clear();
        self.submitted_at = Some(Utc::now());
    }
}

/// Carry accept/discard decisions from `old` onto `new`, matching on file and
/// comment text (the line number often shifts between revisions).
pub fn merge_ai_comments(old: &[ClaudeComment], new: Vec<ClaudeComment>) -> Vec<ClaudeComment> {
    let key = |c: &ClaudeComment| (c.file.clone(), c.body.trim().to_string());
    let mut decisions: HashMap<(String, String), Option<bool>> = HashMap::new();
    for c in old {
        if c.accepted.is_some() {
            decisions.insert(key(c), c.accepted);
        }
    }
    new.into_iter()
        .map(|mut c| {
            if let Some(&decision) = decisions.get(&key(&c)) {
                c.accepted = decision;
            }
            c
        })
        .collect()
}

fn to_live_ai(c: &SavedAiComment) -> ClaudeComment {
    ClaudeComment {
        file: c.file.clone(),
        line: c.line,
        body: c.body.clone(),
        severity: c.severity.clone(),
        accepted: c.accepted,
    }
}

fn to_saved_ai(c: &ClaudeComment) -> SavedAiComment {
    SavedAiComment {
        file: c.file.clone(),
        line: c.line,
        body: c.body.clone(),
        severity: c.severity.clone(),
        accepted: c.accepted,
    }
}

/// Load a stored draft into a freshly built diff view.
pub fn apply_to_view(draft: &PrDraft, dv: &mut DiffView) {
    dv.set_claude_comments(draft.ai_comments.iter().map(to_live_ai).collect());
    dv.draft_comments = draft
        .draft_comments
        .iter()
        .map(|c| DraftComment {
            file: c.file.clone(),
            line: c.line,
            body: c.body.clone(),
            in_reply_to_thread: None,
            reply_to_comment_id: c.reply_to_comment_id,
            resolve: c.resolve,
        })
        .collect();
    dv.pending_resolves.clear();
    dv.pending_resolve_ids = draft.pending_resolve_ids.clone();
    dv.remap_thread_refs();
}

/// Copy the live diff-view state back into a draft, ready to be written out.
pub fn capture_from_view(dv: &DiffView, draft: &mut PrDraft) {
    draft.ai_comments = dv.claude_comments.iter().map(to_saved_ai).collect();
    draft.draft_comments = dv
        .draft_comments
        .iter()
        .map(|d| SavedDraftComment {
            file: d.file.clone(),
            line: d.line,
            body: d.body.clone(),
            reply_to_comment_id: d.reply_to_comment_id.or_else(|| {
                d.in_reply_to_thread
                    .and_then(|ti| dv.threads.get(ti))
                    .and_then(|t| t.comments.first())
                    .map(|c| c.id)
            }),
            resolve: d.resolve,
        })
        .collect();

    let mut ids: Vec<u64> = dv
        .pending_resolves
        .iter()
        .filter_map(|&ti| dv.threads.get(ti))
        .filter_map(|t| t.comments.first())
        .map(|c| c.id)
        .collect();
    // Ids whose thread hasn't loaded yet must not be lost.
    ids.extend(dv.pending_resolve_ids.iter().copied());
    ids.sort_unstable();
    ids.dedup();
    draft.pending_resolve_ids = ids;
}

/// All drafts on disk, kept in memory and written through on every change.
pub struct DraftStore {
    dir: PathBuf,
    drafts: HashMap<(String, u64), PrDraft>,
}

impl DraftStore {
    /// Where drafts live. `GHPR_DRAFT_DIR` overrides the default, which keeps
    /// tests off your real drafts and lets you keep separate sets per machine.
    pub fn dir_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("GHPR_DRAFT_DIR") {
            return PathBuf::from(dir);
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ghpr")
            .join("drafts")
    }

    /// Read every draft in the store. Unreadable files are skipped rather than
    /// taking the app down.
    pub fn load() -> Self {
        let dir = Self::dir_path();
        let drafts = Self::load_from(&dir);
        Self { dir, drafts }
    }

    fn load_from(dir: &Path) -> HashMap<(String, u64), PrDraft> {
        let mut drafts = HashMap::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return drafts };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Ok(mut draft) = serde_json::from_str::<PrDraft>(&text) else { continue };
            // A review that was in flight when we exited never finished —
            // put it back in line.
            if draft.review_state == ReviewState::Running {
                draft.review_state = ReviewState::Queued;
            }
            drafts.insert((draft.repo.clone(), draft.pr_number), draft);
        }
        drafts
    }

    fn path_for(dir: &Path, repo: &str, pr_number: u64) -> PathBuf {
        let safe: String = repo
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' { c } else { '_' })
            .collect();
        dir.join(format!("{}__{}.json", safe, pr_number))
    }

    pub fn get(&self, repo: &str, pr_number: u64) -> Option<&PrDraft> {
        self.drafts.get(&(repo.to_string(), pr_number))
    }

    pub fn keys(&self) -> Vec<(String, u64)> {
        self.drafts.keys().cloned().collect()
    }

    /// Drop stored findings below `min_rank`.
    ///
    /// Accepted findings are kept whatever their severity — they have a queued
    /// comment pointing at them, and removing one would orphan it. Discarded
    /// ones go: they were only retained to stop the same finding being
    /// suggested again, and below the threshold it never will be.
    pub fn drop_below_severity(&mut self, min_rank: u8) {
        let mut changed: Vec<(String, u64)> = Vec::new();
        for (key, draft) in self.drafts.iter_mut() {
            let before = draft.ai_comments.len();
            draft.ai_comments.retain(|c| {
                c.accepted == Some(true)
                    || crate::config::severity_rank(c.severity.as_deref())
                        .map_or(true, |r| r >= min_rank)
            });
            if draft.ai_comments.len() != before {
                changed.push(key.clone());
            }
        }
        for (repo, pr_number) in changed {
            self.save(&repo, pr_number);
        }
    }

    /// Comments and resolves across all drafts that haven't been posted yet,
    /// and how many PRs they're spread over.
    pub fn unsubmitted_totals(&self) -> (usize, usize) {
        let items: usize = self.drafts.values().map(|d| d.unsubmitted_count()).sum();
        let prs = self.drafts.values().filter(|d| d.has_unsubmitted()).count();
        (items, prs)
    }

    /// Existing draft for this PR, or a fresh one.
    pub fn get_or_create(
        &mut self,
        repo: &str,
        pr_number: u64,
        pr_title: &str,
        head_sha: &str,
    ) -> &mut PrDraft {
        self.drafts
            .entry((repo.to_string(), pr_number))
            .or_insert_with(|| {
                PrDraft::new(
                    repo.to_string(),
                    pr_number,
                    pr_title.to_string(),
                    head_sha.to_string(),
                )
            })
    }

    /// Write a draft through to disk, or delete it if nothing is left in it.
    pub fn save(&mut self, repo: &str, pr_number: u64) {
        let key = (repo.to_string(), pr_number);
        let Some(draft) = self.drafts.get(&key) else { return };
        if draft.is_empty() {
            self.remove(repo, pr_number);
            return;
        }
        let path = Self::path_for(&self.dir, repo, pr_number);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(draft) {
            let _ = std::fs::write(&path, text);
        }
    }

    pub fn upsert(&mut self, draft: PrDraft) {
        let key = (draft.repo.clone(), draft.pr_number);
        self.drafts.insert(key.clone(), draft);
        self.save(&key.0, key.1);
    }

    pub fn remove(&mut self, repo: &str, pr_number: u64) {
        self.drafts.remove(&(repo.to_string(), pr_number));
        let _ = std::fs::remove_file(Self::path_for(&self.dir, repo, pr_number));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai(file: &str, line: u64, body: &str, accepted: Option<bool>) -> ClaudeComment {
        ClaudeComment {
            file: file.to_string(),
            line,
            body: body.to_string(),
            severity: None,
            accepted,
        }
    }

    #[test]
    fn merge_keeps_decisions_when_line_shifts() {
        let old = vec![
            ai("a.rs", 10, "unchecked unwrap", Some(false)),
            ai("a.rs", 20, "missing await", Some(true)),
        ];
        // Re-review reports the same findings at new line numbers, plus a new one.
        let new = vec![
            ai("a.rs", 14, "unchecked unwrap", None),
            ai("a.rs", 25, "missing await", None),
            ai("b.rs", 3, "typo", None),
        ];

        let merged = merge_ai_comments(&old, new);
        assert_eq!(merged[0].accepted, Some(false), "discard should carry over");
        assert_eq!(merged[1].accepted, Some(true), "accept should carry over");
        assert_eq!(merged[2].accepted, None, "new finding stays undecided");
        assert_eq!(merged[0].line, 14, "new line number wins");
    }

    #[test]
    fn merge_does_not_resurrect_decisions_for_changed_text() {
        let old = vec![ai("a.rs", 10, "old wording", Some(false))];
        let merged = merge_ai_comments(&old, vec![ai("a.rs", 10, "new wording", None)]);
        assert_eq!(merged[0].accepted, None);
    }

    #[test]
    fn submit_drops_accepted_and_keeps_discarded() {
        let mut draft = PrDraft::new("o/r".into(), 7, "t".into(), "sha".into());
        draft.apply_review(
            vec![
                ai("a.rs", 1, "keep me hidden", Some(false)),
                ai("a.rs", 2, "posted", Some(true)),
            ],
            "sha".into(),
        );
        draft.draft_comments.push(SavedDraftComment {
            file: "a.rs".into(),
            line: 2,
            body: "posted".into(),
            reply_to_comment_id: None,
            resolve: false,
        });
        draft.pending_resolve_ids.push(99);
        assert_eq!(draft.unsubmitted_count(), 2);

        draft.mark_submitted();

        assert!(!draft.has_unsubmitted(), "nothing pending after submit");
        assert_eq!(draft.ai_comments.len(), 1);
        assert_eq!(draft.ai_comments[0].accepted, Some(false));
        // Head SHA is retained so the PR isn't re-reviewed from scratch.
        assert_eq!(draft.head_sha, "sha");
        assert!(!draft.is_empty(), "draft must survive to suppress re-review");
    }

#[test]
    fn only_an_engaged_draft_keeps_a_pr_in_scope() {
        // A queue placeholder must not opt a PR into reviews forever.
        let mut d = PrDraft::new("o/r".into(), 1, "t".into(), "sha".into());
        d.review_state = ReviewState::Queued;
        assert!(!d.is_engaged());

        // A completed review counts, even with nothing to report.
        d.review_state = ReviewState::Done;
        assert!(d.is_engaged());

        // So does a comment of your own.
        let mut d = PrDraft::new("o/r".into(), 1, "t".into(), "sha".into());
        d.draft_comments.push(SavedDraftComment {
            file: "a.rs".into(),
            line: 1,
            body: "x".into(),
            reply_to_comment_id: None,
            resolve: false,
        });
        assert!(d.is_engaged());
    }

#[test]
    fn severity_pruning_keeps_accepted_findings_only() {
        let dir = std::env::temp_dir().join(format!("ghpr-test-sev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let sev = |s: &str, accepted: Option<bool>| SavedAiComment {
            file: "a.rs".into(),
            line: 1,
            body: s.to_string(),
            severity: Some(s.to_string()),
            accepted,
        };

        let mut store = DraftStore { dir: dir.clone(), drafts: HashMap::new() };
        let draft = store.get_or_create("o/r", 1, "t", "sha");
        draft.review_state = ReviewState::Done;
        draft.ai_comments = vec![
            sev("HIGH", None),
            sev("MEDIUM", None),
            sev("LOW", None),           // noise, undecided -> dropped
            sev("INFO", Some(false)),   // noise, discarded -> dropped
            sev("LOW", Some(true)),     // accepted -> kept, its comment is queued
        ];
        draft.ai_comments.push(SavedAiComment {
            file: "a.rs".into(),
            line: 2,
            body: "no severity".into(),
            severity: None,
            accepted: None,
        });
        store.save("o/r", 1);

        store.drop_below_severity(2); // MEDIUM

        let kept: Vec<&str> = store
            .get("o/r", 1)
            .unwrap()
            .ai_comments
            .iter()
            .map(|c| c.body.as_str())
            .collect();
        assert_eq!(kept, vec!["HIGH", "MEDIUM", "LOW", "no severity"]);
        assert_eq!(
            store.get("o/r", 1).unwrap().ai_comments[2].accepted,
            Some(true),
            "the surviving LOW is the accepted one"
        );

        // The pruning is written through, not just held in memory.
        let reloaded = DraftStore::load_from(&dir);
        assert_eq!(
            reloaded.get(&("o/r".to_string(), 1)).unwrap().ai_comments.len(),
            4
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("ghpr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = DraftStore {
            dir: dir.clone(),
            drafts: HashMap::new(),
        };
        let draft = store.get_or_create("Owner/Repo-x", 42, "Title", "abc123");
        draft.review_state = ReviewState::Done;
        draft.ai_comments.push(SavedAiComment {
            file: "src/x.rs".into(),
            line: 5,
            body: "nit".into(),
            severity: Some("LOW".into()),
            accepted: Some(false),
        });
        draft.draft_comments.push(SavedDraftComment {
            file: "src/x.rs".into(),
            line: 5,
            body: "my reply".into(),
            reply_to_comment_id: Some(555),
            resolve: true,
        });
        store.save("Owner/Repo-x", 42);

        let reloaded = DraftStore {
            dir: dir.clone(),
            drafts: DraftStore::load_from(&dir),
        };
        let got = reloaded.get("Owner/Repo-x", 42).expect("draft reloaded");
        assert_eq!(got.pr_number, 42);
        assert_eq!(got.head_sha, "abc123");
        assert_eq!(got.ai_comments[0].accepted, Some(false));
        assert_eq!(got.draft_comments[0].reply_to_comment_id, Some(555));
        assert!(got.draft_comments[0].resolve);

        // Deleting removes the file, so a later load doesn't bring it back.
        let mut store = reloaded;
        store.remove("Owner/Repo-x", 42);
        assert!(DraftStore::load_from(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_review_is_requeued_on_load() {
        let dir = std::env::temp_dir().join(format!("ghpr-test-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = DraftStore {
            dir: dir.clone(),
            drafts: HashMap::new(),
        };
        let draft = store.get_or_create("o/r", 1, "t", "sha");
        draft.review_state = ReviewState::Running;
        store.save("o/r", 1);

        let loaded = DraftStore::load_from(&dir);
        assert_eq!(
            loaded.get(&("o/r".to_string(), 1)).unwrap().review_state,
            ReviewState::Queued
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
