//! git 模块 — 对齐 agegr/pi-web `lib/git-types.ts` + `lib/git-status.ts` +
//! `lib/git-changes.ts` + `lib/worktree.ts`。

pub mod changes;
pub mod status;
pub mod types;
pub mod worktree;

pub use changes::{find_repository_root, get_git_file_diff, get_git_status};
pub use status::{classify_git_status, parse_git_porcelain_v1, GitPorcelainEntry};
pub use types::{
    GitFileDiffResponse, GitFileStatus, GitFileStatusKind, GitStatusCode, GitStatusResponse,
};
pub use worktree::{
    add_worktree, invalidate_project_cache, list_worktrees, remove_worktree, resolve_project,
    ProjectInfo, WorktreeInfo,
};
