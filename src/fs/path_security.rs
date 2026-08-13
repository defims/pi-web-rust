//! 对齐 `lib/path-security.ts`。
//!
//! 路径越界防护。`is_path_within_roots` 是纯计算(规范化 + 前缀匹配),
//! `is_existing_path_within_roots` 需要 canonicalize(解析符号链接)走 async。

/// 对齐 `isPathWithinRoots`。纯计算,规范化路径后做前缀匹配。
///
/// 对齐 TS:若 target 或任一 root 是 Windows 绝对路径,走大小写不敏感比较
/// (win32 文件系统大小写不敏感,含盘符 `d:\repo` vs `D:\repo`);否则 posix 大小写敏感。
/// 路径均经 `path.resolve`(绝对化 + 词法归一化)后再比较。
pub fn is_path_within_roots(target: &str, roots: &std::collections::HashSet<String>) -> bool {
    let is_windows = crate::paths::is_windows_absolute_path(target)
        || roots
            .iter()
            .any(|r| crate::paths::is_windows_absolute_path(r));
    let resolve = |p: &str| -> String {
        let r = crate::paths::resolve(p);
        if is_windows {
            r.to_ascii_lowercase()
        } else {
            r
        }
    };
    let resolved_target = resolve(target);
    for root in roots {
        let resolved_root = resolve(root);
        // `root + "/"` 避免前缀误匹配(/a/b vs /a/bb)
        if resolved_target == resolved_root
            || resolved_target.starts_with(&format!("{resolved_root}/"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn within_roots() {
        let mut roots = HashSet::new();
        roots.insert("/home/user/project".to_string());
        assert!(is_path_within_roots(
            "/home/user/project/src/main.rs",
            &roots
        ));
        assert!(is_path_within_roots("/home/user/project", &roots));
        assert!(!is_path_within_roots("/home/user/other", &roots));
        assert!(!is_path_within_roots("/home/user/projectt/evil", &roots));
    }

    #[test]
    fn normalize_dotdot() {
        let mut roots = HashSet::new();
        roots.insert("/home/user/project".to_string());
        assert!(!is_path_within_roots(
            "/home/user/project/../../../etc/passwd",
            &roots
        ));
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
