//! sessions — sessions_get / sessions_context 命令胶水
//! (自 moho-mate session_scanner 下沉;lib 复用部分不重写)。
//!
//! 分工:
//! - JSONL 读取/header 解析/EntryBase/info 组装 —— 本模块(纯胶水,原样下沉)
//! - context —— lib `session::build_session_context_from_json`(pi::sdk 引擎链)
//! - 树投影 —— parentId 挂接(本模块) + lib `project_tree::project_tree_for_response`
//!   (keep 压缩/深度封顶/branchPreview,上游 parity;moho 旧本地实现无 preview)
//! - id → 文件路径 —— lib `session::resolve_session_path` 缓存 + 本模块扫描兜底

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::ApiError;

/// 会话 header(首行,探针 §5.3;字段 camelCase 对齐 pi 写端)。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionHeader {
    pub id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    pub leaf_id: Option<String>,
    pub branched_from: Option<String>,
}

/// 读首行(有界 ≤64KB;探针 §5.3)。无内容或首行超长 → None。
fn read_first_line(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    // 含换行的上限;超长首行视为坏文件
    reader.read_line(&mut line).ok()?;
    if line.len() > 64 * 1024 {
        return None;
    }
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_header(line: &str) -> Option<SessionHeader> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    serde_json::from_value(v).ok()
}

/// 读全部 entry 行(跳过 header;坏行容忍 —— 与扫描器同语义)。
fn read_entries(path: &Path) -> Vec<Value> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .collect()
}

/// id → 文件路径:lib path 缓存快路径 + 扫描兜底(root/*/*.jsonl 首
/// header.id 比对)。单一会话根(上游 TS 同构)。
pub(crate) fn find_session_file(root: &str, id: &str) -> Option<PathBuf> {
    if let Some(cached) = crate::session::resolve_session_path(id) {
        let path = PathBuf::from(&cached);
        if path.exists() {
            return Some(path);
        }
    }
    find_session_file_in_root(root, id)
}

