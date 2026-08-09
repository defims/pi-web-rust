//! session 模块 — 对齐 agegr/pi-web `lib/session-path.ts` + `lib/session-reader.ts`
//! + `lib/session-timing.ts`。

pub mod path;
pub mod reader;
pub mod timing;

pub use path::session_path_key;
pub use reader::{
    read_session_header, list_session_files, SessionHeader,
    invalidate_path_cache, resolve_session_path, cache_session_path,
};
pub use timing::{TimingEntry, compute_session_total_active_ms};
