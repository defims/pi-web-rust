//! 对齐 `lib/session-title.ts`。会话标题生成(纯逻辑部分)。
//!
//! - `parse_generated_session_title`:LLM 输出 → 干净标题(fence/JSON/首行/
//!   标签/引号/空白/标点/长度上限,按 Node 探针逐项对齐)
//! - `sanitize_title_messages`:去掉无对应 toolResult 的 toolCall 与
//!   无对应 toolCall 的 toolResult(避免标题请求触发工具)
//! - `append_title_request_to_trailing_user`:把标题请求折叠进末尾 user 消息,
//!   避免连续两条 user 消息
//! - `generate_session_title`:编排(注入 runner,运行时无关)
//!
//! 消息用 `serde_json::Value` 承载(引擎中立),由宿主与 pi_agent_rust 互转。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 对齐 `TITLE_TIMEOUT_MS`。
pub const TITLE_TIMEOUT_MS: u64 = 90_000;
/// 对齐 `MAX_TITLE_LENGTH`。
pub const MAX_TITLE_LENGTH: usize = 80;

/// 对齐 `TITLE_PROMPT`。
pub const TITLE_PROMPT: &str = "Create a concise title for this session based on the conversation above.

Requirements:
- Match the primary language used by the user.
- Describe the user's concrete goal or the outcome, not the act of chatting.
- Use 4-12 words for space-separated languages, or 8-24 characters for CJK text when practical.
- Do not call any tools.
- Return only the title as plain text, with no quotes, label, markdown, or explanation.";

/// 对齐 `GeneratedSessionTitle`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedSessionTitle {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// 对齐 `GeneratedSessionTitle.usage`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

/// 对齐 `stripWrappingQuotes`。成对引号包裹时剥掉(长度须大于引号本身)。
///
/// JS 的 `value.length` 按 UTF-16 码元计,所有引号对都是单码元,故守卫恒为
/// `chars().count() > 2`。
pub fn strip_wrapping_quotes(value: &str) -> String {
    const PAIRS: [(char, char); 6] = [
        ('"', '"'),
        ('\'', '\''),
        ('`', '`'),
        ('\u{201c}', '\u{201d}'), // “ ”
        ('\u{300c}', '\u{300d}'), // 「 」
        ('\u{300e}', '\u{300f}'), // 『 』
    ];
    for (start, end) in PAIRS {
        let start_len = start.len_utf8();
        let end_len = end.len_utf8();
        if value.starts_with(start)
            && value.ends_with(end)
            && value.chars().count() > 2
        {
            return value[start_len..value.len() - end_len].trim().to_string();
        }
    }
    value.to_string()
}

/// 对齐 `parseGeneratedSessionTitle`。返回 Err 等价 TS 的 throw。
pub fn parse_generated_session_title(raw: &str) -> Result<String, String> {
    let mut value = raw.trim().to_string();

    // ```json / ```text / ``` 围栏剥离(非贪婪,含空白)
    if let Some(captured) = strip_fenced_block(&value) {
        value = captured;
    }

    if value.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(&value) {
            if let Some(title) = parsed.get("title").and_then(|t| t.as_str()) {
                value = title.trim().to_string();
            }
        }
    }

    // 只取第一行(\r?\n)
    value = value
        .split(['\n', '\r'])
        .next()
        .unwrap_or("")
        .to_string();

    // 标签剥离:^/^(?:session\s+title|title|标题)\s*[:：-]\s*/i
    value = strip_title_label(&value);

    value = strip_wrapping_quotes(&value).split_whitespace().collect::<Vec<_>>().join(" ");

    // 尾部 。.! 剥离
    let trimmed_end = value.trim_end_matches(['。', '.', '!']);
    value = trimmed_end.trim().to_string();

    // 必须含至少一个字母/数字(Unicode)
    if !value.chars().any(|c| c.is_alphabetic() || c.is_numeric()) {
        return Err("The model did not return a usable session title".to_string());
    }

    // 按码点截断到 MAX_TITLE_LENGTH(Array.from + slice)
    if value.chars().count() > MAX_TITLE_LENGTH {
        value = value.chars().take(MAX_TITLE_LENGTH).collect::<String>().trim().to_string();
    }
    Ok(value)
}

