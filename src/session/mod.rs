//! session 模块 — 对齐 agegr/pi-web `lib/session-path.ts` + `lib/session-reader.ts`
//! + `lib/session-timing.ts` + `lib/session-title.ts`。

pub mod path;
pub mod reader;
pub mod timing;
pub mod title;

pub use path::session_path_key;
pub use reader::{
    read_session_header, list_session_files, SessionHeader,
    invalidate_path_cache, resolve_session_path, cache_session_path,
};
pub use timing::{TimingEntry, compute_session_total_active_ms};
pub use title::{
    GeneratedSessionTitle, SessionTitleRunner, TITLE_PROMPT, TITLE_TIMEOUT_MS, Usage,
    append_title_request_to_trailing_user, assistant_result_from_messages,
    generate_session_title, parse_generated_session_title, sanitize_title_messages,
    strip_wrapping_quotes,
};
