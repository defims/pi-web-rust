//! git 模块 — 对齐 agegr/pi-web `lib/git-types.ts` + `lib/git-status.ts` +
//! `lib/git-changes.ts` + `lib/worktree.ts`。

pub mod changes;
pub mod status;
pub mod types;
pub mod worktree;

pub use types::{GitFileStatus, GitFileStatusKind, GitStatusCode, GitStatusResponse, GitFileDiffResponse};
pub use status::{GitPorcelainEntry, parse_git_porcelain_v1, classify_git_status};
pub use changes::{find_repository_root, get_git_status, get_git_file_diff};
pub use worktree::{ProjectInfo, WorktreeInfo, resolve_project, list_worktrees, add_worktree, remove_worktree, invalidate_project_cache};
