//! session 模块 — 对齐 agegr/pi-web `lib/session-path.ts` + `lib/session-reader.ts`
//! + `lib/session-timing.ts` + `lib/session-title.ts`。

pub mod entries;
pub mod path;
pub mod reader;
pub mod rpc;
pub mod timing;
pub mod title;

pub use entries::{
    base64_image_info, build_session_context, build_session_context_from_json, entry_to_ui_message,
    omit_tool_result_base64_images, parse_entry_timestamp, SdkContext, SessionContext,
};
pub use path::session_path_key;
pub use reader::{
    cache_session_path, invalidate_path_cache, list_all_sessions, list_session_entries,
    list_session_files, read_session_header, resolve_session_path, SessionHeader, WebSessionInfo,
};
pub use rpc::{
    is_idle_reset_event, is_running_state_event, normalize_rpc_cwd, with_extension_tools,
    StartingSessionGuard, StartingSessionTracker, CODING_TOOL_NAMES, IDLE_RESET_EVENT_TYPES,
    RUNNING_STATE_EVENT_TYPES,
};
pub use timing::{compute_session_total_active_ms, TimingEntry};
pub use title::{
    append_title_request_to_trailing_user, assistant_result_from_messages, generate_session_title,
    parse_generated_session_title, sanitize_title_messages, strip_wrapping_quotes,
    GeneratedSessionTitle, SessionTitleRunner, Usage, TITLE_PROMPT, TITLE_TIMEOUT_MS,
};