/// `/^```(?:json|text)?\s*([\s\S]*?)\s*```$/i` 对齐。
fn strip_fenced_block(value: &str) -> Option<String> {
    let trimmed = value;
    if !trimmed.starts_with("```") || !trimmed.ends_with("```") {
        return None;
    }
    let inner = &trimmed[3..trimmed.len() - 3];
    // (?:json|text)? 可选语言标识(大小写不敏感)
    let inner = inner.trim_start();
    let inner = if inner.len() >= 4 && (inner[..4].eq_ignore_ascii_case("json") || inner[..4].eq_ignore_ascii_case("text")) {
        inner[4..].to_string()
    } else {
        inner.to_string()
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    Some(inner.to_string())
}

/// `/^(?:session\s+title|title|标题)\s*[:：-]\s*/i` 对齐。
fn strip_title_label(value: &str) -> String {
    let lower = value.to_lowercase();
    // 在原串上按标签字节长度切片(不能用小写副本切片,否则剩余部分被小写化)
    let label_len: Option<usize> = if lower.starts_with("session title") {
        Some("session title".len())
    } else if lower.starts_with("title") {
        Some("title".len())
    } else if value.starts_with('标') {
        // "标题" 前缀(CJK 无大小写)
        let chars: Vec<char> = value.chars().collect();
        if chars.len() >= 2 && chars[0] == '标' && chars[1] == '题' {
            Some(chars[0].len_utf8() + chars[1].len_utf8())
        } else {
            None
        }
    } else {
        None
    };
    let Some(label_len) = label_len else {
        return value.to_string();
    };
    // \s*[:：-]\s*  —— 分隔符可为 : ： 或 -(字面量)
    let rest = &value[label_len..];
    let rest_trimmed = rest.trim_start_matches(|c: char| c.is_whitespace());
    let Some(first) = rest_trimmed.chars().next() else {
        return value.to_string();
    };
    if first == ':' || first == '：' || first == '-' {
        let after = rest_trimmed[first.len_utf8()..].trim_start_matches(|c: char| c.is_whitespace());
        after.to_string()
    } else {
        value.to_string()
    }
}

fn message_role(message: &Value) -> Option<&str> {
    message.get("role").and_then(|r| r.as_str())
}

fn is_user_or_compaction(message: &Value) -> bool {
    matches!(message_role(message), Some("user" | "compactionSummary"))
}

/// 对齐 `appendTitleRequestToTrailingUser`。
/// 末尾是 user 消息 → 字符串 content 追加 prompt,块 content 追加 text 块。
pub fn append_title_request_to_trailing_user(messages: &[Value]) -> Vec<Value> {
    let Some(last) = messages.last() else {
        return messages.to_vec();
    };
    if message_role(last) != Some("user") {
        return messages.to_vec();
    }

    let mut new_last = last.clone();
    let content = last.get("content").cloned().unwrap_or(Value::Null);
    let new_content = match content {
        Value::String(s) => Value::String(format!("{s}\n\n{TITLE_PROMPT}")),
        Value::Array(mut blocks) => {
            blocks.push(serde_json::json!({ "type": "text", "text": TITLE_PROMPT }));
            Value::Array(blocks)
        }
        _ => Value::Null,
    };
    if let Value::Object(map) = &mut new_last {
        map.insert("content".to_string(), new_content);
    }

    let mut out = messages[..messages.len() - 1].to_vec();
    out.push(new_last);
    out
}

/// 对齐 `sanitizeTitleMessages`。
pub fn sanitize_title_messages(messages: &[Value]) -> Vec<Value> {
    let mut sanitized: Vec<Value> = Vec::new();
    let mut expected_tool_result_ids: Option<std::collections::HashSet<String>> = None;

    for (index, message) in messages.iter().enumerate() {
        match message_role(message) {
            Some("assistant") => {
                // 收集紧随其后的连续 toolResult 的 toolCallId
                let mut following_tool_result_ids = std::collections::HashSet::new();
                for result_message in &messages[index + 1..] {
                    if message_role(result_message) != Some("toolResult") {
                        break;
                    }
                    if let Some(id) = result_message.get("toolCallId").and_then(|t| t.as_str()) {
                        following_tool_result_ids.insert(id.to_string());
                    }
                }

                let mut kept_ids = std::collections::HashSet::new();
                let content = message.get("content").cloned().unwrap_or(Value::Array(Vec::new()));
                let filtered: Vec<Value> = match content {
                    Value::Array(blocks) => blocks
                        .into_iter()
                        .filter(|block| {
                            let is_tool_call = block.get("type").and_then(|t| t.as_str()) == Some("toolCall");
                            if !is_tool_call {
                                return true;
                            }
                            let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            if !following_tool_result_ids.contains(id) {
                                return false;
                            }
                            kept_ids.insert(id.to_string());
                            true
                        })
                        .collect(),
                    other => vec![other],
                };

                if !filtered.is_empty() {
                    let mut new_message = message.clone();
                    if let Value::Object(map) = &mut new_message {
                        map.insert("content".to_string(), Value::Array(filtered));
                    }
                    sanitized.push(new_message);
                }
                expected_tool_result_ids = Some(kept_ids);
            }
            Some("toolResult") => {
                let keep = expected_tool_result_ids.as_mut().map(|ids| {
                    message
                        .get("toolCallId")
                        .and_then(|t| t.as_str())
                        .map(|id| ids.remove(id))
                        .unwrap_or(false)
                });
                if keep == Some(true) {
                    sanitized.push(message.clone());
                }
            }
            _ => {
                expected_tool_result_ids = None;
                sanitized.push(message.clone());
            }
        }
    }

    sanitized
}

/// 标题生成运行器(宿主/引擎注入;async 运行时无关)。
///
/// 对应 TS `generateSessionTitle` 里临时 Agent 的 continue/prompt + 超时
/// race + abort + 从新增消息提取结果;`messages` 已含标题请求(或另发 prompt),
/// `history_length` 为 sanitize 后的原消息数(供 `assistant_result_from_messages`
/// 按 `[history_length..]` 切片新增消息)。
pub trait SessionTitleRunner {
    fn run_title(
        &self,
        messages: &[Value],
        continues_from_trailing_user: bool,
        title_prompt: &str,
        history_length: usize,
        timeout_ms: u64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<GeneratedSessionTitle, String>> + Send + '_>,
    >;
}

/// 对齐 `generateSessionTitle` 的编排逻辑(引擎回调注入)。
pub async fn generate_session_title<R: SessionTitleRunner + ?Sized>(
    runner: &R,
    messages: &[Value],
) -> Result<GeneratedSessionTitle, String> {
    let sanitized = sanitize_title_messages(messages);
    let history_length = sanitized.len();
    if !sanitized.iter().any(is_user_or_compaction) {
        return Err("The session has no user messages to name".to_string());
    }

    let continues_from_trailing_user =
        sanitized.last().map(|m| message_role(m) == Some("user")).unwrap_or(false);
    let final_messages = if continues_from_trailing_user {
        append_title_request_to_trailing_user(&sanitized)
    } else {
        sanitized
    };

    runner
        .run_title(
            &final_messages,
            continues_from_trailing_user,
            TITLE_PROMPT,
            history_length,
            TITLE_TIMEOUT_MS,
        )
        .await
}

/// 对齐 `getAssistantResult`:扫描新增消息里最后一条带文本的 assistant 消息。
///
/// `messages` 为完整消息列表(引擎侧 `agent.state.messages`),
/// `history_length` 为 sanitize 后的原长度;只扫描 `[history_length..]` 新增部分。
pub fn assistant_result_from_messages(
    messages: &[Value],
    history_length: usize,
) -> Result<GeneratedSessionTitle, String> {
    for message in messages.iter().skip(history_length).rev() {
        if message_role(message) != Some("assistant") {
            continue;
        }
        if message.get("stopReason").and_then(|s| s.as_str()) == Some("error") {
            let detail = message
                .get("errorMessage")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
                .unwrap_or("The title model request failed");
            return Err(detail.to_string());
        }

        let text = message
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }

        let usage = message.get("usage").and_then(|u| u.as_object()).map(|u| Usage {
            input: u.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
            output: u.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_read: u.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_write: u.get("cacheWrite").and_then(|v| v.as_u64()).unwrap_or(0),
            total: u.get("totalTokens").and_then(|v| v.as_u64()).unwrap_or(0),
        });

        return Ok(GeneratedSessionTitle {
            title: parse_generated_session_title(&text)?,
            usage,
        });
    }
    Err("The model did not return a session title".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_plain_and_trim() {
        assert_eq!(parse_generated_session_title("  My session title  ").unwrap(), "My session title");
        assert_eq!(parse_generated_session_title("  spaced   out   words  ").unwrap(), "spaced out words");
    }

    #[test]
    fn parse_fenced_blocks() {
        assert_eq!(parse_generated_session_title("```text\nFenced title\n```").unwrap(), "Fenced title");
        assert_eq!(
            parse_generated_session_title("```json\n{\"title\": \"JSON Title\"}\n```").unwrap(),
            "JSON Title"
        );
        assert_eq!(parse_generated_session_title("```\nPlain fence\n```").unwrap(), "Plain fence");
        assert_eq!(parse_generated_session_title("```TEXT\nUpper lang\n```").unwrap(), "Upper lang");
    }

    #[test]
    fn parse_json_object() {
        assert_eq!(
            parse_generated_session_title("{\"title\": \"Object Title\"}").unwrap(),
            "Object Title"
        );
        // JSON 但 title 非字符串 → 回落文本清理
        assert_eq!(
            parse_generated_session_title("{\"title\": 42}").unwrap(),
            "{\"title\": 42}"
        );
        // 非法 JSON → 回落为纯文本(含字母 → 通过校验)
        assert_eq!(parse_generated_session_title("{broken}").unwrap(), "{broken}");
    }

    #[test]
    fn parse_label_strip() {
        assert_eq!(parse_generated_session_title("Title: Prefixed").unwrap(), "Prefixed");
        assert_eq!(parse_generated_session_title("title: Lower").unwrap(), "Lower");
        assert_eq!(parse_generated_session_title("session title: With Prefix").unwrap(), "With Prefix");
        assert_eq!(parse_generated_session_title("标题：中文标题").unwrap(), "中文标题");
        assert_eq!(parse_generated_session_title("标题: 中文").unwrap(), "中文");
        // '-' 也是合法分隔符(探针确认)
        assert_eq!(parse_generated_session_title("Session title - dash").unwrap(), "dash");
    }

    #[test]
    fn parse_first_line_only() {
        assert_eq!(parse_generated_session_title("Line one\nLine two").unwrap(), "Line one");
        assert_eq!(parse_generated_session_title("Line one\r\nLine two").unwrap(), "Line one");
    }

    #[test]
    fn parse_quote_strip() {
        assert_eq!(parse_generated_session_title("\"Quoted title\"").unwrap(), "Quoted title");
        assert_eq!(parse_generated_session_title("'Single quoted'").unwrap(), "Single quoted");
        assert_eq!(parse_generated_session_title("`Backticked`").unwrap(), "Backticked");
        assert_eq!(parse_generated_session_title("\u{201c}Curly\u{201d}").unwrap(), "Curly");
        assert_eq!(parse_generated_session_title("\u{300c}Bracketed\u{300d}").unwrap(), "Bracketed");
        assert_eq!(
            parse_generated_session_title("\u{300e}Double bracket\u{300f}").unwrap(),
            "Double bracket"
        );
        // 空引号对:长度不满足守卫 → 不剥离 → 无字母 → 报错
        assert_eq!(
            parse_generated_session_title("\"\"").unwrap_err(),
            "The model did not return a usable session title"
        );
    }

    #[test]
    fn parse_trailing_punctuation() {
        assert_eq!(parse_generated_session_title("Ends with period.").unwrap(), "Ends with period");
        assert_eq!(parse_generated_session_title("结束句号。").unwrap(), "结束句号");
        assert_eq!(parse_generated_session_title("Emphasis!!").unwrap(), "Emphasis");
    }

    #[test]
    fn parse_rejects_no_letters() {
        assert_eq!(
            parse_generated_session_title("!!!").unwrap_err(),
            "The model did not return a usable session title"
        );
        assert_eq!(
            parse_generated_session_title("\u{1f525}").unwrap_err(), // 🔥 仅 emoji
            "The model did not return a usable session title"
        );
    }

    #[test]
    fn parse_length_cap_by_code_points() {
        let long = "123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890ABCDE";
        let result = parse_generated_session_title(long).unwrap();
        assert_eq!(result.chars().count(), MAX_TITLE_LENGTH);
        // emoji 按码点截断,不拆 UTF-8 字节
        let emoji_title = format!("{}{}", "a".repeat(70), "🔤".repeat(6));
        let result = parse_generated_session_title(&emoji_title).unwrap();
        assert!(result.chars().count() <= MAX_TITLE_LENGTH);
    }

    #[test]
    fn append_to_string_content() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
        ];
        let out = append_title_request_to_trailing_user(&messages);
        assert_eq!(out.len(), 1);
        assert!(out[0]["content"].as_str().unwrap().starts_with("hello\n\nCreate a concise title"));
    }

    #[test]
    fn append_to_block_content() {
        let messages = vec![
            json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] }),
        ];
        let out = append_title_request_to_trailing_user(&messages);
        let blocks = out[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["type"], "text");
        assert!(blocks[1]["text"].as_str().unwrap().starts_with("Create a concise title"));
    }

    #[test]
    fn append_skips_non_user_last() {
        let messages = vec![
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": "ok" }),
        ];
        let out = append_title_request_to_trailing_user(&messages);
        assert_eq!(out, messages);
    }

    #[test]
    fn append_empty() {
        let out: Vec<Value> = append_title_request_to_trailing_user(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn sanitize_keeps_paired_tool_call() {
        let messages = vec![
            json!({ "role": "user", "content": "run it" }),
            json!({
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "ok" },
                    { "type": "toolCall", "id": "call1", "name": "bash" }
                ]
            }),
            json!({ "role": "toolResult", "toolCallId": "call1", "content": "done" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "finished" }] }),
        ];
        let out = sanitize_title_messages(&messages);
        assert_eq!(out.len(), 4);
        let tool_call_kept = out[1]["content"].as_array().unwrap().iter().any(|b| {
            b.get("type").and_then(|t| t.as_str()) == Some("toolCall")
        });
        assert!(tool_call_kept);
        // toolResult 保留
        assert!(out.iter().any(|m| message_role(m) == Some("toolResult")));
    }

    #[test]
    fn sanitize_drops_orphan_tool_call() {
        let messages = vec![
            json!({
                "role": "assistant",
                "content": [
                    { "type": "toolCall", "id": "callX", "name": "bash" }
                ]
            }),
            json!({ "role": "user", "content": "next" }),
        ];
        let out = sanitize_title_messages(&messages);
        // assistant content 过滤后为空 → 整条跳过
        assert_eq!(out.len(), 1);
        assert_eq!(message_role(&out[0]), Some("user"));
    }

    #[test]
    fn sanitize_drops_unmatched_tool_result() {
        let messages = vec![
            json!({ "role": "toolResult", "toolCallId": "orphan", "content": "x" }),
            json!({ "role": "user", "content": "hi" }),
        ];
        let out = sanitize_title_messages(&messages);
        assert_eq!(out.len(), 1);
        assert_eq!(message_role(&out[0]), Some("user"));
    }

    #[test]
    fn sanitize_resets_on_other_roles() {
        let messages = vec![
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "a" }] }),
            json!({ "role": "toolResult", "toolCallId": "call1", "content": "x" }),
            json!({ "role": "user", "content": "reset" }),
            json!({ "role": "toolResult", "toolCallId": "call1", "content": "y" }),
        ];
        let out = sanitize_title_messages(&messages);
        // 第一条 assistant 无 toolCall → expected 集合为空 → toolResult(call1) 被丢弃
        // user 之后 expected 重置 → 第二个 toolResult 也被丢弃
        assert_eq!(out.len(), 2);
        assert_eq!(message_role(&out[0]), Some("assistant"));
        assert_eq!(message_role(&out[1]), Some("user"));
    }

    #[test]
    fn assistant_result_extracts_text_and_usage() {
        let messages = vec![
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "  My Title  " }],
                    "usage": { "input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1, "totalTokens": 15 } }),
        ];
        let result = assistant_result_from_messages(&messages, 0).unwrap();
        assert_eq!(result.title, "My Title");
        let usage = result.usage.unwrap();
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 5);
        assert_eq!(usage.cache_read, 2);
        assert_eq!(usage.cache_write, 1);
        assert_eq!(usage.total, 15);
    }

    #[test]
    fn assistant_result_skips_empty_and_picks_last() {
        let messages = vec![
            json!({ "role": "assistant", "content": [] }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "real" }] }),
            json!({ "role": "assistant", "content": [{ "type": "thinking", "text": "t" }] }),
        ];
        let result = assistant_result_from_messages(&messages, 0).unwrap();
        assert_eq!(result.title, "real");
    }

    #[test]
    fn assistant_result_error_stop_reason() {
        let messages = vec![
            json!({ "role": "assistant", "stopReason": "error", "content": [{ "type": "text", "text": "x" }] }),
        ];
        assert_eq!(
            assistant_result_from_messages(&messages, 0).unwrap_err(),
            "The title model request failed"
        );
        let messages = vec![
            json!({ "role": "assistant", "stopReason": "error", "errorMessage": "boom", "content": [] }),
        ];
        assert_eq!(assistant_result_from_messages(&messages, 0).unwrap_err(), "boom");
    }

    #[test]
    fn assistant_result_none_found() {
        let messages = vec![json!({ "role": "user", "content": "hi" })];
        assert_eq!(
            assistant_result_from_messages(&messages, 0).unwrap_err(),
            "The model did not return a session title"
        );
    }

    #[test]
    fn history_length_slices() {
        let messages = vec![
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "OLD" }] }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "NEW" }] }),
        ];
        // 只扫描 [1..],得到 NEW
        let result = assistant_result_from_messages(&messages, 1).unwrap();
        assert_eq!(result.title, "NEW");
    }

    struct FakeRunner;

    impl SessionTitleRunner for FakeRunner {
        fn run_title(
            &self,
            messages: &[Value],
            continues: bool,
            title_prompt: &str,
            history_length: usize,
            timeout_ms: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<GeneratedSessionTitle, String>> + Send + '_>> {
            let messages = messages.to_vec();
            let title_prompt = title_prompt.to_string();
            Box::pin(async move {
                assert_eq!(title_prompt, TITLE_PROMPT);
                assert_eq!(timeout_ms, TITLE_TIMEOUT_MS);
                assert!(history_length >= 1);
                let last = messages.last().unwrap();
                let continues_expected = last.get("role").and_then(|r| r.as_str()) == Some("user");
                assert_eq!(continues, continues_expected);
                Ok(GeneratedSessionTitle { title: "generated".to_string(), usage: None })
            })
        }
    }

    #[tokio::test]
    async fn generate_orchestration() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] }),
        ];
        let result = generate_session_title(&FakeRunner, &messages).await.unwrap();
        assert_eq!(result.title, "generated");
    }

    #[tokio::test]
    async fn generate_no_user_errors() {
        let messages = vec![json!({ "role": "assistant", "content": "hi" })];
        assert_eq!(
            generate_session_title(&FakeRunner, &messages).await.unwrap_err(),
            "The session has no user messages to name"
        );
    }

    #[tokio::test]
    async fn generate_trailing_user_folds_prompt() {
        let messages = vec![json!({ "role": "user", "content": "hello" })];
        let result = generate_session_title(&FakeRunner, &messages).await.unwrap();
        assert_eq!(result.title, "generated");
    }

    #[test]
    fn serialize_shapes() {
        let title = GeneratedSessionTitle {
            title: "T".to_string(),
            usage: Some(Usage {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 4,
                total: 5,
            }),
        };
        let json = serde_json::to_value(&title).unwrap();
        assert_eq!(json["title"], "T");
        assert_eq!(json["usage"]["cacheRead"], 3);
        assert_eq!(json["usage"]["total"], 5);

        let no_usage = GeneratedSessionTitle { title: "T".to_string(), usage: None };
        let json = serde_json::to_value(&no_usage).unwrap();
        assert!(json.get("usage").is_none());
    }
}