fn find_session_file_in_root(root: &str, id: &str) -> Option<PathBuf> {
    let root = Path::new(root);
    let Ok(days) = std::fs::read_dir(root) else {
        return None;
    };
    for day in days.filter_map(|e| e.ok()) {
        let day_path = day.path();
        if !day_path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for f in files.filter_map(|e| e.ok()) {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(line) = read_first_line(&path) {
                if let Some(h) = parse_header(&line) {
                    if h.id == id {
                        // 缓存由 lib 的 list_all_sessions 侧维护,此处不回填
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

/// GET /api/sessions/:id/context —— lib 引擎链 + 形状组装。
pub(crate) fn context_value(
    path: &Path,
    leaf_id: Option<&str>,
    defer_thinking: bool,
    defer_media: bool,
) -> Result<Value, ApiError> {
    let entries = read_entries(path);
    let ctx = crate::session::build_session_context_from_json(
        &entries,
        leaf_id,
        defer_thinking,
        defer_media,
    );
    Ok(json!({
        "context": {
            "messages": ctx.messages,
            "entryIds": ctx.entry_ids,
            "thinkingLevel": ctx.thinking_level.unwrap_or_else(|| "off".to_string()),
            "model": ctx.model,
        }
    }))
}

/// GET /api/sessions/:id —— header + entries + info + tree + context。
/// messageCount/firstMessage 以 context 为准(对齐 route.ts:74/149)。
pub(crate) fn get_value(path: &Path, defer_thinking: bool, defer_media: bool) -> Result<Value, ApiError> {
    let first_line = read_first_line(path)
        .ok_or_else(|| ApiError::internal(format!("cannot read session header: {}", path.display())))?;
    let header = parse_header(&first_line)
        .ok_or_else(|| ApiError::internal(format!("invalid session header: {}", path.display())))?;
    let entries = read_entries(path);
    let leaf_id = header
        .leaf_id
        .clone()
        .or_else(|| entries.iter().rev().find_map(|e| e.get("id").and_then(|i| i.as_str()).map(String::from)));

    let mut info = build_info(path, &header, &entries);
    let context_obj = context_value(path, leaf_id.as_deref(), defer_thinking, defer_media)?;
    let context = context_obj.get("context").cloned().unwrap_or_else(|| json!({}));
    info["messageCount"] = json!(context["messages"].as_array().map(|a| a.len()).unwrap_or(0));
    if let Some(first) = context["messages"]
        .as_array()
        .and_then(|arr| arr.iter().find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user")))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        if !first.is_empty() {
            info["firstMessage"] = json!(first);
        }
    }

    Ok(json!({
        "sessionId": header.id,
        "filePath": path.to_string_lossy(),
        "info": info,
        "leafId": leaf_id,
        "tree": build_tree(&entries),
        "context": context,
    }))
}

/// info 组装(原样下沉 moho build_session_info;route.ts 口径:
/// modified = 文件 mtime 优先、header.timestamp 回退)。
fn build_info(path: &Path, header: &SessionHeader, entries: &[Value]) -> Value {
    let name = entries
        .iter()
        .rev()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("session_info"))
        .and_then(|e| e.get("name"))
        .and_then(|n| n.as_str());
    let modified = file_mtime_iso(path).unwrap_or_else(|| header.timestamp.clone());
    let message_count = entries
        .iter()
        .filter(|e| {
            matches!(
                e.get("type").and_then(|t| t.as_str()).unwrap_or(""),
                "message" | "compaction" | "branch_summary" | "custom"
            )
        })
        .count();
    let first_message = entries
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("message"))
        .and_then(|e| e.get("message"))
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .and_then(|m| m.get("content"))
        .map(user_first_text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(no messages)".to_string());
    let parent_session_id = header
        .branched_from
        .as_deref()
        .map(Path::new)
        .map(|p| p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string())
        .filter(|s| !s.is_empty());

    let mut info = json!({
        "path": path.to_string_lossy(),
        "id": header.id,
        "cwd": header.cwd,
        "created": header.timestamp,
        "modified": modified,
        "messageCount": message_count,
        "firstMessage": first_message,
        "projectRoot": header.cwd,
    });
    if let Some(n) = name {
        info["name"] = json!(n);
    }
    if let Some(pid) = parent_session_id {
        info["parentSessionId"] = json!(pid);
    }
    info
}

fn file_mtime_iso(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = mtime.into();
    Some(dt.to_rfc3339())
}

/// user 消息首段文本(content 为 string 或 blocks)。
fn user_first_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// 树投影:parentId 挂接(本模块)→ lib project_tree_for_response
/// (keep 压缩 + compressedEntryIds + branchPreview + 深度封顶)。
fn build_tree(entries: &[Value]) -> Value {
    use crate::project_tree::{ProjectableTreeNode, project_tree_for_response};

    // 挂接:id → children(按文件顺序);父缺失或不在集合 → 根
    let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in entries {
        if let Some(id) = e.get("id").and_then(|i| i.as_str()) {
            ids.insert(id);
        }
    }
    let mut children_by_parent: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (idx, e) in entries.iter().enumerate() {
        let parent = e.get("parentId").and_then(|p| p.as_str());
        match parent {
            Some(pid) if ids.contains(pid) => {
                children_by_parent.entry(pid.to_string()).or_default().push(idx);
            }
            _ => roots.push(idx),
        }
    }

    // 递归物化为 ProjectableTreeNode(entry = 原始 json 反序列化,extra 吸收其余字段)
    fn materialize(
        idx: usize,
        entries: &[Value],
        children_by_parent: &std::collections::HashMap<String, Vec<usize>>,
    ) -> ProjectableTreeNode {
        let entry: crate::project_tree::ProjectableEntry =
            serde_json::from_value(entries[idx].clone()).unwrap_or_default();
        let children = children_by_parent
            .get(&entry.id)
            .map(|v| {
                v.iter()
                    .map(|&c| materialize(c, entries, children_by_parent))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ProjectableTreeNode { entry, children, ..Default::default() }
    }

    let nodes: Vec<ProjectableTreeNode> =
        roots.iter().map(|&r| materialize(r, entries, &children_by_parent)).collect();
    serde_json::to_value(project_tree_for_response(nodes)).unwrap_or(Value::Array(vec![]))
}

/// GET /api/sessions/:id —— 命令入口(路径经 routes 参数捕获为 args.id)。
pub(crate) async fn get_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<Value, ApiError> {
    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new(400, "id is required"));
    }
    let defer_thinking = dispatch.args.get("deferThinking").and_then(|v| v.as_bool()).unwrap_or(false);
    let defer_media = dispatch.args.get("deferMedia").and_then(|v| v.as_bool()).unwrap_or(false);
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;
    let v = super::commands::blocking(ctx, move || get_value(&path, defer_thinking, defer_media))
        .await??;
    Ok(v)
}

/// GET /api/sessions/:id/context —— 命令入口。
pub(crate) async fn context_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<Value, ApiError> {
    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new(400, "id is required"));
    }
    let leaf_id = dispatch.args.get("leafId").and_then(|v| v.as_str()).map(String::from);
    let defer_thinking = dispatch.args.get("deferThinking").and_then(|v| v.as_bool()).unwrap_or(false);
    let defer_media = dispatch.args.get("deferMedia").and_then(|v| v.as_bool()).unwrap_or(false);
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;
    super::commands::blocking(ctx, move || {
        context_value(&path, leaf_id.as_deref(), defer_thinking, defer_media)
    })
    .await?
}

/// 唯一会话根(上游 TS 同构:single source of truth)。
/// hooks.sessions_root()(宿主配置,如 AppConfig.chat.session_dir);
/// 未提供时回退引擎默认根。写盘/扫描/恢复全部用这一个根。
fn session_root(ctx: &super::commands::ExecCtx) -> String {
    ctx.hooks
        .sessions_root()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(super::commands::default_sessions_root_pub)
}

// ============================================================================
// 测试:合成会话 jsonl(tempdir + HostHooks.sessions_root 注入,确定性)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_session(root: &Path, file: &str, lines: &[String]) -> PathBuf {
        let day = root.join("2026-08-16");
        std::fs::create_dir_all(&day).unwrap();
        let p = day.join(file);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    fn header(id: &str) -> String {
        json!({
            "type": "session", "id": id, "cwd": "/tmp/probe",
            "timestamp": "2026-08-16T00:00:00.000Z", "leafId": "e3"
        })
        .to_string()
    }

    fn entry(id: &str, parent: Option<&str>, kind: &str) -> String {
        let mut m = Map::new();
        m.insert("type".into(), json!(kind));
        m.insert("id".into(), json!(id));
        // timestamp 是 pi EntryBase 必填(缺则 SessionEntry 反序列化失败,lib 容忍为坏行)
        m.insert("timestamp".into(), json!("2026-08-16T00:00:01.000Z"));
        if let Some(p) = parent {
            m.insert("parentId".into(), json!(p));
        }
        Value::Object(m).to_string()
    }

    fn user_msg(id: &str, parent: &str, text: &str) -> String {
        json!({
            "type": "message", "id": id, "parentId": parent,
            "timestamp": "2026-08-16T00:00:01.000Z",
            "message": { "role": "user", "content": text }
        })
        .to_string()
    }

    #[test]
    fn find_session_by_header_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let p = write_session(
            &root,
            "2026-08-16T00-00-00.000Z_deadbeef.jsonl",
            &[header("uuid-1234"), entry("e1", None, "message")],
        );
        assert_eq!(find_session_file(root.to_str().unwrap(), "uuid-1234").as_deref(), Some(p.as_path()));
        assert!(find_session_file(root.to_str().unwrap(), "nope").is_none());
    }

    #[test]
    fn get_value_shape_and_context_message_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        // e2(assistant)经 pi 类型化构造序列化 —— 保证 schema 合法
        // (手写 JSON 对 flatten+tag 组合的键形太脆;typed 构造锁死契约)
        let e2_entry = pi::session::SessionEntry::Message(pi::session::MessageEntry {
            base: pi::session::EntryBase {
                id: Some("e2".into()),
                parent_id: Some("e1".into()),
                timestamp: "2026-08-16T00:00:02.000Z".into(),
            },
            message: pi::session::SessionMessage::Assistant {
                message: pi::model::AssistantMessage {
                    content: vec![pi::model::ContentBlock::Text(pi::model::TextContent {
                        text: "hi".into(),
                        text_signature: None,
                    })],
                    api: "probe".into(),
                    provider: "probe".into(),
                    model: "probe-1".into(),
                    usage: Default::default(),
                    stop_reason: Default::default(),
                    stop_details: None,
                    error_message: None,
                    timestamp: 1786849011944,
                },
            },
        });
        let lines = vec![
            header("uuid-get"),
            user_msg("e1", "uuid-get", "hello spike"),
            serde_json::to_string(&e2_entry).unwrap(),
            user_msg("e3", "e2", "next turn"),
        ];
        let p = write_session(&root, "a.jsonl", &lines);

        let v = get_value(&p, false, false).unwrap();
        assert_eq!(v["sessionId"], json!("uuid-get"));
        assert_eq!(v["info"]["id"], json!("uuid-get"));
        assert_eq!(v["info"]["firstMessage"], json!("hello spike"));
        // messageCount 以 context 为准(route.ts:74)
        assert!(v["info"]["messageCount"].as_u64().unwrap_or(0) > 0);
        assert!(v["tree"].is_array());
        assert!(v["context"]["messages"].is_array());
        assert_eq!(v["leafId"], json!("e3"));
    }

    #[test]
    fn tree_compresses_single_chains() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        // e1 →(单链)→ e2 →(单链)→ e3:中段被吞为 compressedEntryIds
        let lines = vec![
            header("uuid-tree"),
            user_msg("e1", "uuid-tree", "a"),
            user_msg("e2", "e1", "b"),
            user_msg("e3", "e2", "c"),
        ];
        let p = write_session(&root, "t.jsonl", &lines);
        let v = get_value(&p, false, false).unwrap();
        let tree = v["tree"].as_array().unwrap();
        // header 不是 entry,根 = e1;e1 的孩子经单链压缩挂在 e3 节点上
        assert_eq!(tree.len(), 1);
        let children = tree[0]["children"].as_array().unwrap();
        assert!(!children.is_empty());
        let compressed = children[0]["compressedEntryIds"].as_array().unwrap();
        assert!(compressed.contains(&json!("e2")), "mid-chain swallowed: {compressed:?}");
    }

    #[test]
    fn context_value_leaf_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let lines = vec![
            header("uuid-ctx"),
            user_msg("e1", "uuid-ctx", "one"),
            user_msg("e2", "e1", "two"),
        ];
        let p = write_session(&root, "c.jsonl", &lines);
        let v = context_value(&p, Some("e1"), false, false).unwrap();
        let msgs = v["context"]["messages"].as_array().unwrap();
        // leafId=e1 截断:只含 e1 之前的消息
        assert_eq!(msgs.len(), 1);
    }
}

