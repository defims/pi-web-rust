//! 对齐 `lib/file-access.ts`。
//!
//! 文件根白名单组合(会话 cwd + projectRoot + ~/pi-cwd-* + 额外根) +
//! 路径越界校验。5s TTL 缓存。
//!
//! 依赖:`fs::allowed_roots`(额外根) + `fs::path_security`(越界校验) +
//! `session::reader`(会话 cwd 扫描)。

use std::collections::HashSet;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::allowed_roots::{get_additional_allowed_roots, normalize_slashes};
use super::path_security::is_existing_path_within_roots;

#[cfg(test)]
use super::allowed_roots::allow_file_root;

const ALLOWED_ROOTS_TTL: Duration = Duration::from_secs(5);

struct AllowedRootsCache {
    roots: HashSet<String>,
    expires_at: Option<Instant>,
}

static ROOTS_CACHE: LazyLock<Mutex<AllowedRootsCache>> = LazyLock::new(|| {
    Mutex::new(AllowedRootsCache {
        roots: HashSet::new(),
        expires_at: None,
    })
});

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
/// 异步版 `get_allowed_file_roots`:把 `read_dir` + `home_dir()`(可能 `getpwuid`)
/// 移入线程,避免阻塞 executor。缓存逻辑与同步版一致。

/// `~/pi-cwd-YYYY-MM-DD` 目录名判定(上游 default-cwd route 用
/// `toISOString().slice(0,10)` 建名,自动放行模式与此一致)。
/// 此前误匹配无横线 15 字符形式,导致 default-cwd 自建的 17 字符目录
/// 不被放行 → /api/models?cwd= 403 → 新建会话底部模型选择器消失。
fn is_pi_cwd_dir_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if !name.starts_with("pi-cwd-") || bytes.len() != 17 {
        return false;
    }
    let date = &bytes[7..];
    // YYYY-MM-DD:位置 4 与 7 为 '-',其余为数字
    (0..10).all(|i| match i {
        4 | 7 => date[i] == b'-',
        _ => date[i].is_ascii_digit(),
    })
}

pub async fn get_allowed_file_roots_async(
    session_roots: HashSet<String>,
) -> HashSet<String> {
    // 检查缓存(快速路径,纯内存,无需线程)
    {
        let cache = ROOTS_CACHE.lock().unwrap();
        if let Some(expires_at) = cache.expires_at {
            if expires_at > Instant::now() {
                return cache.roots.clone();
            }
        }
    }

    // 慢路径(可能 read_dir + getpwuid)移入线程
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let mut roots = HashSet::new();

        // 1. 会话 cwd + projectRoot
        for root in &session_roots {
            let normalized = normalize_slashes(root);
            if !normalized.is_empty() {
                roots.insert(normalized);
            }
        }

        // 2. ~/pi-cwd-* 目录
        if let Some(home) = crate::paths::home_dir() {
            if let Ok(entries) = std::fs::read_dir(&home) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if is_pi_cwd_dir_name(&name_str) {
                        roots.insert(normalize_slashes(&entry.path().to_string_lossy()));
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

        let _ = tx.send(roots);
    });
    rx.await.unwrap_or_default()
}

/// 同步版 `get_allowed_file_roots`(供不需要 async 的调用方/测试使用)。
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

    // 2. ~/pi-cwd-* 目录(对齐 TS 扫描 os.homedir():HOME,缺失回退 getpwuid passwd)
    if let Some(home) = crate::paths::home_dir() {
        if let Ok(entries) = std::fs::read_dir(&home) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // 匹配 pi-cwd-YYYY-MM-DD(default-cwd 同款命名)
                if is_pi_cwd_dir_name(&name_str) {
                    roots.insert(normalize_slashes(&entry.path().to_string_lossy()));
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
pub async fn is_existing_file_path_allowed(target: &str, allowed_roots: &HashSet<String>) -> bool {
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
        assert!(is_file_path_allowed(
            "/home/user/project/src/main.rs",
            &roots
        ));
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

#[cfg(test)]
mod pi_cwd_pattern_tests {
    use super::is_pi_cwd_dir_name;

    #[test]
    fn dashed_date_matches_default_cwd_naming() {
        // default-cwd route(toISOString().slice(0,10))建的就是这种
        assert!(is_pi_cwd_dir_name("pi-cwd-2026-08-20"));
        assert!(is_pi_cwd_dir_name("pi-cwd-2026-01-01"));
    }

    #[test]
    fn wrong_shapes_rejected() {
        // 此前误配的无横线 15 字符形式(修复前 default-cwd 自建的目录反而不认)
        assert!(!is_pi_cwd_dir_name("pi-cwd-20260820"));
        assert!(!is_pi_cwd_dir_name("pi-cwd-2026-8-20"));
        assert!(!is_pi_cwd_dir_name("pi-cwd-202-08-20"));
        assert!(!is_pi_cwd_dir_name("pi-cwd-2026-08-2"));
        assert!(!is_pi_cwd_dir_name("pi-cwd-"));
        assert!(!is_pi_cwd_dir_name("other-2026-08-20"));
    }
}
