//! 对齐 `lib/session-path.ts`。路径→缓存键(规范化)。

/// 对齐 `sessionPathKey`。规范化路径作为缓存键。
/// macOS/Linux: normalize + 原样;Windows: normalize + toLowerCase。
pub fn session_path_key(file_path: &str) -> String {
    // normalize(消除 . 和 ..)
    let normalized = normalize_path_string(file_path);
    #[cfg(target_os = "windows")]
    {
        normalized.to_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        normalized
    }
}

/// 等价 TS path.normalize()。消除多余的 / 和 . 和 ..
fn normalize_path_string(p: &str) -> String {
    use std::path::{Path, PathBuf};
    let path = Path::new(p);
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
    out.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_key() {
        assert_eq!(session_path_key("/a/b/../c"), "/a/c");
        assert_eq!(session_path_key("/a/./b"), "/a/b");
    }
}
