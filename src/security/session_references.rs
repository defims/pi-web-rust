//! 对齐 `lib/session-file-references.ts`。文件是否被某会话引用。
//!
//! 核心判定(`is_file_path_referenced_by_entries` / `is_bash_output_path_referenced_by_entries`
//! / `is_valid_session_id`)在 `security` 模块;本模块是会话层包装:
//! 校验 session id → 解析会话路径 → 读 entries → 判定。TS 里是 async
//! (路径解析可能触发全量扫描),Rust 版提供 sync + async 两版,路径解析经注入回调。

use crate::security::{
    is_bash_output_path_referenced_by_entries, is_file_path_referenced_by_entries,
    is_valid_session_id,
};

/// 会话路径解析回调:缓存命中直接返回;miss 时宿主可触发扫描后重试
/// (对齐 TS `resolveSessionPath` 的 cache → listAllSessions → retry)。
pub type ResolveSessionPathFn = fn(session_id: &str) -> Option<String>;

/// 对齐 `isFilePathReferencedBySession` 的同步版。
pub fn is_file_path_referenced_by_session(
    file_path: &str,
    session_id: Option<&str>,
    resolve_path: ResolveSessionPathFn,
    read_entries: impl FnOnce(&str) -> Vec<serde_json::Value>,
) -> bool {
    if !is_valid_session_id(session_id) {
        return false;
    }
    let Some(session_path) = resolve_path(session_id.unwrap()) else {
        return false;
    };
    is_file_path_referenced_by_entries(file_path, &read_entries(&session_path))
}

/// 对齐 `isBashOutputPathReferencedBySession` 的同步版。
pub fn is_bash_output_path_referenced_by_session(
    file_path: &str,
    session_id: Option<&str>,
    resolve_path: ResolveSessionPathFn,
    read_entries: impl FnOnce(&str) -> Vec<serde_json::Value>,
) -> bool {
    if !is_valid_session_id(session_id) {
        return false;
    }
    let Some(session_path) = resolve_path(session_id.unwrap()) else {
        return false;
    };
    is_bash_output_path_referenced_by_entries(file_path, &read_entries(&session_path))
}

/// 对齐 async 版(经 std::thread + oneshot,运行时无关)。
pub async fn is_file_path_referenced_by_session_async(
    file_path: &str,
    session_id: Option<&str>,
    resolve_path: ResolveSessionPathFn,
    read_entries: impl FnOnce(&str) -> Vec<serde_json::Value> + Send + 'static,
) -> bool {
    let file_path = file_path.to_string();
    let session_id = session_id.map(|s| s.to_string());
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = is_file_path_referenced_by_session(
            &file_path,
            session_id.as_deref(),
            resolve_path,
            read_entries,
        );
        let _ = tx.send(result);
    });
    rx.await.unwrap_or(false)
}

/// 对齐 async 版(经 std::thread + oneshot,运行时无关)。
pub async fn is_bash_output_path_referenced_by_session_async(
    file_path: &str,
    session_id: Option<&str>,
    resolve_path: ResolveSessionPathFn,
    read_entries: impl FnOnce(&str) -> Vec<serde_json::Value> + Send + 'static,
) -> bool {
    let file_path = file_path.to_string();
    let session_id = session_id.map(|s| s.to_string());
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = is_bash_output_path_referenced_by_session(
            &file_path,
            session_id.as_deref(),
            resolve_path,
            read_entries,
        );
        let _ = tx.send(result);
    });
    rx.await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cache_only(session_id: &str) -> Option<String> {
        match session_id {
            "12345678-1234-1234-1234-123456789abc" => Some("/tmp/sessions/s1.jsonl".to_string()),
            _ => None,
        }
    }

    fn read_entries(path: &str) -> Vec<serde_json::Value> {
        if path.ends_with("s1.jsonl") {
            vec![json!({"type":"message","message":{"content":"edit /home/test/foo.rs"}})]
        } else {
            vec![]
        }
    }

    #[test]
    fn invalid_session_id_short_circuits() {
        assert!(!is_file_path_referenced_by_session(
            "/home/test/foo.rs",
            Some("not-a-uuid"),
            cache_only,
            read_entries,
        ));
        assert!(!is_file_path_referenced_by_session(
            "/home/test/foo.rs",
            None,
            cache_only,
            read_entries,
        ));
    }

    #[test]
    fn unresolved_path_returns_false() {
        assert!(!is_file_path_referenced_by_session(
            "/x",
            Some("12345678-1234-1234-1234-123456789abc"),
            |_| None,
            read_entries,
        ));
    }

    #[test]
    fn referenced_and_not() {
        let sid = Some("12345678-1234-1234-1234-123456789abc");
        assert!(is_file_path_referenced_by_session(
            "/home/test/foo.rs",
            sid,
            cache_only,
            read_entries
        ));
        assert!(!is_file_path_referenced_by_session(
            "/home/test/other.rs",
            sid,
            cache_only,
            read_entries
        ));
    }

    #[test]
    fn bash_output_reference() {
        let sid = Some("12345678-1234-1234-1234-123456789abc");
        let entries = vec![json!({
            "type": "message",
            "message": { "role": "bashExecution", "fullOutputPath": "/tmp/out.txt" }
        })];
        let read = |_p: &str| entries.clone();
        assert!(is_bash_output_path_referenced_by_session(
            "/tmp/out.txt",
            sid,
            cache_only,
            read
        ));
        assert!(!is_bash_output_path_referenced_by_session(
            "/tmp/other.txt",
            sid,
            cache_only,
            read
        ));
    }

    #[tokio::test]
    async fn async_versions_match() {
        let sid = Some("12345678-1234-1234-1234-123456789abc");
        assert!(
            is_file_path_referenced_by_session_async(
                "/home/test/foo.rs",
                sid,
                cache_only,
                read_entries,
            )
            .await
        );
        assert!(
            !is_bash_output_path_referenced_by_session_async(
                "/tmp/missing.txt",
                sid,
                cache_only,
                read_entries,
            )
            .await
        );
    }
}
