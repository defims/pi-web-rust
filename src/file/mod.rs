//! file 模块 — 对齐 agegr/pi-web `lib/file-paths.ts` + `lib/file-types.ts` 等。

pub mod paths;

pub use paths::{
    normalize_file_path_slashes, encode_file_path_for_api, get_file_name,
    get_file_directory, get_relative_file_path, join_file_path,
};
