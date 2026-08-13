//! http 模块 — 请求体/表单处理 + 全局 HTTP dispatcher 配置。
//!
//! 对齐 `lib/bounded-form-data.ts` + `lib/http-dispatcher.ts`(可移植部分)。

pub mod bounded_form_data;
pub mod dispatcher;

pub use bounded_form_data::{
    check_declared_content_length, collect_body_within_limit, declared_content_length,
    parse_form_data_within_limit, BodyLimitError, RequestBodyTooLarge,
};
pub use dispatcher::{
    configure_http_dispatcher, http_dispatcher_configured, parse_http_idle_timeout_ms,
    HttpDispatcherConfig, DEFAULT_HTTP_IDLE_TIMEOUT_MS,
};
