//! 对齐 `lib/git-types.ts`。
//!
//! git 响应类型定义。serde rename_all = "camelCase" 对齐上游 JSON 形状。

use serde::{Deserialize, Serialize};

/// 对齐 `GitFileStatusKind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitFileStatusKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

/// 对齐 `GitFileStatus.code` 的单字母码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitStatusCode {
    #[serde(rename = "M")]
    Modified,
    #[serde(rename = "A")]
    Added,
    #[serde(rename = "D")]
    Deleted,
    #[serde(rename = "R")]
    Renamed,
    #[serde(rename = "U")]
    Untracked,
    #[serde(rename = "C")]
    Conflict,
}

/// 对齐 `GitFileStatus`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    pub file_path: String,
    pub status: GitFileStatusKind,
    pub code: GitStatusCode,
    pub index_status: String,
    pub worktree_status: String,
}

/// 对齐 `GitStatusResponse`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusResponse {
    pub is_git_repository: bool,
    pub repository_root: Option<String>,
    pub files: Vec<GitFileStatus>,
    pub additions: u64,
    pub deletions: u64,
}

/// 对齐 `GitFileDiffResponse`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiffResponse {
    pub supported: bool,
    pub status: Option<GitFileStatusKind>,
    pub patch: Option<String>,
}
