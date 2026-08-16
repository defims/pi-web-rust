//! 对齐 `lib/session-reader.ts` 的纯计算部分。
//!
//! jsonl 会话 entry → UI 消息(AgentMessage 形状)的转换,以及
//! `buildSessionContext` 的编排(SDK 上下文选择注入回调)。
//!
//! - `parse_entry_timestamp`:`Date.parse` 的 ISO 8601 子集(会话文件为 UTC)
//! - `base64_image_info` / `omit_tool_result_base64_images`:从初始历史载荷
//!   省略工具结果图片(防止体积爆炸)
//! - `entry_to_ui_message`:message / compaction / branch_summary / custom_message
//!   分支转换

use std::collections::HashMap;

use serde_json::Value;

use crate::image::get_base64_decoded_byte_length;

/// 对齐 `parseEntryTimestamp`。Date.parse 的 ISO 8601 子集(带 Z/±HH:MM 或裸日期),
/// 失败返回 None。会话文件时间戳由 pi 生成,均为 UTC ISO 格式。
pub fn parse_entry_timestamp(timestamp: &str) -> Option<i64> {
    parse_iso8601_millis(timestamp)
}

/// 解析 `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)` 或 `YYYY-MM-DD`(UTC 语义)。
fn parse_iso8601_millis(value: &str) -> Option<i64> {
    let b = value.as_bytes();
    let mut pos = 0;
    // YYYY-MM-DD
    let year = parse_digits(b, &mut pos, 4)?;
    if b.get(pos) != Some(&b'-') {
        return None;
    }
    pos += 1;
    let month = parse_digits(b, &mut pos, 2)?;
    if b.get(pos) != Some(&b'-') {
        return None;
    }
    pos += 1;
    let day = parse_digits(b, &mut pos, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (hour, minute, second, millis) = if pos == b.len() {
        // 裸日期 → UTC 零点
        (0u32, 0u32, 0u32, 0u32)
    } else {
        if b.get(pos) != Some(&b'T') {
            return None;
        }
        pos += 1;
        let hour = parse_digits(b, &mut pos, 2)?;
        if b.get(pos) != Some(&b':') {
            return None;
        }
        pos += 1;
        let minute = parse_digits(b, &mut pos, 2)?;
        if b.get(pos) != Some(&b':') {
            return None;
        }
        pos += 1;
        let second = parse_digits(b, &mut pos, 2)?;
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        // 可选毫秒小数
        let mut millis = 0u32;
        if b.get(pos) == Some(&b'.') {
            pos += 1;
            let mut frac_digits = 0u32;
            while pos < b.len() && b[pos].is_ascii_digit() && frac_digits < 3 {
                millis = millis * 10 + (b[pos] - b'0') as u32;
                frac_digits += 1;
                pos += 1;
            }
            // 补齐到 3 位
            while frac_digits < 3 {
                millis *= 10;
                frac_digits += 1;
            }
        }
        (hour, minute, second, millis)
    };

    // 时区:Z / ±HH:MM / ±HHMM / 无(UTC)
    let (utc_hour, utc_minute) = if pos == b.len() {
        (0i32, 0i32)
    } else if b[pos] == b'Z' || b[pos] == b'z' {
        if pos + 1 != b.len() {
            return None;
        }
        (0, 0)
    } else if b[pos] == b'+' || b[pos] == b'-' {
        let sign = if b[pos] == b'-' { -1 } else { 1 };
        pos += 1;
        let offset_hour = parse_digits(b, &mut pos, 2)? as i32;
        let offset_minute = if b.get(pos) == Some(&b':') {
            pos += 1;
            parse_digits(b, &mut pos, 2)? as i32
        } else {
            parse_digits(b, &mut pos, 2)? as i32
        };
        if offset_hour > 23 || offset_minute > 59 || pos != b.len() {
            return None;
        }
        (sign * offset_hour, sign * offset_minute)
    } else {
        return None;
    };

    let utc_hour = hour as i32 - utc_hour;
    let utc_minute = minute as i32 - utc_minute;

    let days = days_from_civil(year as i64, month, day)?;
    days.checked_mul(86_400_000)?
        .checked_add((utc_hour as i64 * 3600 + utc_minute as i64 * 60 + second as i64) * 1000)?
        .checked_add(millis as i64)
}

fn parse_digits(b: &[u8], pos: &mut usize, count: usize) -> Option<u32> {
    if *pos + count > b.len() {
        return None;
    }
    let mut value = 0u32;
    for _ in 0..count {
        let c = b[*pos];
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (c - b'0') as u32;
        *pos += 1;
    }
    Some(value)
}

/// 公历 → 自 1970-01-01 的天数(Howard Hinnant 算法)。
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// 对齐 `base64ImageInfo`。识别 base64 图片块并估算字节数 + mime。
pub fn base64_image_info(block: &Value) -> Option<(usize, Option<String>)> {
    if !is_record(block) || block.get("type").and_then(|t| t.as_str()) != Some("image") {
        return None;
    }

    let (data, mime): (String, Option<String>) =
        if let Some(data) = block.get("data").and_then(|d| d.as_str()) {
            (
                data.to_string(),
                block
                    .get("mimeType")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string()),
            )
        } else if let Some(source) = block.get("source").filter(|s| s.is_object()) {
            if source.get("type").and_then(|t| t.as_str()) == Some("base64") {
                if let Some(data) = source.get("data").and_then(|d| d.as_str()) {
                    let mime = source
                        .get("media_type")
                        .and_then(|m| m.as_str())
                        .or_else(|| source.get("mediaType").and_then(|m| m.as_str()))
                        .map(|s| s.to_string());
                    (data.to_string(), mime)
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else {
            return None;
        };

    let bytes = get_base64_decoded_byte_length(&data)?;
    Some((bytes, mime))
}

/// 对齐 `omitToolResultBase64Images`。把 toolResult 里的 base64 图片替换成
/// 一条文本占位,返回是否发生了省略(无图片时原样返回)。
pub fn omit_tool_result_base64_images(message: &Value) -> Value {
    if message.get("role").and_then(|r| r.as_str()) != Some("toolResult") {
        return message.clone();
    }

    let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
        return message.clone();
    };
    let mut omitted = 0usize;
    let mut bytes = 0usize;
    let mut mimes: Vec<String> = Vec::new();
    let kept: Vec<Value> = blocks
        .iter()
        .filter(|block| match base64_image_info(block) {
            Some((b, mime)) => {
                omitted += 1;
                bytes += b;
                if let Some(mime) = mime {
                    if !mimes.contains(&mime) {
                        mimes.push(mime);
                    }
                }
                false
            }
            None => true,
        })
        .cloned()
        .collect();
    if omitted == 0 {
        return message.clone();
    }

    let mime_text = if mimes.is_empty() {
        String::new()
    } else {
        format!(": {}", mimes.join(", "))
    };
    let plural = if omitted == 1 { "" } else { "s" };
    let mut content = kept;
    content.push(serde_json::json!({
        "type": "text",
        "text": format!("[{omitted} tool result image{plural} omitted from initial history payload{mime_text}, ~{bytes} bytes]"),
    }));

    let mut out = message.clone();
    if let Value::Object(map) = &mut out {
        map.insert("content".to_string(), Value::Array(content));
    }
    out
}

/// 对齐 `entryToUiMessage` 的 `message` 分支(assistant 的 thinking defer)。
fn defer_thinking_blocks(message: Value) -> Value {
    let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
        return message;
    };
    let deferred: Vec<Value> = blocks
        .iter()
        .map(|block| {
            if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                let text = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                if !text.trim().is_empty() {
                    let mut b = block.clone();
                    if let Value::Object(map) = &mut b {
                        map.insert("thinking".to_string(), Value::String(String::new()));
                        map.insert("deferred".to_string(), Value::Bool(true));
                    }
                    return b;
                }
            }
            block.clone()
        })
        .collect();
    let mut out = message;
    if let Value::Object(map) = &mut out {
        map.insert("content".to_string(), Value::Array(deferred));
    }
    out
}

