//! 对齐 `lib/git-status.ts`。
//!
//! git porcelain v1 解析 + 状态分类。纯逻辑,无 fs/IO 依赖。

use super::types::{GitFileStatusKind, GitStatusCode};

/// 对齐 `GitPorcelainEntry`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPorcelainEntry {
    pub path: String,
    pub original_path: Option<String>,
    pub index_status: String,
    pub worktree_status: String,
}

/// 对齐 `usesRenamePath`。R/C 状态有 rename originalPath 跟在后面。
fn uses_rename_path(index_status: &str, worktree_status: &str) -> bool {
    index_status == "R" || index_status == "C" || worktree_status == "R" || worktree_status == "C"
}

/// 对齐 `parseGitPorcelainV1`。解析 `git status --porcelain=v1 -z` 的 NUL 分隔输出。
pub fn parse_git_porcelain_v1(output: &str) -> Vec<GitPorcelainEntry> {
    let records: Vec<&str> = output.split('\0').collect();
    let mut entries = Vec::new();
    let mut i = 0;

    while i < records.len() {
        let record = records[i];
        i += 1;
        // 记录格式:XY <space> path(X=index status, Y=worktree status)
        if record.len() < 4 || !record.as_bytes()[..2].is_ascii() || record.as_bytes()[2] != b' ' {
            continue;
        }
        let index_status = &record[0..1];
        let worktree_status = &record[1..2];
        let mut entry = GitPorcelainEntry {
            path: record[3..].to_string(),
            original_path: None,
            index_status: index_status.to_string(),
            worktree_status: worktree_status.to_string(),
        };
        if uses_rename_path(index_status, worktree_status) {
            // rename/copy 记录后面跟一条 originalPath
            if i < records.len() {
                entry.original_path = Some(records[i].to_string());
                i += 1;
            }
        }
        entries.push(entry);
    }

    entries
}

/// 对齐 `CONFLICT_STATUSES`。
const CONFLICT_PAIRS: &[&str] = &["DD", "AU", "UD", "UA", "DU", "AA", "UU"];

/// 对齐 `classifyGitStatus`。返回 (status, code)。
pub fn classify_git_status(entry: &GitPorcelainEntry) -> (GitFileStatusKind, GitStatusCode) {
    let pair = format!("{}{}", entry.index_status, entry.worktree_status);
    if pair == "??" {
        return (GitFileStatusKind::Untracked, GitStatusCode::Untracked);
    }
    if CONFLICT_PAIRS.contains(&pair.as_str()) || pair.contains('U') {
        return (GitFileStatusKind::Conflict, GitStatusCode::Conflict);
    }
    if pair.contains('D') {
        return (GitFileStatusKind::Deleted, GitStatusCode::Deleted);
    }
    if pair.contains('R') || pair.contains('C') {
        return (GitFileStatusKind::Renamed, GitStatusCode::Renamed);
    }
    if pair.contains('A') {
        return (GitFileStatusKind::Added, GitStatusCode::Added);
    }
    (GitFileStatusKind::Modified, GitStatusCode::Modified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_entries() {
        let input = "M  src/main.rs\0A  new.rs\0?? untracked.rs\0";
        let entries = parse_git_porcelain_v1(input);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].index_status, "M");
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[1].index_status, "A");
        assert_eq!(entries[2].worktree_status, "?");
        assert_eq!(entries[2].path, "untracked.rs");
    }

    #[test]
    fn parse_rename_entry() {
        let input = "R  new.rs\0old.rs\0";
        let entries = parse_git_porcelain_v1(input);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index_status, "R");
        assert_eq!(entries[0].path, "new.rs");
        assert_eq!(entries[0].original_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn classify_all_statuses() {
        let cases = [
            (
                "M",
                " ",
                GitFileStatusKind::Modified,
                GitStatusCode::Modified,
            ),
            ("A", " ", GitFileStatusKind::Added, GitStatusCode::Added),
            ("D", " ", GitFileStatusKind::Deleted, GitStatusCode::Deleted),
            ("R", " ", GitFileStatusKind::Renamed, GitStatusCode::Renamed),
            ("C", " ", GitFileStatusKind::Renamed, GitStatusCode::Renamed),
            (
                "?",
                "?",
                GitFileStatusKind::Untracked,
                GitStatusCode::Untracked,
            ),
            (
                "U",
                "U",
                GitFileStatusKind::Conflict,
                GitStatusCode::Conflict,
            ),
            (
                "A",
                "U",
                GitFileStatusKind::Conflict,
                GitStatusCode::Conflict,
            ),
            (
                "D",
                "D",
                GitFileStatusKind::Conflict,
                GitStatusCode::Conflict,
            ),
        ];
        for (idx, wt, expected_status, expected_code) in cases {
            let entry = GitPorcelainEntry {
                path: "test.rs".into(),
                original_path: None,
                index_status: idx.into(),
                worktree_status: wt.into(),
            };
            let (status, code) = classify_git_status(&entry);
            assert_eq!(status, expected_status, "index={idx} worktree={wt}");
            assert_eq!(code, expected_code, "index={idx} worktree={wt}");
        }
    }

    #[test]
    fn skip_short_records() {
        let input = "ab\0xy\0";
        let entries = parse_git_porcelain_v1(input);
        assert!(entries.is_empty(), "短记录(<4 字符或格式不对)应跳过");
    }

    #[test]
    fn serde_camel_case() {
        let resp = super::super::types::GitStatusResponse {
            is_git_repository: true,
            repository_root: Some("/repo".into()),
            files: vec![],
            additions: 5,
            deletions: 3,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("isGitRepository"));
        assert!(json.contains("repositoryRoot"));
    }
}
