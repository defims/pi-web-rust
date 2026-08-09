//! 对齐 `lib/session-reader.ts`。
//!
//! JSONL 会话文件读取(有界,64KB header) + 路径缓存层。
//! SessionManager.listAll/buildSessionContext 依赖 TS SDK(pi_agent_rust
//! 的 in_process 等价),标 TODO 待引擎层补齐。

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

/// 对齐 `getSessionEntries` 的低层部分:读 jsonl 文件的全部行。
/// TODO: 上层 buildSessionContext 依赖 TS SDK(pi_agent_rust Session),
/// 待引擎层补齐后实现 entry 解析 + 上下文构建。
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

/// 列出会话目录下的所有 .jsonl 文件(对齐 SessionManager.listAll 的文件扫描部分)。
/// TODO: SessionInfo 形状(firstMessage/messageCount/parentSessionId 等)需要读
/// header + 首条 entry,待上层实现。
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