/// 对齐 `entryToUiMessage`。把 session entry 转成 UI 消息(AgentMessage 形状)。
/// 返回 None 的 entry 不进入聊天历史(元数据/非消息类型)。
///
/// `defer_thinking` / `defer_tool_result_images` 对应 `options`;工具调用字段
/// 归一化由调用方负责(可先用 `ui::streaming_message::normalize_tool_calls`)。
pub fn entry_to_ui_message(
    entry: &Value,
    defer_thinking: bool,
    defer_tool_result_images: bool,
) -> Option<Value> {
    let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let timestamp = entry
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(parse_entry_timestamp);
    match entry_type {
        "message" => {
            let raw_message = entry.get("message")?;
            let message = if defer_tool_result_images {
                omit_tool_result_base64_images(raw_message)
            } else {
                raw_message.clone()
            };
            let message = crate::ui::streaming_message::normalize_tool_calls(&message);
            if !defer_thinking {
                return Some(message);
            }
            if message.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                return Some(message);
            }
            Some(defer_thinking_blocks(message))
        }
        "compaction" => {
            // 对齐 TS:总是产出(不因缺 summary 而丢条)。content = entry.summary,
            // 缺失(undefined)→ 省略该字段;null/字符串 → 含。
            let mut details = serde_json::Map::new();
            if let Some(tokens_before) = entry.get("tokensBefore") {
                details.insert("tokensBefore".to_string(), tokens_before.clone());
            }
            if let Some(first_kept) = entry.get("firstKeptEntryId") {
                details.insert("firstKeptEntryId".to_string(), first_kept.clone());
            }
            let mut obj = serde_json::Map::new();
            obj.insert("role".to_string(), Value::String("custom".to_string()));
            obj.insert(
                "customType".to_string(),
                Value::String("compaction".to_string()),
            );
            if let Some(summary) = entry.get("summary") {
                obj.insert("content".to_string(), summary.clone());
            }
            obj.insert("display".to_string(), Value::Bool(true));
            obj.insert("details".to_string(), Value::Object(details));
            obj.insert("timestamp".to_string(), serde_json::json!(timestamp));
            Some(Value::Object(obj))
        }
        "branch_summary" => {
            // 对齐 TS `if (!entry.summary) return null`:缺失或空串 → null。
            let Some(summary) = entry
                .get("summary")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
            else {
                return None;
            };
            Some(serde_json::json!({
                "role": "user",
                "content": format!("*The conversation briefly explored another branch and returned with this summary:*\n\n{summary}"),
                "timestamp": timestamp,
            }))
        }
        "custom_message" => {
            // 对齐 TS:customType/content/display/details 各自「存在则含(含 null/false),
            // 缺失则省略」;不因缺字段而丢弃整条。
            let mut obj = serde_json::Map::new();
            obj.insert("role".to_string(), Value::String("custom".to_string()));
            if let Some(ct) = entry.get("customType") {
                obj.insert("customType".to_string(), ct.clone());
            }
            if let Some(c) = entry.get("content") {
                obj.insert("content".to_string(), c.clone());
            }
            if let Some(d) = entry.get("display") {
                obj.insert("display".to_string(), d.clone());
            }
            if let Some(det) = entry.get("details") {
                obj.insert("details".to_string(), det.clone());
            }
            obj.insert("timestamp".to_string(), serde_json::json!(timestamp));
            Some(Value::Object(obj))
        }
        _ => None,
    }
}

