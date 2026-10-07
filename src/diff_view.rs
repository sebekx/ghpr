use crate::github::{DiffSide, ReviewThread};
use crate::highlight::{HighlightedFile, Highlighter};
use std::collections::HashMap;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiffView {
    pub files: Vec<DiffFile>,
    pub tree: Vec<TreeNode>,
    pub selected_file: usize,
    pub scroll: usize,
    pub cursor_line: usize,
    pub threads: Vec<ReviewThread>,
    pub claude_comments: Vec<ClaudeComment>,
    pub draft_comments: Vec<DraftComment>,
    pub input_mode: Option<InputMode>,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub pr_number: u64,
    pub repo_name: String,
    pub loading_review: bool,
    pub review_output: String,
    pub review_scroll: u16,
    /// Status message from submit action
    pub submit_status: Option<String>,
    /// Precomputed comment positions for current file: diff_line_index -> list of thread indices
    pub line_threads: HashMap<usize, Vec<usize>>,
    pub line_claude: HashMap<usize, Vec<usize>>,
    /// Your own standalone draft comments mapped to diff lines for the
    /// current file (replies live inside their thread instead)
    pub line_drafts: HashMap<usize, Vec<usize>>,
    /// File-level threads (no line or line not in diff) for current file
    pub file_level_threads: Vec<usize>,
    /// File-level Claude comments (line not in diff) for current file
    pub file_level_claude: Vec<usize>,
    /// Draft comments whose line is no longer in the diff
    pub file_level_drafts: Vec<usize>,
    /// Syntax highlight cache: file_index -> highlighted data
    pub highlight_cache: HashMap<usize, HighlightedFile>,
    /// Thread indices pending resolve (draft)
    pub pending_resolves: Vec<usize>,
    /// Resolve targets loaded from a stored draft whose thread hasn't been
    /// matched yet — resolved into `pending_resolves` once threads arrive.
    pub pending_resolve_ids: Vec<u64>,
    /// Set when draft state changed and needs writing to disk.
    pub dirty: bool,
    /// Rendered line offset for input overlay positioning (set during draw)
    pub input_target_line: Option<usize>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiffFile {
    pub path: String,
    pub lines: Vec<DiffLine>,
    pub additions: u64,
    pub deletions: u64,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub new_line: Option<u64>,
    pub old_line: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    Hunk,
    Meta,
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub display: String,
    pub depth: usize,
    pub is_dir: bool,
    pub file_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ClaudeComment {
    pub file: String,
    pub line: u64,
    pub body: String,
    pub severity: Option<String>,
    pub accepted: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct DraftComment {
    pub file: String,
    pub line: u64,
    pub body: String,
    pub in_reply_to_thread: Option<usize>,
    /// Root comment id of the replied-to thread. Survives restarts, unlike
    /// `in_reply_to_thread`, which is an index into the current `threads`.
    pub reply_to_comment_id: Option<u64>,
    pub resolve: bool,
}

/// What the diff cursor is currently pointing at, for the `e` and `d` keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorTarget {
    /// AI finding you haven't decided on
    PendingAi,
    /// AI finding you accepted — queued for submission
    AcceptedAi,
    /// AI finding you discarded
    DiscardedAi,
    /// A comment of your own that hasn't been posted
    OwnDraft,
    None,
}

#[derive(Debug, Clone)]
pub enum InputMode {
    NewComment { diff_line: usize },
    Reply { thread_idx: usize, resolve: bool },
    EditClaude { claude_idx: usize },
    /// Editing a comment of your own that hasn't been posted yet
    EditDraft { draft_idx: usize },
}

impl DiffView {
    pub fn new(diff_text: &str, repo_name: String, pr_number: u64) -> Self {
        let files = parse_diff(diff_text);
        let tree = build_tree(&files);
        let mut view = DiffView {
            files,
            tree,
            selected_file: 0,
            scroll: 0,
            cursor_line: 0,
            threads: Vec::new(),
            claude_comments: Vec::new(),
            draft_comments: Vec::new(),
            input_mode: None,
            input_buffer: String::new(),
            input_cursor: 0,
            pr_number,
            repo_name,
            loading_review: false,
            review_output: String::new(),
            review_scroll: 0,
            submit_status: None,
            line_threads: HashMap::new(),
            line_claude: HashMap::new(),
            line_drafts: HashMap::new(),
            file_level_threads: Vec::new(),
            file_level_claude: Vec::new(),
            file_level_drafts: Vec::new(),
            highlight_cache: HashMap::new(),
            pending_resolves: Vec::new(),
            pending_resolve_ids: Vec::new(),
            dirty: false,
            input_target_line: None,
        };
        view.rebuild_line_maps();
        view
    }

    pub fn set_threads(&mut self, threads: Vec<ReviewThread>) {
        self.threads = threads;
        self.remap_thread_refs();
    }

    /// Bind draft replies and pending resolves loaded from disk to the thread
    /// indices of the currently loaded threads, matching on comment id.
    /// Anything that finds no match stays pending for a later thread load.
    pub fn remap_thread_refs(&mut self) {
        let mut id_to_thread: HashMap<u64, usize> = HashMap::new();
        for (ti, thread) in self.threads.iter().enumerate() {
            for comment in &thread.comments {
                id_to_thread.entry(comment.id).or_insert(ti);
            }
        }

        for draft in &mut self.draft_comments {
            if draft.in_reply_to_thread.is_none() {
                if let Some(id) = draft.reply_to_comment_id {
                    draft.in_reply_to_thread = id_to_thread.get(&id).copied();
                }
            }
        }

        for id in std::mem::take(&mut self.pending_resolve_ids) {
            match id_to_thread.get(&id) {
                Some(&ti) => {
                    if !self.pending_resolves.contains(&ti) {
                        self.pending_resolves.push(ti);
                    }
                }
                None => self.pending_resolve_ids.push(id),
            }
        }

        self.rebuild_line_maps();
    }

    pub fn set_claude_comments(&mut self, comments: Vec<ClaudeComment>) {
        self.claude_comments = comments;
        self.rebuild_line_maps();
    }

    pub fn current_file(&self) -> Option<&DiffFile> {
        self.files.get(self.selected_file)
    }

    /// Ensure syntax highlighting is cached for the current file
    pub fn ensure_highlighted(&mut self, highlighter: &Highlighter) {
        let idx = self.selected_file;
        if self.highlight_cache.contains_key(&idx) {
            return;
        }
        let Some(file) = self.files.get(idx) else { return };
        // Only pass code lines (Added/Removed/Context) to the highlighter,
        // skipping Meta/Hunk which would corrupt the parser state.
        let code_lines: Vec<(usize, &str)> = file.lines.iter().enumerate()
            .filter_map(|(li, dl)| {
                let c = dl.content.as_str();
                match dl.kind {
                    LineKind::Added | LineKind::Removed => {
                        Some((li, if c.is_empty() { c } else { &c[1..] }))
                    }
                    LineKind::Context => {
                        Some((li, if c.starts_with(' ') { &c[1..] } else { c }))
                    }
                    _ => None,
                }
            })
            .collect();
        let highlighted = highlighter.highlight_file(&file.path, &code_lines);
        self.highlight_cache.insert(idx, highlighted);
    }

    pub fn select_file(&mut self, idx: usize) {
        if idx < self.files.len() {
            self.selected_file = idx;
            self.scroll = 0;
            self.cursor_line = 0;
            self.rebuild_line_maps();
        }
    }

    fn rebuild_line_maps(&mut self) {
        self.line_threads.clear();
        self.line_claude.clear();
        self.line_drafts.clear();
        self.file_level_threads.clear();
        self.file_level_claude.clear();
        self.file_level_drafts.clear();

        let Some(file) = self.files.get(self.selected_file) else {
            return;
        };

        // Map threads to diff lines, using side to pick old_line vs new_line
        for (ti, thread) in self.threads.iter().enumerate() {
            if thread.path != file.path {
                continue;
            }
            if let Some(target_line) = thread.line {
                let mut found = false;
                match thread.side {
                    DiffSide::Left => {
                        for (li, dl) in file.lines.iter().enumerate() {
                            if dl.old_line == Some(target_line) {
                                self.line_threads.entry(li).or_default().push(ti);
                                found = true;
                                break;
                            }
                        }
                    }
                    DiffSide::Right => {
                        for (li, dl) in file.lines.iter().enumerate() {
                            if dl.new_line == Some(target_line) {
                                self.line_threads.entry(li).or_default().push(ti);
                                found = true;
                                break;
                            }
                        }
                    }
                }
                // Fallback: try the other side
                if !found {
                    let fallback_match = match thread.side {
                        DiffSide::Left => file.lines.iter().enumerate()
                            .find(|(_, dl)| dl.new_line == Some(target_line)),
                        DiffSide::Right => file.lines.iter().enumerate()
                            .find(|(_, dl)| dl.old_line == Some(target_line)),
                    };
                    if let Some((li, _)) = fallback_match {
                        self.line_threads.entry(li).or_default().push(ti);
                    } else {
                        // Line not in diff context — show as file-level
                        self.file_level_threads.push(ti);
                        self.line_threads.entry(0).or_default().push(ti);
                    }
                }
            } else {
                // No line — file-level comment (also map to line 0 for selection)
                self.file_level_threads.push(ti);
                self.line_threads.entry(0).or_default().push(ti);
            }
        }

        // Map Claude comments to diff lines
        for (ci, cc) in self.claude_comments.iter().enumerate() {
            if cc.file != file.path {
                continue;
            }
            let mut found = false;
            // Try new_line first
            for (li, dl) in file.lines.iter().enumerate() {
                if dl.new_line == Some(cc.line) {
                    self.line_claude.entry(li).or_default().push(ci);
                    found = true;
                    break;
                }
            }
            // Fallback: try old_line
            if !found {
                for (li, dl) in file.lines.iter().enumerate() {
                    if dl.old_line == Some(cc.line) {
                        self.line_claude.entry(li).or_default().push(ci);
                        found = true;
                        break;
                    }
                }
            }
            // Still not found — show as file-level
            if !found {
                self.file_level_claude.push(ci);
                self.line_claude.entry(0).or_default().push(ci);
            }
        }

        // Map your own standalone draft comments to diff lines. Replies are
        // reached through their thread, so they're skipped here.
        for (di, draft) in self.draft_comments.iter().enumerate() {
            if draft.file != file.path || draft.in_reply_to_thread.is_some() {
                continue;
            }
            let hit = file
                .lines
                .iter()
                .position(|dl| dl.new_line == Some(draft.line))
                .or_else(|| {
                    file.lines
                        .iter()
                        .position(|dl| dl.old_line == Some(draft.line))
                });
            match hit {
                Some(li) => self.line_drafts.entry(li).or_default().push(di),
                // The line dropped out of the diff (e.g. narrowed commit
                // range) — surface it at file level so it stays visible.
                None => {
                    self.file_level_drafts.push(di);
                    self.line_drafts.entry(0).or_default().push(di);
                }
            }
        }
    }

    /// Is there anything to stop at on this diff line — a review thread, an AI
    /// finding, or a comment of your own? Drives n/N navigation.
    pub fn has_comment_at(&self, li: usize) -> bool {
        self.line_threads.contains_key(&li)
            || self.line_claude.contains_key(&li)
            || self.line_drafts.contains_key(&li)
    }

    /// Standalone drafts to draw at `li`. Drafts created by accepting an AI
    /// finding are left out — that finding renders its own box. Rendering and
    /// height computation both go through here so they can't disagree.
    pub fn drafts_to_render_at(&self, li: usize) -> Vec<usize> {
        let Some(indices) = self.line_drafts.get(&li) else { return Vec::new() };
        indices
            .iter()
            .copied()
            .filter(|&di| {
                if self.file_level_drafts.contains(&di) {
                    return false;
                }
                let Some(draft) = self.draft_comments.get(di) else { return false };
                !self.is_accepted_ai_draft(draft)
            })
            .collect()
    }

    /// Did this draft come from accepting an AI finding?
    fn is_accepted_ai_draft(&self, draft: &DraftComment) -> bool {
        self.claude_comments.iter().any(|cc| {
            cc.accepted == Some(true)
                && cc.file == draft.file
                && cc.line == draft.line
                && cc.body == draft.body
        })
    }

    pub fn scroll_up(&mut self) {
        self.cursor_line = self.cursor_line.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        if let Some(file) = self.current_file() {
            if self.cursor_line < file.lines.len().saturating_sub(1) {
                self.cursor_line += 1;
            }
        }
    }

    pub fn page_down(&mut self, page_size: usize) {
        if let Some(file) = self.current_file() {
            let max = file.lines.len().saturating_sub(1);
            self.cursor_line = (self.cursor_line + page_size).min(max);
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.cursor_line = self.cursor_line.saturating_sub(page_size);
    }

    /// Mouse wheel: move the viewport and drag the cursor along so it keeps
    /// its place on screen (otherwise adjust_scroll would snap the view back).
    pub fn wheel_scroll(&mut self, delta: isize) {
        let Some(file) = self.current_file() else { return };
        let max = file.lines.len().saturating_sub(1);
        let step = delta.unsigned_abs();
        if delta < 0 {
            self.scroll = self.scroll.saturating_sub(step);
            self.cursor_line = self.cursor_line.saturating_sub(step);
        } else {
            self.scroll = (self.scroll + step).min(max);
            self.cursor_line = (self.cursor_line + step).min(max);
        }
    }

    /// Compute rendered line count for each diff line (1 for the line itself + inline comments)
    pub fn compute_line_heights(&self, wrap_width: usize) -> Vec<usize> {
        let Some(file) = self.current_file() else { return Vec::new() };
        let w = wrap_width.max(10);

        file.lines.iter().enumerate().map(|(li, _dl)| {
            let mut h: usize = 1; // the diff line itself

            // Inline thread comments
            if let Some(thread_indices) = self.line_threads.get(&li) {
                for &ti in thread_indices {
                    if self.file_level_threads.contains(&ti) { continue; }
                    if let Some(thread) = self.threads.get(ti) {
                        h += 1; // ┌─ Thread header
                        for comment in &thread.comments {
                            h += count_wrapped_lines(&crate::html::to_text(&comment.body), w);
                        }
                        for draft in &self.draft_comments {
                            if draft.in_reply_to_thread == Some(ti) {
                                h += count_wrapped_lines(&draft.body, w);
                            }
                        }
                        h += 1; // └─ footer
                    }
                }
            }

            // Inline Claude comments
            if let Some(claude_indices) = self.line_claude.get(&li) {
                for &ci in claude_indices {
                    if let Some(cc) = self.claude_comments.get(ci) {
                        h += 1; // header
                        h += count_wrapped_lines(&cc.body, w);
                        h += 1; // footer
                    }
                }
            }

            // Your own standalone draft comments
            for di in self.drafts_to_render_at(li) {
                if let Some(draft) = self.draft_comments.get(di) {
                    h += 1; // header
                    h += count_wrapped_lines(&draft.body, w);
                    h += 1; // footer
                }
            }

            h
        }).collect()
    }

    /// Adjust scroll so cursor_line is visible, accounting for inline comment heights
    pub fn adjust_scroll(&mut self, inner_height: usize, inner_width: usize) {
        if inner_height == 0 { return; }
        let heights = self.compute_line_heights(inner_width.saturating_sub(14));
        if heights.is_empty() { return; }

        let cursor = self.cursor_line.min(heights.len().saturating_sub(1));
        let scroll = self.scroll.min(heights.len().saturating_sub(1));

        if cursor < scroll {
            // Cursor above visible area
            self.scroll = cursor;
        } else {
            // Check if cursor is below visible area
            let rendered: usize = heights[scroll..=cursor].iter().sum();
            if rendered > inner_height {
                // Scroll forward until cursor fits on screen
                let mut new_scroll = scroll;
                while new_scroll < cursor {
                    let vis: usize = heights[new_scroll..=cursor].iter().sum();
                    if vis <= inner_height { break; }
                    new_scroll += 1;
                }
                self.scroll = new_scroll;
            }
        }
    }

    /// Check if a file (by index) has any comments
    pub fn file_has_comments(&self, file_idx: usize) -> bool {
        if let Some(file) = self.files.get(file_idx) {
            let has_threads = self.threads.iter().any(|t| t.path == file.path);
            let has_claude = self.claude_comments.iter().any(|c| c.file == file.path && c.accepted.is_none());
            let has_drafts = self.draft_comments.iter().any(|d| d.file == file.path);
            has_threads || has_claude || has_drafts
        } else {
            false
        }
    }

    /// Jump to next file with comments, returns the tree index if found
    pub fn jump_next_file_with_comments(&mut self) -> Option<usize> {
        let total = self.files.len();
        if total == 0 { return None; }
        for offset in 1..=total {
            let fi = (self.selected_file + offset) % total;
            if self.file_has_comments(fi) {
                self.select_file(fi);
                // Find the tree index for this file
                return self.tree.iter().position(|n| n.file_index == Some(fi));
            }
        }
        None
    }

    /// Jump to prev file with comments, returns the tree index if found
    pub fn jump_prev_file_with_comments(&mut self) -> Option<usize> {
        let total = self.files.len();
        if total == 0 { return None; }
        for offset in 1..=total {
            let fi = (self.selected_file + total - offset) % total;
            if self.file_has_comments(fi) {
                self.select_file(fi);
                return self.tree.iter().position(|n| n.file_index == Some(fi));
            }
        }
        None
    }

    /// Jump to next comment in current file. If none, jump to next file with comments.
    /// Returns Some(tree_index) if jumped to a different file.
    pub fn jump_next_comment_or_file(&mut self) -> Option<usize> {
        // Try within current file first
        if let Some(file) = self.current_file() {
            let max = file.lines.len();
            for li in (self.cursor_line + 1)..max {
                if self.has_comment_at(li) {
                    self.cursor_line = li;
                    return None; // stayed in same file
                }
            }
        }
        // No more comments in this file — jump to next file with comments
        if let Some(ti) = self.jump_next_file_with_comments() {
            // Jump cursor to first comment in the new file
            self.jump_to_first_comment();
            Some(ti)
        } else {
            // Wrap: go to first comment in current file
            if let Some(file) = self.current_file() {
                for li in 0..file.lines.len() {
                    if self.has_comment_at(li) {
                        self.cursor_line = li;
                        break;
                    }
                }
            }
            None
        }
    }

    /// Jump to prev comment in current file. If none, jump to prev file with comments.
    pub fn jump_prev_comment_or_file(&mut self) -> Option<usize> {
        // Try within current file
        if self.cursor_line > 0 {
            for li in (0..self.cursor_line).rev() {
                if self.has_comment_at(li) {
                    self.cursor_line = li;
                    return None;
                }
            }
        }
        // Jump to prev file with comments
        if let Some(ti) = self.jump_prev_file_with_comments() {
            self.jump_to_last_comment();
            Some(ti)
        } else {
            None
        }
    }

    fn jump_to_first_comment(&mut self) {
        if let Some(file) = self.current_file() {
            for li in 0..file.lines.len() {
                if self.has_comment_at(li) {
                    self.cursor_line = li;
                    return;
                }
            }
        }
    }

    fn jump_to_last_comment(&mut self) {
        if let Some(file) = self.current_file() {
            for li in (0..file.lines.len()).rev() {
                if self.has_comment_at(li) {
                    self.cursor_line = li;
                    return;
                }
            }
        }
    }



    pub fn start_new_comment(&mut self) {
        self.input_mode = Some(InputMode::NewComment {
            diff_line: self.cursor_line,
        });
        self.input_buffer.clear();
        self.input_cursor = 0;
    }

    pub fn start_reply(&mut self) {
        // Find the nearest thread within ±3 lines
        for offset in 0..=3 {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                vec![self.cursor_line.saturating_sub(offset), self.cursor_line + offset]
            };
            for li in lines_to_check {
                if let Some(thread_indices) = self.line_threads.get(&li) {
                    if let Some(&ti) = thread_indices.first() {
                        self.input_mode = Some(InputMode::Reply { thread_idx: ti, resolve: false });
                        self.input_buffer.clear();
                        self.input_cursor = 0;
                        return;
                    }
                }
            }
        }
    }

    pub fn submit_input(&mut self) {
        if self.input_buffer.trim().is_empty() {
            self.input_mode = None;
            return;
        }

        let Some(file) = self.current_file() else {
            self.input_mode = None;
            return;
        };

        match &self.input_mode {
            Some(InputMode::NewComment { diff_line }) => {
                let dl = file.lines.get(*diff_line);
                let line_num = dl.and_then(|l| l.new_line)
                    .or_else(|| dl.and_then(|l| l.old_line))
                    .unwrap_or(1);
                self.draft_comments.push(DraftComment {
                    file: file.path.clone(),
                    line: line_num,
                    body: self.input_buffer.clone(),
                    in_reply_to_thread: None,
                    reply_to_comment_id: None,
                    resolve: false,
                });
                self.dirty = true;
            }
            Some(InputMode::Reply { thread_idx, resolve }) => {
                let thread = self.threads.get(*thread_idx);
                let line_num = thread.and_then(|t| t.line).unwrap_or(0);
                let file_path = thread.map(|t| t.path.clone()).unwrap_or_default();
                let root_id = thread.and_then(|t| t.comments.first()).map(|c| c.id);
                self.draft_comments.push(DraftComment {
                    file: file_path,
                    line: line_num,
                    body: self.input_buffer.clone(),
                    in_reply_to_thread: Some(*thread_idx),
                    reply_to_comment_id: root_id,
                    resolve: *resolve,
                });
                self.dirty = true;
            }
            Some(InputMode::EditClaude { claude_idx }) => {
                if let Some(cc) = self.claude_comments.get_mut(*claude_idx) {
                    cc.body = self.input_buffer.clone();
                    cc.accepted = Some(true);
                    self.draft_comments.push(DraftComment {
                        file: cc.file.clone(),
                        line: cc.line,
                        body: cc.body.clone(),
                        in_reply_to_thread: None,
                        reply_to_comment_id: None,
                        resolve: false,
                    });
                    self.dirty = true;
                }
            }
            Some(InputMode::EditDraft { draft_idx }) => {
                let di = *draft_idx;
                let new_body = self.input_buffer.clone();
                if let Some(draft) = self.draft_comments.get(di) {
                    // If this draft mirrors an accepted AI finding, retitle
                    // that finding too so the two don't drift apart and get
                    // drawn as two separate boxes.
                    let (file, line, old_body) =
                        (draft.file.clone(), draft.line, draft.body.clone());
                    for cc in self.claude_comments.iter_mut() {
                        if cc.accepted == Some(true)
                            && cc.file == file
                            && cc.line == line
                            && cc.body == old_body
                        {
                            cc.body = new_body.clone();
                        }
                    }
                }
                if let Some(draft) = self.draft_comments.get_mut(di) {
                    draft.body = new_body;
                    self.dirty = true;
                }
            }
            None => {}
        }
        self.input_buffer.clear();
        self.input_cursor = 0;
        self.input_mode = None;
        // The draft list changed, so the line map has to be rebuilt or the
        // new comment won't render or be reachable with n/N.
        self.rebuild_line_maps();
    }

    pub fn toggle_resolve(&mut self) {
        if let Some(InputMode::Reply { resolve, .. }) = &mut self.input_mode {
            *resolve = !*resolve;
        }
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = None;
        self.input_buffer.clear();
        self.input_cursor = 0;
    }

    /// Check if there's a thread near the cursor (±3 lines)
    pub fn has_thread_at_cursor(&self) -> bool {
        for offset in 0..=3 {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                vec![self.cursor_line.saturating_sub(offset), self.cursor_line + offset]
            };
            for li in lines_to_check {
                if self.line_threads.contains_key(&li) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if there's an unresolved thread near the cursor (±3 lines)
    pub fn has_unresolved_thread_at_cursor(&self) -> bool {
        for offset in 0..=3 {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                vec![self.cursor_line.saturating_sub(offset), self.cursor_line + offset]
            };
            for li in lines_to_check {
                if let Some(indices) = self.line_threads.get(&li) {
                    if indices.iter().any(|&ti| self.threads.get(ti).map_or(false, |t| !t.is_resolved)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if there's a pending AI comment near the cursor
    pub fn has_pending_ai_at_cursor(&self) -> bool {
        !self.find_nearest_claude().is_empty()
    }

    /// Find nearest Claude comment index within ±3 lines of cursor (any status, for copy)
    fn find_nearest_claude_any(&self) -> Vec<usize> {
        for offset in 0..=3 {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                vec![self.cursor_line.saturating_sub(offset), self.cursor_line + offset]
            };
            for li in lines_to_check {
                if let Some(indices) = self.line_claude.get(&li) {
                    if !indices.is_empty() {
                        return indices.clone();
                    }
                }
            }
        }
        Vec::new()
    }

    /// Find nearest pending Claude comment index within ±3 lines of cursor
    fn find_nearest_claude(&self) -> Vec<usize> {
        for offset in 0..=3 {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                let mut v = Vec::new();
                v.push(self.cursor_line.saturating_sub(offset));
                v.push(self.cursor_line + offset);
                v
            };
            for li in lines_to_check {
                if let Some(indices) = self.line_claude.get(&li) {
                    let pending: Vec<usize> = indices.iter()
                        .filter(|&&ci| self.claude_comments.get(ci).map_or(false, |c| c.accepted.is_none()))
                        .copied()
                        .collect();
                    if !pending.is_empty() {
                        return pending;
                    }
                }
            }
        }
        Vec::new()
    }

    /// Accept Claude comment at/near cursor — marks as accepted and adds as draft comment
    pub fn accept_claude_at_cursor(&mut self) {
        let indices = self.find_nearest_claude();
        for ci in indices {
            if let Some(cc) = self.claude_comments.get_mut(ci) {
                if cc.accepted.is_none() {
                    cc.accepted = Some(true);
                    self.draft_comments.push(DraftComment {
                        file: cc.file.clone(),
                        line: cc.line,
                        body: cc.body.clone(),
                        in_reply_to_thread: None,
                        reply_to_comment_id: None,
                        resolve: false,
                    });
                    self.dirty = true;
                }
            }
        }
        self.rebuild_line_maps();
    }

    /// Toggle the discard flag on the AI comment(s) at/near the cursor.
    ///
    /// Comments that are pending or accepted become discarded; a comment that
    /// is already discarded goes back to pending so it can be reconsidered.
    /// Un-accepting also drops the draft comment the acceptance created.
    pub fn toggle_discard_at_cursor(&mut self) {
        let indices = self.find_nearest_claude_any();
        if indices.is_empty() {
            return;
        }

        // Everything already discarded → un-discard. Otherwise discard all.
        let all_discarded = indices
            .iter()
            .filter_map(|&ci| self.claude_comments.get(ci))
            .all(|c| c.accepted == Some(false));

        for ci in indices {
            let Some(cc) = self.claude_comments.get_mut(ci) else { continue };
            if all_discarded {
                cc.accepted = None;
            } else {
                let was_accepted = cc.accepted == Some(true);
                cc.accepted = Some(false);
                if was_accepted {
                    let (file, line, body) = (cc.file.clone(), cc.line, cc.body.clone());
                    self.draft_comments.retain(|d| {
                        d.in_reply_to_thread.is_some()
                            || !(d.file == file && d.line == line && d.body == body)
                    });
                }
            }
            self.dirty = true;
        }
        self.rebuild_line_maps();
    }

    /// Nearest standalone draft comment of yours within ±3 lines of the cursor
    fn find_nearest_draft(&self) -> Option<usize> {
        for offset in 0..=3usize {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                vec![self.cursor_line.saturating_sub(offset), self.cursor_line + offset]
            };
            for li in lines_to_check {
                if let Some(&di) = self.line_drafts.get(&li).and_then(|v| v.first()) {
                    return Some(di);
                }
            }
        }
        None
    }

    /// Nearest unposted reply of yours, found via the thread it hangs off
    fn find_nearest_reply_draft(&self) -> Option<usize> {
        let ti = self.find_nearest_thread()?;
        self.draft_comments
            .iter()
            .position(|d| d.in_reply_to_thread == Some(ti))
    }

    /// An unposted comment of yours at the cursor — standalone, a reply, or
    /// the copy created by accepting an AI finding.
    pub fn find_draft_at_cursor(&self) -> Option<usize> {
        self.find_nearest_draft()
            .or_else(|| self.find_nearest_reply_draft())
    }

    /// What the cursor is sitting on. Editing, discarding and the key hints in
    /// the status bar all read this, so the labels always match what the keys
    /// actually do.
    pub fn cursor_target(&self) -> CursorTarget {
        let ai = self.find_nearest_claude_any();
        if !ai.is_empty() {
            let states: Vec<Option<bool>> = ai
                .iter()
                .filter_map(|&ci| self.claude_comments.get(ci))
                .map(|c| c.accepted)
                .collect();
            if states.iter().all(|s| *s == Some(false)) {
                return CursorTarget::DiscardedAi;
            }
            if states.iter().any(|s| s.is_none()) {
                return CursorTarget::PendingAi;
            }
            return CursorTarget::AcceptedAi;
        }
        if self.find_draft_at_cursor().is_some() {
            return CursorTarget::OwnDraft;
        }
        CursorTarget::None
    }

    /// `e` — edit whatever is under the cursor. A pending AI finding is edited
    /// and accepted in one step; an accepted finding or a comment of your own
    /// opens for rewording.
    pub fn edit_at_cursor(&mut self) {
        match self.cursor_target() {
            CursorTarget::PendingAi => self.edit_claude_at_cursor(),
            CursorTarget::AcceptedAi | CursorTarget::OwnDraft => self.edit_draft_at_cursor(),
            // A discarded finding has to be brought back before it can be
            // reworded, otherwise the edit would be invisible.
            CursorTarget::DiscardedAi | CursorTarget::None => {}
        }
    }

    fn edit_draft_at_cursor(&mut self) {
        let Some(di) = self.find_draft_at_cursor() else { return };
        let Some(draft) = self.draft_comments.get(di) else { return };
        self.input_buffer = draft.body.clone();
        self.input_cursor = self.input_buffer.len();
        self.input_mode = Some(InputMode::EditDraft { draft_idx: di });
    }

    /// `d` — discard whatever is under the cursor. AI findings toggle between
    /// discarded and pending; your own comments are removed outright, since
    /// there would be nothing left to bring back.
    pub fn discard_at_cursor(&mut self) {
        match self.cursor_target() {
            CursorTarget::PendingAi | CursorTarget::AcceptedAi | CursorTarget::DiscardedAi => {
                self.toggle_discard_at_cursor()
            }
            CursorTarget::OwnDraft => self.remove_draft_at_cursor(),
            CursorTarget::None => {}
        }
    }

    /// Drop an unposted comment of yours.
    pub fn remove_draft_at_cursor(&mut self) {
        let Some(di) = self.find_draft_at_cursor() else { return };
        if di >= self.draft_comments.len() {
            return;
        }
        self.draft_comments.remove(di);
        // Indices shifted, so anything holding one has to be rebuilt.
        self.fix_indices_after_draft_removal(di);
        self.dirty = true;
        self.rebuild_line_maps();
    }

    /// Keep an open input box pointing at the right draft after a removal.
    fn fix_indices_after_draft_removal(&mut self, removed: usize) {
        if let Some(InputMode::EditDraft { draft_idx }) = &mut self.input_mode {
            if *draft_idx == removed {
                self.input_mode = None;
                self.input_buffer.clear();
                self.input_cursor = 0;
            } else if *draft_idx > removed {
                *draft_idx -= 1;
            }
        }
    }

    /// Find the nearest thread index within ±3 lines of cursor (prefers unresolved)
    fn find_nearest_thread(&self) -> Option<usize> {
        for offset in 0..=3 {
            let lines_to_check: Vec<usize> = if offset == 0 {
                vec![self.cursor_line]
            } else {
                vec![self.cursor_line.saturating_sub(offset), self.cursor_line + offset]
            };
            for li in lines_to_check {
                if let Some(indices) = self.line_threads.get(&li) {
                    // Prefer unresolved
                    if let Some(&ti) = indices
                        .iter()
                        .find(|&&ti| self.threads.get(ti).map_or(false, |t| !t.is_resolved))
                    {
                        return Some(ti);
                    }
                    if let Some(&ti) = indices.first() {
                        return Some(ti);
                    }
                }
            }
        }
        None
    }

    /// Render a thread as plain text suitable for clipboard
    fn format_thread(&self, ti: usize) -> Option<String> {
        let thread = self.threads.get(ti)?;
        let mut out = String::new();
        let line_str = thread
            .line
            .map(|l| format!(":{}", l))
            .unwrap_or_default();
        let resolved = if thread.is_resolved { " [resolved]" } else { "" };
        out.push_str(&format!(
            "{}#{} — {}{}{}\n",
            self.repo_name, self.pr_number, thread.path, line_str, resolved
        ));
        for (i, c) in thread.comments.iter().enumerate() {
            out.push('\n');
            let prefix = if i == 0 { "" } else { "↳ " };
            out.push_str(&format!("{}{} ({})\n", prefix, c.author, c.created_at));
            out.push_str(c.body.trim_end());
            out.push('\n');
        }
        Some(out)
    }

    /// Format a Claude/AI comment as plain text suitable for clipboard
    fn format_claude_comment(&self, ci: usize) -> Option<String> {
        let cc = self.claude_comments.get(ci)?;
        let mut out = String::new();
        let line_str = if cc.line > 0 {
            format!(":{}", cc.line)
        } else {
            String::new()
        };
        let status = match cc.accepted {
            Some(true) => " [accepted]",
            Some(false) => " [discarded]",
            None => "",
        };
        out.push_str(&format!(
            "{}#{} — {}{}{}\n",
            self.repo_name, self.pr_number, cc.file, line_str, status
        ));
        if let Some(sev) = &cc.severity {
            out.push_str(&format!("\n[{}]\n", sev));
        }
        out.push('\n');
        out.push_str(cc.body.trim_end());
        out.push('\n');
        Some(out)
    }

    /// Copy the thread or AI comment at cursor to the system clipboard.
    pub fn copy_at_cursor(&mut self) {
        // Try review thread first
        if let Some(ti) = self.find_nearest_thread() {
            if let Some(text) = self.format_thread(ti) {
                match copy_to_clipboard(&text) {
                    Ok(tool) => {
                        let n = self.threads.get(ti).map(|t| t.comments.len()).unwrap_or(0);
                        self.submit_status =
                            Some(format!("Copied thread ({} comments) via {}", n, tool));
                    }
                    Err(e) => {
                        self.submit_status = Some(format!("Copy failed: {}", e));
                    }
                }
                return;
            }
        }
        // Fall back to AI comment
        let claude_indices = self.find_nearest_claude_any();
        if let Some(&ci) = claude_indices.first() {
            if let Some(text) = self.format_claude_comment(ci) {
                match copy_to_clipboard(&text) {
                    Ok(tool) => {
                        self.submit_status =
                            Some(format!("Copied AI comment via {}", tool));
                    }
                    Err(e) => {
                        self.submit_status = Some(format!("Copy failed: {}", e));
                    }
                }
                return;
            }
        }
        self.submit_status = Some("No thread or AI comment at cursor".to_string());
    }

    /// Edit Claude comment at/near cursor — opens input with existing text
    pub fn edit_claude_at_cursor(&mut self) {
        let indices = self.find_nearest_claude();
        if let Some(&ci) = indices.first() {
            if let Some(cc) = self.claude_comments.get(ci) {
                if cc.accepted.is_none() {
                    self.input_buffer = cc.body.clone();
                    self.input_cursor = self.input_buffer.len();
                    self.input_mode = Some(InputMode::EditClaude { claude_idx: ci });
                }
            }
        }
    }

    pub fn tree_select(&mut self, tree_idx: usize) {
        if let Some(node) = self.tree.get(tree_idx) {
            if let Some(fi) = node.file_index {
                self.select_file(fi);
            }
        }
    }
}

/// Parse unified diff into per-file structures
fn parse_diff(diff: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_lines: Vec<DiffLine> = Vec::new();
    let mut adds: u64 = 0;
    let mut dels: u64 = 0;
    let mut old_line: u64 = 0;
    let mut new_line: u64 = 0;

    for raw_line in diff.lines() {
        if raw_line.starts_with("diff --git") {
            // Save previous file
            if let Some(path) = current_path.take() {
                files.push(DiffFile {
                    path,
                    lines: std::mem::take(&mut current_lines),
                    additions: adds,
                    deletions: dels,
                    status: String::new(),
                });
            }
            adds = 0;
            dels = 0;
            // Extract path from "diff --git a/path b/path"
            let parts: Vec<&str> = raw_line.split(" b/").collect();
            let path = parts.get(1).unwrap_or(&"unknown").to_string();
            current_path = Some(path);
            current_lines.push(DiffLine {
                kind: LineKind::Meta,
                content: raw_line.to_string(),
                old_line: None,
                new_line: None,
            });
        } else if raw_line.starts_with("index ")
            || raw_line.starts_with("--- ")
            || raw_line.starts_with("+++ ")
            || raw_line.starts_with("new file")
            || raw_line.starts_with("deleted file")
            || raw_line.starts_with("similarity")
            || raw_line.starts_with("rename")
            || raw_line.starts_with("old mode")
            || raw_line.starts_with("new mode")
        {
            current_lines.push(DiffLine {
                kind: LineKind::Meta,
                content: raw_line.to_string(),
                old_line: None,
                new_line: None,
            });
        } else if raw_line.starts_with("@@") {
            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
            if let Some(plus_pos) = raw_line.find('+') {
                let after_plus = &raw_line[plus_pos + 1..];
                let num_str = after_plus.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("1");
                new_line = num_str.parse().unwrap_or(1);
            }
            if let Some(minus_pos) = raw_line.find('-') {
                let after_minus = &raw_line[minus_pos + 1..];
                let num_str = after_minus.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("1");
                old_line = num_str.parse().unwrap_or(1);
            }
            current_lines.push(DiffLine {
                kind: LineKind::Hunk,
                content: raw_line.to_string(),
                old_line: None,
                new_line: None,
            });
        } else if raw_line.starts_with('+') {
            adds += 1;
            current_lines.push(DiffLine {
                kind: LineKind::Added,
                content: raw_line.to_string(),
                old_line: None,
                new_line: Some(new_line),
            });
            new_line += 1;
        } else if raw_line.starts_with('-') {
            dels += 1;
            current_lines.push(DiffLine {
                kind: LineKind::Removed,
                content: raw_line.to_string(),
                old_line: Some(old_line),
                new_line: None,
            });
            old_line += 1;
        } else {
            // Context line (starts with space or is empty)
            current_lines.push(DiffLine {
                kind: LineKind::Context,
                content: raw_line.to_string(),
                old_line: Some(old_line),
                new_line: Some(new_line),
            });
            old_line += 1;
            new_line += 1;
        }
    }

    // Save last file
    if let Some(path) = current_path {
        files.push(DiffFile {
            path,
            lines: current_lines,
            additions: adds,
            deletions: dels,
            status: String::new(),
        });
    }

    files
}

/// Build a nested tree from file paths, collapsing shared directory prefixes
fn build_tree(files: &[DiffFile]) -> Vec<TreeNode> {
    use std::collections::BTreeMap;

    // Insert all paths into a trie-like structure
    struct DirNode {
        children_dirs: BTreeMap<String, DirNode>,
        files: Vec<(usize, String, u64, u64)>, // (file_index, filename, adds, dels)
    }

    impl DirNode {
        fn new() -> Self {
            DirNode {
                children_dirs: BTreeMap::new(),
                files: Vec::new(),
            }
        }
    }

    let mut root = DirNode::new();
    for (i, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split('/').collect();
        if parts.len() == 1 {
            root.files.push((i, parts[0].to_string(), f.additions, f.deletions));
        } else {
            let mut node = &mut root;
            for &dir_part in &parts[..parts.len() - 1] {
                node = node
                    .children_dirs
                    .entry(dir_part.to_string())
                    .or_insert_with(DirNode::new);
            }
            let filename = parts[parts.len() - 1].to_string();
            node.files.push((i, filename, f.additions, f.deletions));
        }
    }

    // Flatten the trie into TreeNodes, collapsing single-child directories
    let mut nodes = Vec::new();

    fn flatten(
        node: &DirNode,
        prefix: &str,
        depth: usize,
        nodes: &mut Vec<TreeNode>,
    ) {
        // Process child directories
        for (name, child) in &node.children_dirs {
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };

            // Collapse: if this dir has exactly one child dir and no files, merge names
            if child.files.is_empty() && child.children_dirs.len() == 1 {
                flatten(child, &full, depth, nodes);
                continue;
            }

            nodes.push(TreeNode {
                display: format!("{}/", full),
                depth,
                is_dir: true,
                file_index: None,
            });
            flatten(child, "", depth + 1, nodes);
        }

        // Process files in this directory
        for (fi, filename, adds, dels) in &node.files {
            nodes.push(TreeNode {
                display: format!("{} +{} -{}", filename, adds, dels),
                depth,
                is_dir: false,
                file_index: Some(*fi),
            });
        }
    }

    flatten(&root, "", 0, &mut nodes);
    nodes
}

/// Count how many rendered lines a text block produces when wrapped to `width`
fn count_wrapped_lines(text: &str, width: usize) -> usize {
    let w = width.max(10);
    let mut count = 0;
    for line in text.lines() {
        if line.len() <= w {
            count += 1;
        } else {
            let mut remaining = line.len();
            while remaining > 0 {
                count += 1;
                remaining = remaining.saturating_sub(w);
            }
        }
    }
    count.max(1)
}

/// Copy `text` to the system clipboard via an external tool.
/// Tries pbcopy → wl-copy → xclip → xsel and returns the name of the tool used.
fn copy_to_clipboard(text: &str) -> Result<&'static str, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];

    let mut last_err = String::from("no clipboard tool found (install pbcopy/wl-copy/xclip/xsel)");
    for (cmd, args) in candidates {
        let spawn = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawn {
            Ok(c) => c,
            Err(_) => continue, // tool not present, try next
        };
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(text.as_bytes()) {
                last_err = format!("{}: write failed: {}", cmd, e);
                let _ = child.wait();
                continue;
            }
        }
        match child.wait() {
            Ok(status) if status.success() => return Ok(cmd),
            Ok(status) => last_err = format!("{} exited with {}", cmd, status),
            Err(e) => last_err = format!("{} wait failed: {}", cmd, e),
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/a.rs b/a.rs\n\
@@ -1,2 +1,3 @@\n\
 fn main() {\n\
+    let x = 1;\n\
 }\n";

    /// A view with one AI finding on the added line, cursor parked on it.
    fn view_with_finding() -> DiffView {
        let mut dv = DiffView::new(DIFF, "o/r".into(), 1);
        dv.set_claude_comments(vec![ClaudeComment {
            file: "a.rs".into(),
            line: 2,
            body: "unused binding".into(),
            severity: Some("LOW".into()),
            accepted: None,
        }]);
        // The added line is new-file line 2.
        dv.cursor_line = dv
            .files[0]
            .lines
            .iter()
            .position(|l| l.new_line == Some(2) && l.kind == LineKind::Added)
            .expect("added line present");
        assert!(
            dv.has_pending_ai_at_cursor(),
            "finding should be reachable from the cursor"
        );
        dv
    }

    #[test]
    fn d_discards_then_brings_the_finding_back() {
        let mut dv = view_with_finding();

        dv.toggle_discard_at_cursor();
        assert_eq!(dv.claude_comments[0].accepted, Some(false));
        assert_eq!(dv.cursor_target(), CursorTarget::DiscardedAi);
        assert!(dv.dirty, "a decision must be persisted");

        // Pressing d again un-discards it, so it can be reconsidered.
        dv.toggle_discard_at_cursor();
        assert_eq!(dv.claude_comments[0].accepted, None);
        assert_ne!(dv.cursor_target(), CursorTarget::DiscardedAi);
        assert!(dv.has_pending_ai_at_cursor());
    }

    #[test]
    fn d_on_an_accepted_finding_discards_it_and_drops_its_draft() {
        let mut dv = view_with_finding();

        dv.accept_claude_at_cursor();
        assert_eq!(dv.claude_comments[0].accepted, Some(true));
        assert_eq!(dv.draft_comments.len(), 1, "accepting queues a comment");

        dv.toggle_discard_at_cursor();
        assert_eq!(dv.claude_comments[0].accepted, Some(false));
        assert!(
            dv.draft_comments.is_empty(),
            "un-accepting must withdraw the queued comment"
        );
    }

    #[test]
    fn discarded_findings_are_not_re_accepted_by_a() {
        let mut dv = view_with_finding();
        dv.toggle_discard_at_cursor();
        dv.accept_claude_at_cursor();
        assert_eq!(
            dv.claude_comments[0].accepted,
            Some(false),
            "a must not resurrect a discarded finding"
        );
        assert!(dv.draft_comments.is_empty());
    }


    /// Cursor position of the added line (new-file line 2).
    fn added_line_idx(dv: &DiffView) -> usize {
        dv.files[0]
            .lines
            .iter()
            .position(|l| l.new_line == Some(2) && l.kind == LineKind::Added)
            .expect("added line present")
    }

    fn view_with_manual_comment() -> DiffView {
        let mut dv = DiffView::new(DIFF, "o/r".into(), 1);
        dv.cursor_line = added_line_idx(&dv);
        dv.start_new_comment();
        dv.input_buffer = "please rename this".into();
        dv.submit_input();
        assert_eq!(dv.draft_comments.len(), 1);
        dv
    }

    #[test]
    fn n_navigation_stops_on_manually_added_comments() {
        let mut dv = view_with_manual_comment();
        let target = added_line_idx(&dv);

        // Walk away from the comment, then jump forward to it.
        dv.cursor_line = 0;
        assert!(
            dv.has_comment_at(target),
            "manual comment must register on its line"
        );
        dv.jump_next_comment_or_file();
        assert_eq!(dv.cursor_line, target, "n should land on the manual comment");

        // And back again from below.
        dv.cursor_line = dv.files[0].lines.len() - 1;
        dv.jump_prev_comment_or_file();
        assert_eq!(dv.cursor_line, target, "N should land on the manual comment");
    }

    #[test]
    fn manual_comment_can_be_edited() {
        let mut dv = view_with_manual_comment();
        dv.dirty = false;

        dv.edit_at_cursor();
        assert!(
            matches!(dv.input_mode, Some(InputMode::EditDraft { draft_idx: 0 })),
            "e should open the draft for editing"
        );
        assert_eq!(
            dv.input_buffer, "please rename this",
            "editing starts from the existing text"
        );

        dv.input_buffer = "rename to snake_case".into();
        dv.submit_input();

        assert_eq!(dv.draft_comments.len(), 1, "editing must not add a comment");
        assert_eq!(dv.draft_comments[0].body, "rename to snake_case");
        assert!(dv.dirty, "an edit must be persisted");
    }

    #[test]
    fn manual_comment_can_be_removed() {
        let mut dv = view_with_manual_comment();
        dv.dirty = false;

        dv.discard_at_cursor();
        assert!(dv.draft_comments.is_empty(), "d should remove the draft");
        assert!(dv.dirty, "a removal must be persisted");
        assert!(
            !dv.has_comment_at(added_line_idx(&dv)),
            "removed comment must leave the navigation map"
        );
    }

    #[test]
    fn editing_an_accepted_finding_keeps_it_in_one_box() {
        let mut dv = view_with_finding();
        dv.accept_claude_at_cursor();

        // The accepted finding renders itself; its mirror draft must not
        // draw a second box.
        assert!(
            dv.drafts_to_render_at(dv.cursor_line).is_empty(),
            "accepted finding must not also render as a draft"
        );

        // Editing it updates both sides so they stay a single box.
        dv.edit_at_cursor();
        dv.input_buffer = "reworded".into();
        dv.submit_input();

        assert_eq!(dv.claude_comments[0].body, "reworded");
        assert_eq!(dv.draft_comments[0].body, "reworded");
        assert!(
            dv.drafts_to_render_at(dv.cursor_line).is_empty(),
            "still one box after editing"
        );
    }

    #[test]
    fn rendered_and_measured_heights_agree_for_drafts() {
        // A mismatch here makes the cursor drift while scrolling.
        let mut dv = view_with_manual_comment();
        let li = added_line_idx(&dv);
        let heights = dv.compute_line_heights(60);
        // 1 diff line + header + one body line + footer
        assert_eq!(heights[li], 4);

        // Accepting an AI finding adds a mirror draft that isn't drawn, so
        // the height must not grow twice.
        dv.draft_comments.clear();
        dv.set_claude_comments(vec![ClaudeComment {
            file: "a.rs".into(),
            line: 2,
            body: "unused".into(),
            severity: None,
            accepted: None,
        }]);
        dv.cursor_line = li;
        dv.accept_claude_at_cursor();
        let heights = dv.compute_line_heights(60);
        assert_eq!(heights[li], 4, "accepted finding counted once");
    }

    #[test]
    fn a_draft_reply_is_editable_through_its_thread() {
        let mut dv = DiffView::new(DIFF, "o/r".into(), 1);
        dv.set_threads(vec![ReviewThread {
            path: "a.rs".into(),
            line: Some(2),
            side: DiffSide::Right,
            is_resolved: false,
            comments: vec![crate::github::ThreadComment {
                id: 7,
                author: "them".into(),
                body: "why?".into(),
                created_at: String::new(),
            }],
            node_id: None,
        }]);
        dv.cursor_line = added_line_idx(&dv);

        dv.start_reply();
        dv.input_buffer = "because of X".into();
        dv.submit_input();
        assert_eq!(dv.draft_comments.len(), 1);

        assert_eq!(dv.cursor_target(), CursorTarget::OwnDraft, "reply should be reachable");
        dv.edit_at_cursor();
        assert_eq!(dv.input_buffer, "because of X");
        dv.input_buffer = "because of Y".into();
        dv.submit_input();
        assert_eq!(dv.draft_comments[0].body, "because of Y");
        assert_eq!(dv.draft_comments[0].in_reply_to_thread, Some(0));

        dv.discard_at_cursor();
        assert!(dv.draft_comments.is_empty(), "d should drop the reply");
    }

    #[test]
    fn saved_replies_rebind_to_threads_by_comment_id() {
        let mut dv = DiffView::new(DIFF, "o/r".into(), 1);
        // A reply and a resolve restored from disk, before threads load.
        dv.draft_comments.push(DraftComment {
            file: "a.rs".into(),
            line: 2,
            body: "agreed".into(),
            in_reply_to_thread: None,
            reply_to_comment_id: Some(4242),
            resolve: false,
        });
        dv.pending_resolve_ids.push(4242);

        dv.set_threads(vec![ReviewThread {
            path: "a.rs".into(),
            line: Some(2),
            side: DiffSide::Right,
            is_resolved: false,
            comments: vec![crate::github::ThreadComment {
                id: 4242,
                author: "someone".into(),
                body: "please fix".into(),
                created_at: String::new(),
            }],
            node_id: Some("node".into()),
        }]);

        assert_eq!(dv.draft_comments[0].in_reply_to_thread, Some(0));
        assert_eq!(dv.pending_resolves, vec![0]);
        assert!(dv.pending_resolve_ids.is_empty());
    }

    #[test]
    fn unmatched_resolve_ids_survive_until_their_thread_arrives() {
        let mut dv = DiffView::new(DIFF, "o/r".into(), 1);
        dv.pending_resolve_ids.push(999);
        dv.set_threads(Vec::new());
        assert_eq!(
            dv.pending_resolve_ids,
            vec![999],
            "must not silently drop a pending resolve"
        );
        assert!(dv.pending_resolves.is_empty());
    }
}