// ============================================================================
// PATCH /api/sessions/:id — rename(追加 session_info entry,纯文件手术)
// ============================================================================

pub(crate) async fn rename_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<Value, ApiError> {
    use std::io::Write;
    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new(400, "id is required"));
    }
    let name = dispatch
        .args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;

    super::commands::blocking(ctx, move || -> Result<(), ApiError> {
        let first_line = read_first_line(&path)
            .ok_or_else(|| ApiError::internal(format!("cannot read session header: {}", path.display())))?;
        let header = parse_header(&first_line)
            .ok_or_else(|| ApiError::internal(format!("invalid session header: {}", path.display())))?;
        let entry_id = uuid::Uuid::new_v4().to_string();
        let parent_id = header.leaf_id.clone().unwrap_or_else(|| header.id.clone());
        let timestamp = chrono::Utc::now().to_rfc3339();
        let entry = json!({
            "type": "session_info",
            "id": entry_id,
            "parentId": parent_id,
            "timestamp": timestamp,
            "name": name,
        });
        let line = serde_json::to_string(&entry).map_err(|e| ApiError::internal(e.to_string()))?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path)
            .map_err(|e| ApiError::internal(format!("open session append: {e}")))?;
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| ApiError::internal(format!("append session_info: {e}")))?;
        Ok(())
    })
    .await??;
    Ok(json!({ "success": true }))
}

