//! 对齐 `lib/allowed-roots.ts`。
//!
//! 运行时动态加白名单根。用全局 Mutex<HashSet> 替代 TS 的 globalThis。

use std::collections::HashSet;
use std::sync::Mutex;

use std::sync::LazyLock;

static ADDITIONAL_ROOTS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// 对齐 `normalizeSlashes`。
pub fn normalize_slashes(file_path: &str) -> String {
    file_path.replace('\\', "/")
}

/// 对齐 `getAdditionalAllowedRoots`。返回当前所有额外根的快照。
pub fn get_additional_allowed_roots() -> HashSet<String> {
    ADDITIONAL_ROOTS
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// 对齐 `allowFileRoot`。动态添加白名单根。
pub fn allow_file_root(root: &str) {
    if root.is_empty() {
        return;
    }
    let normalized = normalize_slashes(root);
    if let Ok(mut guard) = ADDITIONAL_ROOTS.lock() {
        guard.insert(normalized);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize() {
        assert_eq!(normalize_slashes(r"C:\Users\test"), "C:/Users/test");
        assert_eq!(normalize_slashes("/unix/path"), "/unix/path");
    }

    #[test]
    fn allow_and_get() {
        allow_file_root("/tmp/test_allowed_roots");
        let roots = get_additional_allowed_roots();
        assert!(roots.contains("/tmp/test_allowed_roots"));
    }
}
