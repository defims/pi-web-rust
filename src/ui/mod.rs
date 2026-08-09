//! ui 模块 — 对齐 agegr/pi-web 客户端 UI 工具(纯计算,无 IO)。
//!
//! 对齐 `lib/chat-lazy-load.ts` + `lib/panel-layout.ts` + `lib/streaming-message.ts`
//! + `lib/normalize.ts`。

pub mod lazy_load;
pub mod panel_layout;
pub mod streaming_message;

pub use lazy_load::{get_visible_render_window, get_next_visible_count, capture_scroll_distance, restore_scroll_top, VISIBLE_PAGE_SIZE};
pub use panel_layout::{
    clamp_panel_width, get_default_right_panel_width, get_sidebar_max_width, get_right_panel_max_width,
    MOBILE_MAX_WIDTH, SPLIT_PANEL_MIN_WIDTH, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH,
    RIGHT_PANEL_FALLBACK_WIDTH, RIGHT_PANEL_MIN_WIDTH, RIGHT_PANEL_MAX_WIDTH,
};
pub use streaming_message::{
    DeltaEvent, StreamAction, StreamingState, ToolCallInfo, initial_streaming_state,
    normalize_tool_calls, stream_reducer,
};