/// 对齐 `buildSessionContext` 的编排。
///
/// `sdk_context` 注入 SDK 的 `piBuildSessionContext` 结果
/// (`{ thinkingLevel, model }`),`context_entries` 注入 SDK 选中的上下文条目
/// (按 pi 的 compaction 顺序)。`entry_to_ui_message` 由本模块实现。
pub fn build_session_context(
    _entries: &[Value],
    sdk_context: SdkContext,
    context_entries: &[Value],
    defer_thinking: bool,
    defer_tool_result_images: bool,
) -> SessionContext {
    let mut messages: Vec<Value> = Vec::new();
    let mut entry_ids: Vec<String> = Vec::new();
    for entry in context_entries {
        if let Some(message) = entry_to_ui_message(entry, defer_thinking, defer_tool_result_images)
        {
            entry_ids.push(
                entry
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            messages.push(message);
        }
    }

    SessionContext {
        messages,
        entry_ids,
        thinking_level: sdk_context.thinking_level,
        model: sdk_context.model,
    }
}

/// SDK `piBuildSessionContext` 的返回值。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SdkContext {
    pub thinking_level: Option<String>,
    pub model: Option<Value>,
}

/// 引擎接线:直接调 `pi::sdk::build_session_context`。
///
/// 把 JSON entries 反序列化为 `pi::SessionEntry`,构建 entry_index,
/// 调引擎的 build_session_context(含 compaction 截断 + thinking/model 提取),
/// 然后把返回的 `Message` 列表转回 UI 形状(Value)。
///
/// `defer_thinking` / `defer_tool_result_images` 对应原 options。
pub fn build_session_context_from_json(
    entries: &[Value],
    leaf_id: Option<&str>,
    defer_thinking: bool,
    defer_tool_result_images: bool,
) -> SessionContext {
    // 1. 反序列化为 pi::SessionEntry
    // 注意:by_id 的下标必须对应 pi_entries(过滤坏行后的列表),而非原始
    // entries —— 否则任何坏行都会让后续成功条目的下标错位,引擎沿 path
    // 索引时越界 panic(实测:坏行 + 有效链 = index out of bounds)。
    let mut pi_entries: Vec<pi::session::SessionEntry> = Vec::with_capacity(entries.len());
    let mut entry_ids: Vec<String> = Vec::with_capacity(entries.len());
    let mut by_id: std::collections::HashMap<String, usize> = HashMap::new();

    for raw in entries.iter() {
        if let Ok(entry) = serde_json::from_value::<pi::session::SessionEntry>(raw.clone()) {
            if let Some(id) = entry.base().id.as_ref() {
                by_id.insert(id.clone(), pi_entries.len());
                entry_ids.push(id.clone());
            } else {
                entry_ids.push(String::new());
            }
            pi_entries.push(entry);
        } else {
            entry_ids.push(String::new());
        }
    }

    // 2. 调引擎 build_session_context
    // leaf_id=None 时用最后一条 entry 的 id(对齐 TS buildSessionContext 的回退)
    let effective_leaf_id =
        leaf_id.or_else(|| pi_entries.last().and_then(|e| e.base().id.as_deref()));
    let snapshot = pi::sdk::build_session_context(&pi_entries, effective_leaf_id, &by_id);

    // 3. Message → UI Value
    let messages: Vec<Value> = snapshot
        .messages
        .iter()
        .map(|msg| {
            let mut value = serde_json::to_value(msg).unwrap_or(Value::Null);
            // 应用 defer 选项
            if defer_thinking || defer_tool_result_images {
                value = apply_defer_options(value, defer_thinking, defer_tool_result_images);
            }
            value
        })
        .collect();

    // 4. 派生 entry_ids(引擎选中的 path 上的 entry id)
    // build_session_context 内部走了 path,但 SessionContextSnapshot 不返回 path;
    // 暂用全部 entry_ids(对齐 TS buildContextEntries 返回全部的简化场景)
    let context_entry_ids: Vec<String> = pi_entries
        .iter()
        .filter_map(|e| e.base().id.clone())
        .collect();

    SessionContext {
        messages,
        entry_ids: context_entry_ids,
        thinking_level: snapshot.thinking_level,
        model: snapshot
            .model
            .map(|(provider, id)| serde_json::json!({ "provider": provider, "id": id })),
    }
}

