//! fs 模块 — 路径安全 + 文件系统操作。
//!
//! 对齐 `lib/path-security.ts` + `lib/allowed-roots.ts`。
//! IO 函数用 async fn + std::thread(运行时无关),不绑定 tokio。

pub mod path_security;

pub use path_security::{is_path_within_roots, is_existing_path_within_roots};
