//! 对齐 `lib/streaming-message.ts` + `lib/normalize.ts`(normalizeToolCalls 部分)。
//!
//! 流式消息 reducer:把 SDK 的 message_update delta 事件投影到正在生成的
//! assistant 消息上。纯状态机,消息/块用 serde_json::Value 承载(引擎中立,
//! 由宿主与 pi_agent_rust 互转)。语义按 TS 逐项对齐:
//! - contentIndex 非法(非整数 / < 0)→ 原状态不变
//! - update 返回 null(如 text_delta 作用在非 text 块上)→ 原状态不变
//! - toolcall_end 无条件替换为 toolCall 块(丢弃原块字段)

use serde_json::Value;

/// 对齐 `StreamingState`。
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingState {
    pub is_streaming: bool,
    pub streaming_message: Option<Value>,
}

/// 对齐 `INITIAL_STREAMING_STATE`。
pub fn initial_streaming_state() -> StreamingState {
    StreamingState {
        is_streaming: false,
        streaming_message: None,
    }
}

/// 复刻 JS `String(value)` 隐式 ToString coercion,用于 `current.text + delta` / `current.thinking + delta`。
///
/// TS `current.text + event.delta`:当 `text` 字段缺失(undefined)时,`undefined + "x"`
/// 得 `"undefinedx"`(JS 把 undefined 强制为字符串 `"undefined"`)。Rust 此前用 `""`
/// 兜底,与上游不一致。此处忠实复刻:缺失→`"undefined"`,null→`"null"`,
/// bool/number→其字符串形式,object→`"[object Object]"`。
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Object(_)) => "[object Object]".to_string(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                Value::String(s) if s.is_empty() => String::new(),
                _ => js_string(Some(v)),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// 对齐 `StreamAction`。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamAction {
    Start,
    /// `{ type: "snapshot"; message }` —— 快照消息(已 normalize 或调用方处理)。
    Snapshot(Value),
    /// `{ type: "delta"; event }`。
    Delta(DeltaEvent),
    End,
}

/// 对齐 `ClientAssistantMessageEvent`(message_update 的 delta 投影)。
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaEvent {
    /// `text_start` / `text_delta` / `text_end` / `thinking_start` /
    /// `thinking_delta` / `thinking_end` / `toolcall_end`。
    pub r#type: String,
    pub content_index: Option<u64>,
    pub delta: Option<String>,
    pub content: Option<String>,
    /// `toolcall_end` 的 `event.toolCall`。
    pub tool_call: Option<ToolCallInfo>,
}

/// 对齐 `event.toolCall`(toolcall_end)。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    /// `event.toolCall.arguments`(JSON 字符串或对象)。
    pub arguments: Value,
}

impl DeltaEvent {
    /// 宽松解析:从 serde_json 事件对象读取(对齐 TS 的投影事件结构)。
    pub fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let r#type = obj.get("type").and_then(|t| t.as_str())?.to_string();
        let content_index = obj.get("contentIndex").and_then(|v| v.as_u64());
        let delta = obj
            .get("delta")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let content = obj
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let tool_call = obj.get("toolCall").and_then(|tc| {
            let tc = tc.as_object()?;
            Some(ToolCallInfo {
                id: tc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                name: tc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                arguments: tc.get("arguments").cloned().unwrap_or(Value::Null),
            })
        });
        Some(DeltaEvent {
            r#type,
            content_index,
            delta,
            content,
            tool_call,
        })
    }
}

/// 对齐 `normalizeToolCallBlock`。
fn normalize_tool_call_block(block: &Value) -> Option<Value> {
    let obj = block.as_object()?;
    if obj.get("type").and_then(|t| t.as_str()) != Some("toolCall") {
        return None;
    }
    let tool_call_id = obj
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("id").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let tool_name = obj
        .get("toolName")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("name").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let input = match obj.get("input") {
        Some(v) if v.is_object() => v.clone(),
        _ => match obj.get("arguments") {
            Some(v) if v.is_object() => v.clone(),
            _ => Value::Object(Default::default()),
        },
    };
    Some(serde_json::json!({
        "type": "toolCall",
        "toolCallId": tool_call_id,
        "toolName": tool_name,
        "input": input,
    }))
}

