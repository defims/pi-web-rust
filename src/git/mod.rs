//! git 模块 — 对齐 agegr/pi-web `lib/git-types.ts` + `lib/git-status.ts`。

pub mod types;
pub mod status;

pub use types::{GitFileStatus, GitFileStatusKind, GitStatusResponse, GitFileDiffResponse};
pub use status::{GitPorcelainEntry, parse_git_porcelain_v1, classify_git_status};
