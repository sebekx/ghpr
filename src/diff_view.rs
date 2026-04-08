use crate::github::{DiffSide, ReviewThread};
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
    /// File-level threads (no line or line not in diff) for current file
    pub file_level_threads: Vec<usize>,
    /// File-level Claude comments (line not in diff) for current file
    pub file_level_claude: Vec<usize>,
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
    pub accepted: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct DraftComment {
    pub file: String,
    pub line: u64,
    pub body: String,
    pub in_reply_to_thread: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum InputMode {
    NewComment { diff_line: usize },
    Reply { thread_idx: usize },
    EditClaude { claude_idx: usize },
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
            pr_number,
            repo_name,
            loading_review: false,
            review_output: String::new(),
            review_scroll: 0,
            submit_status: None,
            line_threads: HashMap::new(),
            line_claude: HashMap::new(),
            file_level_threads: Vec::new(),
            file_level_claude: Vec::new(),
        };
        view.rebuild_line_maps();
        view
    }

    pub fn set_threads(&mut self, threads: Vec<ReviewThread>) {
        self.threads = threads;
        self.rebuild_line_maps();
    }

    pub fn set_claude_comments(&mut self, comments: Vec<ClaudeComment>) {
        self.claude_comments = comments;
        self.rebuild_line_maps();
    }

    pub fn current_file(&self) -> Option<&DiffFile> {
        self.files.get(self.selected_file)
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
        self.file_level_threads.clear();
        self.file_level_claude.clear();

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
                if self.line_threads.contains_key(&li) || self.line_claude.contains_key(&li) {
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
                    if self.line_threads.contains_key(&li) || self.line_claude.contains_key(&li) {
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
                if self.line_threads.contains_key(&li) || self.line_claude.contains_key(&li) {
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
                if self.line_threads.contains_key(&li) || self.line_claude.contains_key(&li) {
                    self.cursor_line = li;
                    return;
                }
            }
        }
    }

    fn jump_to_last_comment(&mut self) {
        if let Some(file) = self.current_file() {
            for li in (0..file.lines.len()).rev() {
                if self.line_threads.contains_key(&li) || self.line_claude.contains_key(&li) {
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
                        self.input_mode = Some(InputMode::Reply { thread_idx: ti });
                        self.input_buffer.clear();
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
                });
            }
            Some(InputMode::Reply { thread_idx }) => {
                let line_num = self
                    .threads
                    .get(*thread_idx)
                    .and_then(|t| t.line)
                    .unwrap_or(0);
                let file_path = self
                    .threads
                    .get(*thread_idx)
                    .map(|t| t.path.clone())
                    .unwrap_or_default();
                self.draft_comments.push(DraftComment {
                    file: file_path,
                    line: line_num,
                    body: self.input_buffer.clone(),
                    in_reply_to_thread: Some(*thread_idx),
                });
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
                    });
                }
            }
            None => {}
        }
        self.input_buffer.clear();
        self.input_mode = None;
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = None;
        self.input_buffer.clear();
    }

    /// Find nearest Claude comment index within ±3 lines of cursor
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
                    });
                }
            }
        }
    }

    /// Discard Claude comment at/near cursor
    pub fn discard_claude_at_cursor(&mut self) {
        let indices = self.find_nearest_claude();
        for ci in indices {
            if let Some(cc) = self.claude_comments.get_mut(ci) {
                if cc.accepted.is_none() {
                    cc.accepted = Some(false);
                }
            }
        }
    }

    /// Edit Claude comment at/near cursor — opens input with existing text
    pub fn edit_claude_at_cursor(&mut self) {
        let indices = self.find_nearest_claude();
        if let Some(&ci) = indices.first() {
            if let Some(cc) = self.claude_comments.get(ci) {
                if cc.accepted.is_none() {
                    self.input_buffer = cc.body.clone();
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
