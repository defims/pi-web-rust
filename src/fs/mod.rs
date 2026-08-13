//! fs 模块 — 路径安全 + 文件系统操作 + 文件访问控制。
//!
//! 对齐 `lib/path-security.ts` + `lib/allowed-roots.ts` + `lib/atomic-file.ts`
//! + `lib/directory-browser.ts` + `lib/file-access.ts` + `lib/file-dirent.ts`,
//! 文件 IO 函数均为 async + std::thread(运行时无关,不绑定 tokio)。
//
// clippy 误判中文段落为列表项(`文件` 被当成 list marker),抑制。
#![allow(clippy::doc_lazy_continuation)]

pub mod allowed_roots;
pub mod atomic_file;
pub mod bash_output;
pub mod directory_browser;
pub mod file_access;
pub mod file_dirent;
pub mod file_upload;
pub mod models_config_store;
pub mod path_security;

pub use allowed_roots::{allow_file_root, get_additional_allowed_roots, normalize_slashes};
pub use atomic_file::{write_private_file_atomic, write_private_file_atomic_blocking};
pub use directory_browser::{
    get_browse_start_directory, get_parent_directory, list_directories, normalize_directory,
    resolve_directory, should_show_windows_drive_picker, BrowsableDirectory,
};
pub use file_access::{
    get_allowed_file_roots, invalidate_allowed_roots_cache, is_existing_file_path_allowed,
    is_file_path_allowed, is_windows_absolute_path,
};
pub use file_dirent::resolve_dirent_is_directory;
pub use file_upload::{
    inspect_upload_targets, parse_upload_conflict_strategy, validate_upload_file_names,
    UploadConflictStrategy, UploadTargetInspection,
};
pub use path_security::{is_existing_path_within_roots, is_path_within_roots};