// ============================================================================
// DELETE /api/sessions/:id — 删文件 + 重挂子会话 + 活跃会话重建
// ============================================================================

pub(crate) async fn delete_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<Value, ApiError> {
    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new(400, "id is required"));
    }
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;

    // 文件手术(blocking):删文件 + 重挂直接子会话(branchedFrom 改指祖父)
    let was_active = ctx.sessions.get(&id).is_some();
    super::commands::blocking(ctx, move || -> Result<(), ApiError> {
        // 1. 本文件 header 的父(branchedFrom)作为子会话的新祖父
        let grandparent = read_first_line(&path)
            .and_then(|l| parse_header(&l))
            .and_then(|h| h.branched_from);
        // 2. 重挂直接子会话(首行 branchedFrom == 本文件 → 改指祖父)
        if let Some(dir) = path.parent() {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if p == path {
                        continue;
                    }
                    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(line) = read_first_line(&p) {
                        if let Some(h) = parse_header(&line) {
                            if h.branched_from.as_deref() == Some(path.to_string_lossy().as_ref()) {
                                // 重写首行 branchedFrom(若祖父存在)
                                let mut v: serde_json::Value =
                                    serde_json::from_str(&line).unwrap_or_default();
                                if let Some(obj) = v.as_object_mut() {
                                    match &grandparent {
                                        Some(gp) => {
                                            obj.insert(
                                                "branchedFrom".to_string(),
                                                serde_json::json!(gp),
                                            );
                                        }
                                        None => {
                                            obj.remove("branchedFrom");
                                        }
                                    }
                                    let new_line =
                                        serde_json::to_string(&v).map_err(|e| ApiError::internal(e.to_string()))?;
                                    let content = std::fs::read_to_string(&p)
                                        .map_err(|e| ApiError::internal(e.to_string()))?;
                                    let rest = content
                                        .lines()
                                        .skip(1)
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    std::fs::write(
                                        &p,
                                        format!("{new_line}\n{}\n", rest.trim_end_matches('\n')),
                                    )
                                    .map_err(|e| ApiError::internal(format!("rewrite child: {e}")))?;
                                }
                            }
                        }
                    }
                }
            }
        }
        // 3. 删文件
        std::fs::remove_file(&path).map_err(|e| ApiError::internal(format!("delete: {e}")))?;
        Ok(())
    })
    .await??;

    // 4. 删除的是活跃会话 → 重建全新会话(防引擎 autosave 复活已删文件)
    if was_active {
        let h = ctx.sessions.get(&id).expect("active session present");
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        h.tx
            .send(super::session_runtime::SessionCmd::Rebuild {
                path: None,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ApiError::internal("session task gone during delete"))?;
        let _ = reply_rx
            .await
            .map_err(|_| ApiError::internal("delete rebuild reply dropped"))?;
    }
    Ok(json!({ "success": true }))
}