/// 对齐 `normalizeToolCalls`:仅 assistant 消息;toolCall 块的
/// toolCallId/toolName/input 字段归一化。
pub fn normalize_tool_calls(message: &Value) -> Value {
    if message.get("role").and_then(|r| r.as_str()) != Some("assistant") {
        return message.clone();
    }
    let Some(content) = message.get("content") else {
        return message.clone();
    };
    let Some(blocks) = content.as_array() else {
        return message.clone();
    };

    let normalized: Vec<Value> = blocks
        .iter()
        .map(|block| normalize_tool_call_block(block).unwrap_or_else(|| block.clone()))
        .collect();

    let mut out = message.clone();
    if let Value::Object(map) = &mut out {
        map.insert("content".to_string(), Value::Array(normalized));
    }
    out
}

/// 对齐 `updateContentBlock`。更新失败(update 返回 null / contentIndex 非法)时
/// 返回 None,调用方保持原状态。
///
/// JS 的 `content[contentIndex] = nextBlock` 在 index 等于当前长度时会自动扩长
/// (text_start 作用在空快照上正是这样),Rust 版等价处理。
fn update_content_block(
    state: &StreamingState,
    content_index: Option<u64>,
    update: impl FnOnce(Option<&Value>) -> Option<Value>,
) -> Option<StreamingState> {
    let message = state.streaming_message.as_ref()?;
    let index = content_index?;
    let index = index as usize;
    let content = message.get("content")?.as_array()?;
    let next_block = update(content.get(index))?;
    if !next_block.is_object() {
        return None;
    }
    let mut new_content = content.clone();
    // 对齐 JS `content[index] = block` 的自动扩长:index 超出当前长度时,
    // 间隙用稀疏洞填充(undefined → JSON.stringify 输出 null)。正常 delta 序列不会触发
    // (agent 顺序产出块),仅畸形/越界 index 下与上游一致。
    while new_content.len() < index {
        new_content.push(Value::Null);
    }
    if index == new_content.len() {
        new_content.push(next_block);
    } else {
        new_content[index] = next_block;
    }
    let mut new_message = message.clone();
    if let Value::Object(map) = &mut new_message {
        map.insert("content".to_string(), Value::Array(new_content));
    }
    Some(StreamingState {
        is_streaming: true,
        streaming_message: Some(new_message),
    })
}

