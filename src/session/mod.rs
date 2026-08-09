//! session 模块 — 对齐 agegr/pi-web `lib/session-path.ts` + `lib/session-reader.ts`。
//!
//! JSONL 会话文件读取 + 路径缓存 + entry→UI message 转换。

pub mod path;
pub mod reader;

pub use path::session_path_key;
pub use reader::{
    read_session_header, list_session_files, SessionHeader,
    invalidate_path_cache, resolve_session_path, cache_session_path,
};