// ============================================================================
// POST /api/sessions/:id/auto-name — 退化:首条 user 消息截断(~60 字符)
// (上游调模型生成标题;无 API key 时不可行,与 moho 同款退化。usage:null)
// ============================================================================

pub(crate) async fn auto_name_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<Value, ApiError> {
    use std::io::Write;

    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new(400, "id is required"));
    }
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;

    // 1) 读取会话消息 + header 模型(标题生成用会话自身模型优先)
    let (messages, header_model): (Vec<Value>, Option<(String, String)>) =
        super::commands::blocking(ctx, move || {
            let entries = read_entries(&path);
            let msgs: Vec<Value> = entries
                .iter()
                .filter_map(|v| {
                    if v.get("type").and_then(|t| t.as_str()) != Some("message") {
                        return None;
                    }
                    v.get("message").cloned()
                })
                .collect();
            let header_pair = read_first_line(&path)
                .and_then(|l| parse_header(&l))
                .and_then(|h| match (h.provider, h.model_id) {
                    (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => Some((p, m)),
                    _ => None,
                });
            Ok::<_, ApiError>((msgs, header_pair))
        })
        .await??;

    // 2) LLM 生成标题(上游 session-title.ts:registry 解析模型 → 无状态
    //    provider 调用;会话自身模型优先,settings 默认兜底)
    let runner = LlmTitleRunner::for_session(ctx, header_model).await?;
    let generated = crate::session::title::generate_session_title(&runner, &messages)
        .await
        .map_err(|e| ApiError::new(500, e))?;
    let title = generated.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::internal("generated title is empty"));
    }

    // 3) 持久化:session_info 追加(与 rename 同机制)+ 活会话引擎侧同步
    let title_for_persist = title.clone();
    let path_for_persist = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;
    super::commands::blocking(ctx, move || -> Result<(), ApiError> {
        let first_line = read_first_line(&path_for_persist)
            .ok_or_else(|| ApiError::internal("cannot read session header"))?;
        let header = parse_header(&first_line)
            .ok_or_else(|| ApiError::internal("invalid session header"))?;
        let entry = json!({
            "type": "session_info",
            "id": uuid::Uuid::new_v4().to_string(),
            "parentId": header.leaf_id.clone().unwrap_or_else(|| header.id.clone()),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "name": title_for_persist,
        });
        let line = serde_json::to_string(&entry)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path_for_persist)
            .map_err(|e| ApiError::internal(format!("open session append: {e}")))?;
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| ApiError::internal(format!("append session_info: {e}")))?;
        Ok(())
    })
    .await??;
    // 活会话同步(set_session_name 会话内生效;磁盘恢复路径也带名)
    if let Some(h) = ctx.sessions.get(&id) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = h
            .tx
            .send(super::session_runtime::SessionCmd::SetSessionName {
                name: title.clone(),
                reply: tx,
            })
            .await;
        let _ = rx.await;
    }

    Ok(json!({
        "title": title,
        "usage": generated.usage.map(|u| serde_json::to_value(u).unwrap_or(Value::Null)).unwrap_or(Value::Null),
    }))
}