fn apply_delta(state: &StreamingState, event: &DeltaEvent) -> StreamingState {
    let result = match event.r#type.as_str() {
        "text_start" => update_content_block(state, event.content_index, |current| {
            match current.and_then(|b| b.get("type").and_then(|t| t.as_str())) {
                Some("text") => current.cloned(),
                _ => Some(serde_json::json!({ "type": "text", "text": "" })),
            }
        }),
        "text_delta" => update_content_block(state, event.content_index, |current| {
            match current.and_then(|b| b.get("type").and_then(|t| t.as_str())) {
                Some("text") => {
                    let mut out = current.unwrap().clone();
                    if let Value::Object(map) = &mut out {
                        // 对齐 TS `current.text + event.delta`(缺失 text → "undefined"+delta)。
                        let text = js_string(map.get("text"));
                        let delta = event.delta.as_deref().unwrap_or("");
                        map.insert("text".to_string(), Value::String(format!("{text}{delta}")));
                    }
                    Some(out)
                }
                _ => None,
            }
        }),
        "text_end" => update_content_block(state, event.content_index, |current| {
            let mut out = match current.and_then(|b| b.get("type").and_then(|t| t.as_str())) {
                Some("text") => current.unwrap().clone(),
                _ => serde_json::json!({}),
            };
            if let Value::Object(map) = &mut out {
                map.insert("type".to_string(), Value::String("text".to_string()));
                map.insert(
                    "text".to_string(),
                    Value::String(event.content.clone().unwrap_or_default()),
                );
            }
            Some(out)
        }),
        "thinking_start" => {
            update_content_block(state, event.content_index, |current| {
                match current.and_then(|b| b.get("type").and_then(|t| t.as_str())) {
                    Some("thinking") => current.cloned(),
                    _ => Some(serde_json::json!({ "type": "thinking", "thinking": "" })),
                }
            })
        }
        "thinking_delta" => {
            update_content_block(state, event.content_index, |current| {
                match current.and_then(|b| b.get("type").and_then(|t| t.as_str())) {
                    Some("thinking") => {
                        let mut out = current.unwrap().clone();
                        if let Value::Object(map) = &mut out {
                            // 对齐 TS `current.thinking + event.delta`(缺失 → "undefined"+delta)。
                            let text = js_string(map.get("thinking"));
                            let delta = event.delta.as_deref().unwrap_or("");
                            map.insert(
                                "thinking".to_string(),
                                Value::String(format!("{text}{delta}")),
                            );
                        }
                        Some(out)
                    }
                    _ => None,
                }
            })
        }
        "thinking_end" => update_content_block(state, event.content_index, |current| {
            let mut out = match current.and_then(|b| b.get("type").and_then(|t| t.as_str())) {
                Some("thinking") => current.unwrap().clone(),
                _ => serde_json::json!({}),
            };
            if let Value::Object(map) = &mut out {
                map.insert("type".to_string(), Value::String("thinking".to_string()));
                map.insert(
                    "thinking".to_string(),
                    Value::String(event.content.clone().unwrap_or_default()),
                );
            }
            Some(out)
        }),
        "toolcall_end" => {
            let Some(tool_call) = event.tool_call.as_ref() else {
                return state.clone();
            };
            let arguments = tool_call.arguments.clone();
            // 对齐 TS `input: event.toolCall.arguments`:原样赋值。字符串参数保持字符串,
            // 不解析成对象(Rust 此前会解析,导致 input 形状与上游不一致)。
            update_content_block(state, event.content_index, |_| {
                Some(serde_json::json!({
                    "type": "toolCall",
                    "toolCallId": tool_call.id,
                    "toolName": tool_call.name,
                    "input": arguments,
                }))
            })
        }
        _ => None,
    };
    result.unwrap_or_else(|| state.clone())
}

