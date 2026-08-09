//! fs 模块 — 路径安全 + 文件系统操作 + 文件访问控制。
//!
//! 对齐 `lib/path-security.ts` + `lib/allowed-roots.ts` + `lib/atomic-file.ts`
//! + `lib/directory-browser.ts` + `lib/file-access.ts`。
//! IO 函数用 async fn + std::thread(运行时无关),不绑定 tokio。

pub mod allowed_roots;
pub mod atomic_file;
pub mod bash_output;
pub mod directory_browser;
pub mod file_access;
pub mod file_upload;
pub mod models_config_store;
pub mod path_security;

pub use path_security::{is_path_within_roots, is_existing_path_within_roots};
pub use allowed_roots::{normalize_slashes, get_additional_allowed_roots, allow_file_root};
pub use atomic_file::{write_private_file_atomic, write_private_file_atomic_blocking};
pub use directory_browser::{
    BrowsableDirectory, should_show_windows_drive_picker, get_browse_start_directory,
    normalize_directory, get_parent_directory, resolve_directory, list_directories,
};
pub use file_access::{
    is_windows_absolute_path, get_allowed_file_roots, is_file_path_allowed,
    is_existing_file_path_allowed, invalidate_allowed_roots_cache,
};
pub use file_upload::{
    UploadConflictStrategy, UploadTargetInspection,
    parse_upload_conflict_strategy, validate_upload_file_names, inspect_upload_targets,
};