/// 对 defer_thinking / defer_tool_result_images 的后处理。
fn apply_defer_options(
    mut message: Value,
    defer_thinking: bool,
    defer_tool_result_images: bool,
) -> Value {
    if defer_tool_result_images
        && message.get("role").and_then(|r| r.as_str()) == Some("toolResult")
    {
        message = omit_tool_result_base64_images(&message);
    }
    if defer_thinking && message.get("role").and_then(|r| r.as_str()) == Some("assistant") {
        if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
            let deferred: Vec<Value> = content
                .iter()
                .map(|block| {
                    if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                        let text = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                        if !text.trim().is_empty() {
                            let mut b = block.clone();
                            if let Value::Object(map) = &mut b {
                                map.insert("thinking".to_string(), Value::String(String::new()));
                                map.insert("deferred".to_string(), Value::Bool(true));
                            }
                            return b;
                        }
                    }
                    block.clone()
                })
                .collect();
            if let Value::Object(map) = &mut message {
                map.insert("content".to_string(), Value::Array(deferred));
            }
        }
    }
    message
}

/// 对齐 `SessionContext` 返回形状。
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct SessionContext {
    pub messages: Vec<Value>,
    #[serde(rename = "entryIds")]
    pub entry_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinkingLevel")]
    pub thinking_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timestamp_iso_utc() {
        assert_eq!(
            parse_entry_timestamp("2024-01-01T00:00:00Z"),
            Some(1704067200000)
        );
        assert_eq!(
            parse_entry_timestamp("2024-01-01T00:00:00.000Z"),
            Some(1704067200000)
        );
        assert_eq!(
            parse_entry_timestamp("2024-01-01T00:00:00.123Z"),
            Some(1704067200123)
        );
        assert_eq!(parse_entry_timestamp("2024-01-01"), Some(1704067200000));
        assert_eq!(
            parse_entry_timestamp("2024-01-01T00:00:00+08:00"),
            Some(1704038400000)
        );
        assert_eq!(
            parse_entry_timestamp("2024-01-01T08:00:00+0800"),
            Some(1704067200000)
        );
        assert_eq!(
            parse_entry_timestamp("2024-01-01T00:00:00"),
            Some(1704067200000)
        );
        assert_eq!(parse_entry_timestamp("not-a-date"), None);
        assert_eq!(parse_entry_timestamp(""), None);
        assert_eq!(parse_entry_timestamp("1714608000000"), None);
        assert_eq!(parse_entry_timestamp(" 2024-01-01T00:00:00Z"), None);
        assert_eq!(parse_entry_timestamp("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_entry_timestamp("2024-01-01T25:00:00Z"), None);
    }

    #[test]
    fn base64_image_detection() {
        // "aGVsbG8=" = "hello"(5 字节)
        let block = json!({ "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" });
        assert_eq!(
            base64_image_info(&block),
            Some((5, Some("image/png".to_string())))
        );
        // source 形状(media_type)
        let block = json!({ "type": "image", "source": { "type": "base64", "data": "aGVsbG8=", "media_type": "image/jpeg" } });
        assert_eq!(
            base64_image_info(&block),
            Some((5, Some("image/jpeg".to_string())))
        );
        // 非图片 / url source → None
        assert_eq!(base64_image_info(&json!({ "type": "text" })), None);
        assert_eq!(
            base64_image_info(&json!({ "type": "image", "source": { "type": "url", "url": "x" } })),
            None
        );
        // 非法 base64 → None
        assert_eq!(
            base64_image_info(&json!({ "type": "image", "data": "not!!base64" })),
            None
        );
    }

    #[test]
    fn omit_images_replaces_with_text() {
        let message = json!({
            "role": "toolResult",
            "toolCallId": "c1",
            "content": [
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" },
                { "type": "text", "text": "keep me" },
            ]
        });
        let out = omit_tool_result_base64_images(&message);
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["text"], "keep me");
        let placeholder = content[1]["text"].as_str().unwrap();
        assert!(placeholder.starts_with("[1 tool result image omitted"));
        assert!(placeholder.contains("image/png"));
        assert!(placeholder.contains("~5 bytes"));
    }

    #[test]
    fn omit_images_passthrough() {
        let message = json!({ "role": "toolResult", "toolCallId": "c1", "content": [{ "type": "text", "text": "x" }] });
        assert_eq!(omit_tool_result_base64_images(&message), message);
        // 非 toolResult 原样
        let user = json!({ "role": "user", "content": "hi" });
        assert_eq!(omit_tool_result_base64_images(&user), user);
    }

    #[test]
    fn entry_message_with_thinking_defer() {
        let entry = json!({
            "type": "message",
            "id": "e1",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "  secret  " },
                    { "type": "text", "text": "answer" }
                ]
            }
        });
        let out = entry_to_ui_message(&entry, true, false).unwrap();
        let blocks = out["content"].as_array().unwrap();
        assert_eq!(blocks[0]["thinking"], "");
        assert_eq!(blocks[0]["deferred"], true);
        // 非 assistant 消息不受 defer 影响
        let user_entry =
            json!({ "type": "message", "message": { "role": "user", "content": "hi" } });
        let out = entry_to_ui_message(&user_entry, true, false).unwrap();
        assert_eq!(out["role"], "user");
    }

    #[test]
    fn entry_compaction() {
        let entry = json!({
            "type": "compaction",
            "id": "e2",
            "summary": "Summarized earlier",
            "tokensBefore": 1000,
            "firstKeptEntryId": "e9",
            "timestamp": "2024-01-01T00:00:00Z"
        });
        let out = entry_to_ui_message(&entry, true, false).unwrap();
        assert_eq!(out["role"], "custom");
        assert_eq!(out["customType"], "compaction");
        assert_eq!(out["content"], "Summarized earlier");
        assert_eq!(out["display"], true);
        assert_eq!(out["details"]["tokensBefore"], 1000);
        assert_eq!(out["details"]["firstKeptEntryId"], "e9");
        assert_eq!(out["timestamp"], 1704067200000i64);
    }

    #[test]
    fn entry_branch_summary() {
        let entry = json!({ "type": "branch_summary", "summary": "Explored X", "timestamp": "2024-01-01T00:00:00Z" });
        let out = entry_to_ui_message(&entry, true, false).unwrap();
        assert_eq!(out["role"], "user");
        assert!(out["content"].as_str().unwrap().contains("Explored X"));
        // 无 summary → None
        assert!(entry_to_ui_message(&json!({ "type": "branch_summary" }), true, false).is_none());
    }

    #[test]
    fn entry_custom_message() {
        let entry = json!({
            "type": "custom_message",
            "customType": "reminder",
            "content": "Remember",
            "display": false,
            "details": { "k": "v" },
            "timestamp": "2024-01-01T00:00:00Z"
        });
        let out = entry_to_ui_message(&entry, true, false).unwrap();
        assert_eq!(out["role"], "custom");
        assert_eq!(out["customType"], "reminder");
        assert_eq!(out["content"], "Remember");
        assert_eq!(out["display"], false);
        assert_eq!(out["details"]["k"], "v");
    }

    #[test]
    fn unknown_entry_is_none() {
        assert!(entry_to_ui_message(&json!({ "type": "metadata" }), true, false).is_none());
        assert!(entry_to_ui_message(&json!({ "type": "branch_point" }), true, false).is_none());
    }

    #[test]
    fn build_context_uses_sdk_entries() {
        let entries = vec![
            json!({ "type": "message", "id": "a", "message": { "role": "user", "content": "hi" } }),
            json!({ "type": "message", "id": "b", "message": { "role": "assistant", "content": "ok" } }),
        ];
        // SDK 只选中第一条
        let sdk = SdkContext {
            thinking_level: Some("high".to_string()),
            model: Some(json!({ "id": "m1" })),
        };
        let ctx = build_session_context(&entries, sdk, &entries[..1], true, true);
        assert_eq!(ctx.messages.len(), 1);
        assert_eq!(ctx.entry_ids, vec!["a".to_string()]);
        assert_eq!(ctx.thinking_level.as_deref(), Some("high"));
        assert_eq!(ctx.model, Some(json!({ "id": "m1" })));

        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["entryIds"][0], "a");
        assert_eq!(json["thinkingLevel"], "high");
    }
}