/// 对齐 `streamReducer`。
pub fn stream_reducer(state: &StreamingState, action: &StreamAction) -> StreamingState {
    match action {
        StreamAction::Start => StreamingState {
            is_streaming: true,
            streaming_message: None,
        },
        StreamAction::Snapshot(message) => {
            let normalized = normalize_tool_calls(message);
            if normalized.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                StreamingState {
                    is_streaming: true,
                    streaming_message: Some(normalized),
                }
            } else {
                state.clone()
            }
        }
        StreamAction::Delta(event) => apply_delta(state, event),
        StreamAction::End => initial_streaming_state(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(content: Value) -> Value {
        json!({ "role": "assistant", "content": content })
    }

    #[test]
    fn initial_and_start_end() {
        assert_eq!(
            stream_reducer(&initial_streaming_state(), &StreamAction::End),
            initial_streaming_state()
        );
        let started = stream_reducer(&initial_streaming_state(), &StreamAction::Start);
        assert!(started.is_streaming);
        assert_eq!(started.streaming_message, None);
        // end 回到初始
        assert_eq!(
            stream_reducer(&started, &StreamAction::End),
            initial_streaming_state()
        );
    }

    #[test]
    fn snapshot_assistant_only() {
        let state = initial_streaming_state();
        // 非 assistant 快照 → 原状态
        let user = json!({ "role": "user", "content": "hi" });
        assert_eq!(stream_reducer(&state, &StreamAction::Snapshot(user)), state);
        // assistant 快照
        let assistant = msg(json!([{ "type": "text", "text": "x" }]));
        let next = stream_reducer(&state, &StreamAction::Snapshot(assistant.clone()));
        assert!(next.is_streaming);
        assert_eq!(next.streaming_message, Some(assistant));
    }

    #[test]
    fn snapshot_normalizes_tool_calls() {
        let assistant = json!({
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": "c1", "name": "bash", "arguments": { "cmd": "ls" } }]
        });
        let next = stream_reducer(
            &initial_streaming_state(),
            &StreamAction::Snapshot(assistant),
        );
        let content = next.streaming_message.unwrap();
        let block = &content["content"][0];
        assert_eq!(block["toolCallId"], "c1");
        assert_eq!(block["toolName"], "bash");
        assert_eq!(block["input"]["cmd"], "ls");
        // 旧字段被替换
        assert!(block.get("id").is_none());
    }

    #[test]
    fn text_streaming_sequence() {
        // 真实流:先 snapshot 再 delta(TS 对 null streamingMessage 的 delta 是 no-op)
        let snapshot = stream_reducer(
            &initial_streaming_state(),
            &StreamAction::Snapshot(msg(json!([]))),
        );
        let delta = |t: &str, idx: u64, d: Option<&str>| {
            StreamAction::Delta(DeltaEvent {
                r#type: t.to_string(),
                content_index: Some(idx),
                delta: d.map(|s| s.to_string()),
                content: None,
                tool_call: None,
            })
        };

        let s1 = stream_reducer(&snapshot, &delta("text_start", 0, None));
        assert_eq!(
            s1.streaming_message.as_ref().unwrap()["content"][0],
            json!({"type":"text","text":""})
        );
        let s2 = stream_reducer(&s1, &delta("text_delta", 0, Some("Hel")));
        let s3 = stream_reducer(&s2, &delta("text_delta", 0, Some("lo")));
        assert_eq!(
            s3.streaming_message.as_ref().unwrap()["content"][0]["text"],
            "Hello"
        );
        let s4 = stream_reducer(
            &s3,
            &StreamAction::Delta(DeltaEvent {
                r#type: "text_end".to_string(),
                content_index: Some(0),
                delta: None,
                content: Some("Final text".to_string()),
                tool_call: None,
            }),
        );
        assert_eq!(
            s4.streaming_message.as_ref().unwrap()["content"][0]["text"],
            "Final text"
        );
    }

    #[test]
    fn text_delta_on_non_text_is_noop() {
        let state = StreamingState {
            is_streaming: true,
            streaming_message: Some(msg(json!([{ "type": "thinking", "thinking": "t" }]))),
        };
        let delta = StreamAction::Delta(DeltaEvent {
            r#type: "text_delta".to_string(),
            content_index: Some(0),
            delta: Some("x".to_string()),
            content: None,
            tool_call: None,
        });
        assert_eq!(stream_reducer(&state, &delta), state);
    }

    #[test]
    fn text_delta_on_block_missing_text_uses_undefined() {
        // 对齐 TS `current.text + event.delta`:text 字段缺失(undefined)→ "undefined"+delta。
        // 畸形块(agent 不会产出),仅校验与上游 JS coercion 一致。
        let state = StreamingState {
            is_streaming: true,
            streaming_message: Some(msg(json!([{ "type": "text" }]))),
        };
        let delta = StreamAction::Delta(DeltaEvent {
            r#type: "text_delta".to_string(),
            content_index: Some(0),
            delta: Some("abc".to_string()),
            content: None,
            tool_call: None,
        });
        let next = stream_reducer(&state, &delta);
        assert_eq!(
            next.streaming_message.as_ref().unwrap()["content"][0]["text"],
            json!("undefinedabc")
        );
    }

    #[test]
    fn thinking_streaming() {
        let state = StreamingState {
            is_streaming: true,
            streaming_message: Some(msg(json!([{ "type": "text", "text": "keep" }]))),
        };
        let mk = |t: &str, delta: Option<&str>, content: Option<&str>| {
            StreamAction::Delta(DeltaEvent {
                r#type: t.to_string(),
                content_index: Some(0),
                delta: delta.map(|s| s.to_string()),
                content: content.map(|s| s.to_string()),
                tool_call: None,
            })
        };
        let s1 = stream_reducer(&state, &mk("thinking_start", None, None));
        assert_eq!(
            s1.streaming_message.as_ref().unwrap()["content"][0],
            json!({"type":"thinking","thinking":""})
        );
        let s2 = stream_reducer(&s1, &mk("thinking_delta", Some("a"), None));
        assert_eq!(
            s2.streaming_message.as_ref().unwrap()["content"][0]["thinking"],
            "a"
        );
        let s3 = stream_reducer(&s2, &mk("thinking_end", None, Some("final")));
        assert_eq!(
            s3.streaming_message.as_ref().unwrap()["content"][0]["thinking"],
            "final"
        );
    }

    #[test]
    fn toolcall_end_replaces_block() {
        let state = StreamingState {
            is_streaming: true,
            streaming_message: Some(msg(json!([{ "type": "text", "text": "old" }]))),
        };
        let action = StreamAction::Delta(DeltaEvent {
            r#type: "toolcall_end".to_string(),
            content_index: Some(0),
            delta: None,
            content: None,
            tool_call: Some(ToolCallInfo {
                id: "c9".to_string(),
                name: "bash".to_string(),
                arguments: json!("{\"cmd\":\"ls\"}"),
            }),
        });
        let next = stream_reducer(&state, &action);
        let block = next.streaming_message.as_ref().unwrap()["content"][0].clone();
        assert_eq!(block["type"], "toolCall");
        assert_eq!(block["toolCallId"], "c9");
        assert_eq!(block["toolName"], "bash");
        // 对齐 TS `input: event.toolCall.arguments`:原样赋值,字符串参数保持字符串。
        assert_eq!(block["input"], json!("{\"cmd\":\"ls\"}"));
    }

    #[test]
    fn invalid_content_index_is_noop() {
        let state = StreamingState {
            is_streaming: true,
            streaming_message: Some(msg(json!([{ "type": "text", "text": "x" }]))),
        };
        // 越界 index → update 拿到 None → 原状态
        let action = StreamAction::Delta(DeltaEvent {
            r#type: "text_delta".to_string(),
            content_index: Some(5),
            delta: Some("y".to_string()),
            content: None,
            tool_call: None,
        });
        assert_eq!(stream_reducer(&state, &action), state);
    }

    #[test]
    fn no_streaming_message_is_noop() {
        let state = initial_streaming_state();
        let action = StreamAction::Delta(DeltaEvent {
            r#type: "text_delta".to_string(),
            content_index: Some(0),
            delta: Some("y".to_string()),
            content: None,
            tool_call: None,
        });
        assert_eq!(stream_reducer(&state, &action), state);
    }

    #[test]
    fn unknown_event_type_is_noop() {
        let state = StreamingState {
            is_streaming: true,
            streaming_message: Some(msg(json!([{ "type": "text", "text": "x" }]))),
        };
        let action = StreamAction::Delta(DeltaEvent {
            r#type: "mystery".to_string(),
            content_index: Some(0),
            delta: None,
            content: None,
            tool_call: None,
        });
        assert_eq!(stream_reducer(&state, &action), state);
    }

    #[test]
    fn delta_from_value_parsing() {
        let v = json!({
            "type": "text_delta",
            "contentIndex": 2,
            "delta": "ab",
        });
        let event = DeltaEvent::from_value(&v).unwrap();
        assert_eq!(event.r#type, "text_delta");
        assert_eq!(event.content_index, Some(2));
        assert_eq!(event.delta.as_deref(), Some("ab"));

        let v = json!({
            "type": "toolcall_end",
            "contentIndex": 0,
            "toolCall": { "id": "c", "name": "n", "arguments": {} },
        });
        let event = DeltaEvent::from_value(&v).unwrap();
        assert_eq!(event.tool_call.as_ref().unwrap().id, "c");
        assert_eq!(event.tool_call.as_ref().unwrap().arguments, json!({}));

        assert!(DeltaEvent::from_value(&json!("x")).is_none());
        assert!(DeltaEvent::from_value(&json!({ "noType": true })).is_none());
    }

    #[test]
    fn normalize_tool_calls_roles() {
        // 非 assistant 原样返回
        let user = json!({ "role": "user", "content": "hi" });
        assert_eq!(normalize_tool_calls(&user), user);
        // assistant 无 content 数组 → 原样
        let bare = json!({ "role": "assistant" });
        assert_eq!(normalize_tool_calls(&bare), bare);
        // 非 toolCall 块原样
        let assistant = msg(json!([{ "type": "text", "text": "x" }]));
        assert_eq!(normalize_tool_calls(&assistant), assistant);
    }
}