/// 无状态标题生成 runner(上游 runner 语义:独立 LLM 调用,不进会话历史)。
/// 模型解析:会话 header 的 provider/model 优先 → settings 默认 → 首个可用。
struct LlmTitleRunner {
    provider: std::sync::Arc<dyn pi::provider::Provider>,
    api_key: Option<String>,
}

impl LlmTitleRunner {
    async fn for_session(
        ctx: &super::commands::ExecCtx,
        header_model: Option<(String, String)>,
    ) -> Result<Self, ApiError> {
        super::commands::blocking(ctx, move || {
            let global_dir = pi::sdk::Config::global_dir();
            let auth = pi::auth::AuthStorage::load(global_dir.join("auth.json"))
                .map_err(|e| ApiError::internal(format!("auth load: {e}")))?;
            let models_path = global_dir.join("models.json");
            let registry = pi::models::ModelRegistry::load(&auth, Some(models_path));
            let header_pair = header_model;
            let entry = header_pair
                .and_then(|(p, m)| registry.find(&p, &m))
                .or_else(|| {
                    let cfg = pi::config::Config::load().unwrap_or_default();
                    match (&cfg.default_provider, &cfg.default_model) {
                        (Some(p), Some(m)) => registry.find(p, m),
                        _ => None,
                    }
                })
                .or_else(|| registry.get_available().into_iter().next())
                .ok_or_else(|| ApiError::internal("no model available for title generation"))?;
            let api_key = entry.api_key.clone().filter(|k| !k.trim().is_empty());
            let provider = pi::providers::create_provider(&entry, None)
                .map_err(|e| ApiError::internal(format!("create provider: {e}")))?;
            Ok(Self { provider, api_key })
        })
        .await?
    }
}

