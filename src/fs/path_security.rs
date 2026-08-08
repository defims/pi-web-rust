//! 对齐 `lib/path-security.ts`。
//!
//! 路径越界防护。`is_path_within_roots` 是纯计算(规范化 + 前缀匹配),
//! `is_existing_path_within_roots` 需要 canonicalize(解析符号链接)走 async。

use std::path::{Path, PathBuf};

/// 对齐 `isPathWithinRoots`。纯计算,规范化路径后做前缀匹配。
///
/// Rust 用 `Path::components` 做组件级 starts_with,比 TS 字符串前缀更严谨
/// (天然避免 /a/b vs /a/bb)。
pub fn is_path_within_roots(target: &str, roots: &std::collections::HashSet<String>) -> bool {
    let target_path = PathBuf::from(target);
    for root in roots {
        let root_path = PathBuf::from(root);
        // 对齐 TS resolve(): 规范化路径(. 和 .. 消除)
        let normalized_target = normalize_path(&target_path);
        let normalized_root = normalize_path(&root_path);
        if normalized_target == normalized_root
            || normalized_target.starts_with(&normalized_root)
        {
            return true;
        }
    }
    false
}

/// 对齐 `isExistingPathWithinRoots`。canonicalize 解析符号链接后做前缀匹配。
///
/// async + std::thread:canonicalize 是阻塞 IO,在线程里跑,
/// 经 oneshot channel 回传结果。不绑定特定 runtime。
pub async fn is_existing_path_within_roots(
    target: &str,
    roots: &std::collections::HashSet<String>,
) -> bool {
    let target = target.to_string();
    let roots = roots.clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = (|| -> bool {
            let real_target = match std::fs::canonicalize(&target) {
                Ok(p) => p,
                Err(_) => return false,
            };
            let mut real_roots = std::collections::HashSet::new();
            for root in &roots {
                if let Ok(real) = std::fs::canonicalize(root) {
                    real_roots.insert(real.to_string_lossy().to_string());
                }
            }
            let target_str = real_target.to_string_lossy().to_string();
            is_path_within_roots(&target_str, &real_roots)
        })();
        let _ = tx.send(result);
    });
    rx.await.unwrap_or(false)
}

/// 规范化路径(消除 . 和 ..)。等价 TS path.resolve() 的规范化部分。
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn within_roots() {
        let mut roots = HashSet::new();
        roots.insert("/home/user/project".to_string());
        assert!(is_path_within_roots("/home/user/project/src/main.rs", &roots));
        assert!(is_path_within_roots("/home/user/project", &roots));
        assert!(!is_path_within_roots("/home/user/other", &roots));
        assert!(!is_path_within_roots("/home/user/projectt/evil", &roots));
    }

    #[test]
    fn normalize_dotdot() {
        let mut roots = HashSet::new();
        roots.insert("/home/user/project".to_string());
        assert!(!is_path_within_roots("/home/user/project/../../../etc/passwd", &roots));
    }

    #[tokio::test]
    async fn existing_path_within_roots() {
        let mut roots = HashSet::new();
        roots.insert("/tmp".to_string());
        // /tmp 应该存在于任何 unix 系统
        assert!(is_existing_path_within_roots("/tmp", &roots).await);
        assert!(!is_existing_path_within_roots("/nonexistent/path", &roots).await);
    }
}
