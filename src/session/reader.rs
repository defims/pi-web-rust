//! 对齐 `lib/session-reader.ts`。
//!
//! JSONL 会话文件读取(有界,64KB header) + 路径缓存层。
//! SessionManager.listAll 经 `pi::sdk::SessionIndex` 接线;
//! buildSessionContext 经 `pi::sdk::build_session_context` 接线(见 entries.rs)。

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use super::path::session_path_key;

const MAX_HEADER_BYTES: usize = 64 * 1024;

/// 对齐 `SessionHeader`(pi jsonl 第一行)。camelCase serde 对齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    pub id: String,
    pub timestamp: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "model_id")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "modelId")]
    pub model_id_alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "leafId")]
    pub leaf_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "branchedFrom")]
    pub branched_from: Option<String>,
}

// ── 路径缓存(对齐 globalThis __piSessionPathCache 等) ───────────────────

struct PathCache {
    id_to_path: HashMap<String, String>,
    path_to_id: HashMap<String, String>,
}

static PATH_CACHE: LazyLock<Mutex<PathCache>> =
    LazyLock::new(|| Mutex::new(PathCache { id_to_path: HashMap::new(), path_to_id: HashMap::new() }));

/// 对齐 `cacheSessionPath`。
pub fn cache_session_path(session_id: &str, file_path: &str) {
    let normalized = normalize_file_path(file_path);
    let key = session_path_key(&normalized);
    let mut cache = PATH_CACHE.lock().unwrap();
    // 清理旧映射
    if let Some(old_path) = cache.id_to_path.get(session_id) {
        let old_key = session_path_key(old_path);
        if old_key != key && cache.path_to_id.get(&old_key).map(|s| s.as_str()) == Some(session_id) {
            cache.path_to_id.remove(&old_key);
        }
    }
    cache.id_to_path.insert(session_id.to_string(), normalized);
    cache.path_to_id.insert(key, session_id.to_string());
}

/// 对齐 `invalidateSessionPathCache`。
pub fn invalidate_path_cache(session_id: &str) {
    let mut cache = PATH_CACHE.lock().unwrap();
    if let Some(path) = cache.id_to_path.remove(session_id) {
        let key = session_path_key(&path);
        if cache.path_to_id.get(&key).map(|s| s.as_str()) == Some(session_id) {
            cache.path_to_id.remove(&key);
        }
    }
}

/// 对齐 `resolveSessionPath`。从缓存取路径(不触发全扫描,调用方负责预热)。
pub fn resolve_session_path(session_id: &str) -> Option<String> {
    PATH_CACHE
        .lock()
        .ok()
        .and_then(|c| c.id_to_path.get(session_id).cloned())
}

fn normalize_file_path(p: &str) -> String {
    PathBuf::from(p).to_string_lossy().to_string()
}

// ── JSONL 读取 ─────────────────────────────────────────────────────────

