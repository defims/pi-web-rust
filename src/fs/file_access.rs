//! 对齐 `lib/file-access.ts`。
//!
//! 文件根白名单组合(会话 cwd + projectRoot + ~/pi-cwd-* + 额外根) +
//! 路径越界校验。5s TTL 缓存。
//!
//! 依赖:`fs::allowed_roots`(额外根) + `fs::path_security`(越界校验) +
//! `session::reader`(会话 cwd 扫描)。

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use super::allowed_roots::{get_additional_allowed_roots, normalize_slashes, allow_file_root};
use super::path_security::is_existing_path_within_roots;

const ALLOWED_ROOTS_TTL: Duration = Duration::from_secs(5);

struct AllowedRootsCache {
    roots: HashSet<String>,
    expires_at: Option<Instant>,
}

static ROOTS_CACHE: LazyLock<Mutex<AllowedRootsCache>> =
    LazyLock::new(|| Mutex::new(AllowedRootsCache { roots: HashSet::new(), expires_at: None }));

/// 对齐 `isWindowsAbsolutePath`。
pub fn is_windows_absolute_path(file_path: &str) -> bool {
    (file_path.len() >= 3
        && file_path.as_bytes()[0].is_ascii_alphabetic()
        && file_path.as_bytes()[1] == b':'
        && (file_path.as_bytes()[2] == b'\\' || file_path.as_bytes()[2] == b'/'))
        || file_path.starts_with("\\\\")
        || file_path.starts_with("//")
}

/// 对齐 `getAllowedFileRoots`。组合所有允许根:
/// 1. 会话 cwd + projectRoot(需调用方预热 session 列表)
/// 2. ~/pi-cwd-* 目录
/// 3. 额外根(allowFileRoot 动态添加)
///
/// session 列表获取是 async 的,由调用方传入 session_roots(HashSet<String>),
/// 本函数负责缓存 + 组合 ~/pi-cwd-* + 额外根。
pub fn get_allowed_file_roots(session_roots: &HashSet<String>) -> HashSet<String> {
    // 检查缓存
    {
        let cache = ROOTS_CACHE.lock().unwrap();
        if let Some(expires_at) = cache.expires_at {
            if expires_at > Instant::now() {
                return cache.roots.clone();
            }
        }
    }

    let mut roots = HashSet::new();

    // 1. 会话 cwd + projectRoot
    for root in session_roots {
        let normalized = normalize_slashes(root);
        if !normalized.is_empty() {
            roots.insert(normalized);
        }
    }

    // 2. ~/pi-cwd-* 目录
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(entries) = std::fs::read_dir(&home) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // 匹配 pi-cwd-YYYYMMDD
                if name_str.starts_with("pi-cwd-")
                    && name_str.len() == 15 // "pi-cwd-" (7) + 8 digits
                    && name_str[7..].chars().all(|c| c.is_ascii_digit())
                {
                    roots.insert(normalize_slashes(
                        &entry.path().to_string_lossy(),
                    ));
                }
            }
        }
    }

    // 3. 额外根
    for root in get_additional_allowed_roots() {
        roots.insert(root);
    }

    // 写缓存
    {
        let mut cache = ROOTS_CACHE.lock().unwrap();
        cache.roots = roots.clone();
        cache.expires_at = Some(Instant::now() + ALLOWED_ROOTS_TTL);
    }

    roots
}

/// 对齐 `isFilePathAllowed`。纯计算(规范化 + 前缀匹配),委托 path_security。
pub fn is_file_path_allowed(target: &str, allowed_roots: &HashSet<String>) -> bool {
    super::path_security::is_path_within_roots(target, allowed_roots)
}

/// 对齐 `isExistingFilePathAllowed`。canonicalize 后校验(async IO)。
pub async fn is_existing_file_path_allowed(
    target: &str,
    allowed_roots: &HashSet<String>,
) -> bool {
    is_existing_path_within_roots(target, allowed_roots).await
}

/// 失效缓存(对齐 session 变化后调用)。
pub fn invalidate_allowed_roots_cache() {
    let mut cache = ROOTS_CACHE.lock().unwrap();
    cache.expires_at = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_path_detection() {
        assert!(is_windows_absolute_path("C:\\Users\\test"));
        assert!(is_windows_absolute_path("D:/foo"));
        assert!(!is_windows_absolute_path("/unix/path"));
    }

    #[test]
    fn file_path_allowed() {
        let mut roots = HashSet::new();
        roots.insert("/home/user/project".to_string());
        assert!(is_file_path_allowed("/home/user/project/src/main.rs", &roots));
        assert!(!is_file_path_allowed("/etc/passwd", &roots));
    }

    #[test]
    fn allowed_roots_with_session() {
        let mut session_roots = HashSet::new();
        session_roots.insert("/tmp/test_session_root".to_string());
        allow_file_root("/tmp/test_extra_root");
        let roots = get_allowed_file_roots(&session_roots);
        assert!(roots.contains("/tmp/test_session_root"));
        assert!(roots.contains("/tmp/test_extra_root"));
    }
}