impl crate::session::title::SessionTitleRunner for LlmTitleRunner {
    fn run_title(
        &self,
        messages: &[Value],
        _continues_from_trailing_user: bool,
        title_prompt: &str,
        _history_length: usize,
        _timeout_ms: u64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::session::title::GeneratedSessionTitle, String>> + Send + '_>,
    > {
        let provider = self.provider.clone();
        let api_key = self.api_key.clone();
        let prompt = title_prompt.to_string();
        // 消息平铺为对话文本(compaction serialize_conversation 同思路);
        // 标题生成无需真实消息结构,单条 user 提示即可
        let mut conversation = String::new();
        for m in messages {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = match m.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|b| {
                        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                            Some(
                                b.get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            )
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if content.trim().is_empty() {
                continue;
            }
            conversation.push_str(&format!("{role}: {content}\n"));
        }
        Box::pin(async move {
            use futures::StreamExt;
            use pi::model::{ContentBlock, Message, StreamEvent, TextContent, UserContent, UserMessage};
            use pi::provider::{Context, StreamOptions};

            let context = Context::owned(
                Some(prompt),
                vec![Message::User(UserMessage {
                    content: UserContent::Blocks(vec![ContentBlock::Text(TextContent::new(conversation))]),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                })],
                Vec::new(),
            );
            let options = StreamOptions {
                api_key,
                max_tokens: Some(256),
                ..Default::default()
            };
            let mut stream = provider
                .stream(&context, &options)
                .await
                .map_err(|e| format!("title stream: {e}"))?;
            let mut text = String::new();
            let mut usage = None;
            while let Some(event) = stream.next().await {
                match event.map_err(|e| format!("title stream event: {e}"))? {
                    StreamEvent::Done { message, .. } => {
                        for block in &message.content {
                            if let ContentBlock::Text(t) = block {
                                text.push_str(&t.text);
                            }
                        }
                        usage = Some(crate::session::title::Usage {
                            input: message.usage.input,
                            output: message.usage.output,
                            cache_read: message.usage.cache_read,
                            cache_write: message.usage.cache_write,
                            total: message.usage.total_tokens,
                        });
                    }
                    StreamEvent::Error { error, .. } => {
                        return Err(error.error_message.unwrap_or_else(|| "title error".into()));
                    }
                    _ => {}
                }
            }
            let title = crate::session::title::parse_generated_session_title(&text)?;
            Ok(crate::session::title::GeneratedSessionTitle { title, usage })
        })
    }
}

// ============================================================================
// GET /api/sessions/:id/export — 上游导出器完整移植(见 api/export.rs)
// ============================================================================

pub(crate) async fn export_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new(400, "id is required"));
    }
    // query_to_args 把 1 解析为数字;字符串/数字两种形态都认
    let inline = dispatch
        .args
        .get("inline")
        .is_some_and(|v| v == "1" || v.as_i64() == Some(1));
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found("Session not found".to_string()))?;

    let file_name = super::export::export_file_name(&path);
    // 文件 IO 走 blocking(本仓纪律);组装 + 补丁见 export.rs
    let html = super::commands::blocking(ctx, move || -> Result<String, ApiError> {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| ApiError::internal(format!("read session: {e}")))?;
        super::export::export_session_html(&content)
            .map_err(|e| ApiError::new(500, e))
    })
    .await??;

    super::export::html_response(html, &file_name, inline)
}

// ============================================================================
// GET /api/sessions/:id/entries/:entryId/thinking — 惰性加载 thinking 块
// ============================================================================

pub(crate) async fn thinking_command(
    ctx: &super::commands::ExecCtx,
    dispatch: super::routes::Dispatch,
) -> Result<Value, ApiError> {
    let id = dispatch
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let entry_id = dispatch
        .args
        .get("entryId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    // blockIndex 缺失/非安全整数/负数 → 400(对齐 thinking/route.ts:10-12)
    let block_index = match dispatch.args.get("blockIndex").and_then(|v| v.as_u64()) {
        Some(b) => b,
        None => return Err(ApiError::new(400, "Valid blockIndex is required")),
    };
    if id.is_empty() || entry_id.is_empty() {
        return Err(ApiError::new(400, "id and entryId are required"));
    }
    let root = session_root(ctx);
    let path = find_session_file(&root, &id)
        .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;

    super::commands::blocking(ctx, move || -> Result<Value, ApiError> {
        let entries = read_entries(&path);
        let entry = entries
            .into_iter()
            .find(|v| v.get("id").and_then(|i| i.as_str()) == Some(entry_id.as_str()))
            .ok_or_else(|| ApiError::not_found(format!("entry not found: {entry_id}")))?;
        // 参考 thinking/route.ts:18-33:entry 必须是 assistant message
        let msg = entry
            .get("message")
            .ok_or_else(|| ApiError::not_found("entry has no message".to_string()))?;
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            return Err(ApiError::not_found(format!("entry {entry_id} is not an assistant message")));
        }
        let content = msg
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ApiError::not_found("entry has no content array".to_string()))?;
        // blockIndex 是 content 数组原始下标(客户端传 originalIndex);块非 thinking → 404
        let block = content
            .get(block_index as usize)
            .ok_or_else(|| ApiError::not_found(format!("block {block_index} out of range in entry {entry_id}")))?;
        if block.get("type").and_then(|t| t.as_str()) != Some("thinking") {
            return Err(ApiError::not_found(format!("block {block_index} is not a thinking block")));
        }
        let thinking = block
            .get("thinking")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        Ok(json!({ "thinking": thinking }))
    })
    .await?
}