/// 对齐 `readSessionHeader`。有界读(≤64KB),取第一行 JSON,解析为 SessionHeader。
pub fn read_session_header(file_path: &str) -> Option<SessionHeader> {
    read_first_line(file_path, MAX_HEADER_BYTES)
        .and_then(|line| {
            let line = line.trim_end();
            if line.is_empty() {
                return None;
            }
            serde_json::from_str::<SessionHeader>(line).ok()
        })
        .filter(|h| h.r#type == "session")
}

/// 对齐 `getSessionEntries`:读 jsonl 文件的全部行(header 之后)。
/// 上下文构建用 `session::build_session_context_from_json`(经 pi::sdk::build_session_context)。
pub fn list_session_entries(file_path: &str) -> Vec<serde_json::Value> {
    let Ok(content) = std::fs::read_to_string(file_path) else {
        return vec![];
    };
    content
        .lines()
        .skip(1) // 跳过 header 行
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// 列出会话目录下的所有 .jsonl 文件。
/// SessionInfo 派生(含 firstMessage/messageCount/parentSessionId)用 `list_all_sessions`(经 pi::sdk::SessionIndex)。
pub fn list_session_files(sessions_dir: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

/// 对齐 pi-web `SessionInfo`(Web UI 消费形状,camelCase serde)。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSessionInfo {
    pub path: String,
    pub id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created: String,
    pub modified: String,
    pub message_count: u64,
    pub first_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

/// 对齐 TS `loadAllSessions()`。调用 pi_agent_rust 的 `SessionIndex::list_sessions`
/// 获取 SessionMeta 列表,派生 WebSessionInfo(projectRoot / worktreeBranch /
/// parentSessionId),更新 path 缓存。
///
/// `sessions_root` 对齐 TS `getAgentDir() + "sessions"`;
/// `resolve_project` 注入 `git::worktree::resolve_project`(按 cwd 派生 projectRoot,
/// 已有 60s 缓存)。
pub fn list_all_sessions(
    sessions_root: &str,
    resolve_project: impl Fn(&str) -> crate::git::worktree::ProjectInfo,
) -> Vec<WebSessionInfo> {
    let index = pi::sdk::SessionIndex::for_sessions_root(std::path::Path::new(sessions_root));
    let metas = match index.list_sessions(None) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    // path → id 映射(对齐 TS pathToId)
    let mut path_to_id: HashMap<String, String> = HashMap::new();
    for m in &metas {
        path_to_id.insert(super::path::session_path_key(&m.path), m.id.clone());
    }

    // 按 unique cwd 派生 projectRoot(对齐 TS projectByCwd)
    let mut project_by_cwd: HashMap<String, crate::git::worktree::ProjectInfo> = HashMap::new();

    metas
        .iter()
        .map(|m| {
            cache_session_path(&m.id, &m.path);

            let project = if m.cwd.is_empty() {
                None
            } else {
                Some(
                    project_by_cwd
                        .entry(m.cwd.clone())
                        .or_insert_with(|| resolve_project(&m.cwd))
                        .clone(),
                )
            };

            let parent_session_id = m
                .parent_session_path
                .as_ref()
                .and_then(|p| path_to_id.get(&super::path::session_path_key(p)).cloned());

            let worktree_branch = project
                .as_ref()
                .filter(|p| p.is_worktree)
                .and_then(|p| p.branch.clone());

            WebSessionInfo {
                path: m.path.clone(),
                id: m.id.clone(),
                cwd: m.cwd.clone(),
                name: m.name.clone(),
                created: m.timestamp.clone(),
                modified: format_millis_iso(m.modified_ms),
                message_count: m.message_count,
                first_message: m.first_message.clone(),
                parent_session_id,
                project_root: project
                    .as_ref()
                    .map(|p| p.project_root.clone())
                    .or_else(|| if m.cwd.is_empty() { None } else { Some(m.cwd.clone()) }),
                worktree_branch,
            }
        })
        .collect()
}

/// epoch ms → ISO 8601 字符串(对齐 TS `new Date(ms).toISOString()`)。
fn format_millis_iso(ms: i64) -> String {
    if ms <= 0 {
        return "1970-01-01T00:00:00.000Z".to_string();
    }
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}

/// 有界读取文件第一行(对齐 openSync + readSync 循环)。
fn read_first_line(file_path: &str, max_bytes: usize) -> Option<String> {
    let mut file = std::fs::File::open(file_path).ok()?;
    let mut buf = vec![0u8; 4096.min(max_bytes)];
    let mut chunks: Vec<u8> = Vec::new();
    let mut position = 0u64;

    while position < max_bytes as u64 {
        let to_read = buf.len().min(max_bytes - position as usize);
        let n = file.read(&mut buf[..to_read]).ok()?;
        if n == 0 {
            break;
        }
        let data = &buf[..n];
        if let Some(idx) = data.iter().position(|&b| b == b'\n') {
            chunks.extend_from_slice(&data[..idx]);
            return String::from_utf8(chunks).ok();
        }
        chunks.extend_from_slice(data);
        position += n as u64;
    }
    String::from_utf8(chunks).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_cache_roundtrip() {
        cache_session_path("test-id-1", "/home/user/.pi/sessions/abc.jsonl");
        assert_eq!(
            resolve_session_path("test-id-1"),
            Some("/home/user/.pi/sessions/abc.jsonl".to_string())
        );
        invalidate_path_cache("test-id-1");
        assert!(resolve_session_path("test-id-1").is_none());
    }

    #[tokio::test]
    async fn read_header_of_test_file() {
        // 写一个临时 jsonl 测试 header 读取
        let dir = std::env::temp_dir();
        let path = dir.join("pi_web_rust_test_session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session","id":"abc-123","timestamp":"2024-01-01T00:00:00Z","cwd":"/tmp"}"#,
        )
        .unwrap();

        let header = read_session_header(path.to_str().unwrap());
        assert!(header.is_some());
        let header = header.unwrap();
        assert_eq!(header.id, "abc-123");
        assert_eq!(header.cwd, "/tmp");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_header_nonexistent() {
        assert!(read_session_header("/nonexistent/path/file.jsonl").is_none());
    }
}
