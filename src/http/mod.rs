//! http 模块 — 请求体/表单处理。
//!
//! 对齐 `lib/bounded-form-data.ts`:在解析 multipart 之前先约束完整的线上请求体,
//! 同样限制 chunked 请求(Content-Length 缺失或不可信)。

pub mod bounded_form_data;

pub use bounded_form_data::{
    RequestBodyTooLarge, check_declared_content_length, collect_body_within_limit,
    parse_form_data_within_limit,
};
