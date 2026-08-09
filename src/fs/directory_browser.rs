//! 对齐 `lib/directory-browser.ts`。
//!
//! cwd 选择器目录浏览。async + std::thread(运行时无关)。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 对齐 `BrowsableDirectory`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowsableDirectory {
    pub name: String,
    pub path: String,
}

/// 对齐 `shouldShowWindowsDrivePicker`。macOS/Linux 恒 false。
pub fn should_show_windows_drive_picker(directory: Option<&str>) -> bool {
    cfg!(target_os = "windows") && directory.is_none()
}

/// 对齐 `getBrowseStartDirectory`。
pub fn get_browse_start_directory(directory: Option<&str>) -> String {
    directory
        .map(String::from)
        .unwrap_or_else(dirs_home)
}

/// 对齐 `normalizeDirectory`。展开 ~ 和 ~/。
pub fn normalize_directory(directory: &str) -> PathBuf {
    if directory == "~" {
        return PathBuf::from(dirs_home());
    }
    if let Some(rest) = directory.strip_prefix("~/") {
        return PathBuf::from(dirs_home()).join(rest);
    }
    PathBuf::from(directory)
}

/// 对齐 `getParentDirectory`。
pub fn get_parent_directory(directory: &str) -> Option<String> {
    let path = PathBuf::from(directory);
    let parent = path.parent()?;
    if parent == path {
        None
    } else {
        Some(parent.to_string_lossy().to_string())
    }
}

/// 对齐 `resolveDirectory`。canonicalize(realpath)。
pub async fn resolve_directory(directory: &str) -> std::io::Result<String> {
    let normalized = normalize_directory(directory);
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = std::fs::canonicalize(&normalized)
            .map(|p| p.to_string_lossy().to_string());
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| std::io::Error::other("thread panicked"))?
}

/// 对齐 `listDirectories`。列出目录下的子目录(含符号链接解析后是目录的)。
pub async fn list_directories(directory: &str) -> std::io::Result<Vec<BrowsableDirectory>> {
    let dir = directory.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result: std::io::Result<Vec<BrowsableDirectory>> = (|| {
            let entries = std::fs::read_dir(&dir)?;
            let mut dirs: Vec<BrowsableDirectory> = Vec::new();
            for entry in entries {
                let entry = entry?;
                let entry_path = entry.path();
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    dirs.push(BrowsableDirectory {
                        name: entry.file_name().to_string_lossy().to_string(),
                        path: entry_path.to_string_lossy().to_string(),
                    });
                } else if file_type.is_symlink() {
                    // 解析符号链接,是目录才加
                    if let Ok(real) = std::fs::metadata(&entry_path) {
                        if real.is_dir() {
                            dirs.push(BrowsableDirectory {
                                name: entry.file_name().to_string_lossy().to_string(),
                                path: entry_path.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
            dirs.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(dirs)
        })();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| std::io::Error::other("thread panicked"))?
}

fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_directory() {
        assert_eq!(
            get_parent_directory("/home/user/project"),
            Some("/home/user".to_string())
        );
        assert_eq!(get_parent_directory("/"), None);
    }

    #[test]
    fn normalize() {
        let n = normalize_directory("/tmp");
        assert_eq!(n, PathBuf::from("/tmp"));
    }

    #[tokio::test]
    async fn list_tmp() {
        // /tmp 在 unix 系统上存在
        let dirs = list_directories("/tmp").await;
        // 可能成功也可能因权限失败,只要不 panic 就行
        if let Ok(dirs) = dirs {
            // 至少返回了 Vec(可能为空)
            assert!(dirs.iter().all(|d| !d.name.is_empty()));
        }
    }
}
