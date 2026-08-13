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

/// 对齐 `getWindowsDriveCandidates`。A-Z 盘符候选(纯逻辑,无 IO)。
pub fn get_windows_drive_candidates() -> Vec<BrowsableDirectory> {
    (b'A'..=b'Z')
        .map(|letter| {
            let l = letter as char;
            BrowsableDirectory {
                name: format!("{l}:"),
                path: format!("{l}:\\"),
            }
        })
        .collect()
}

/// 对齐 `listWindowsDrives`。过滤到实际存在的盘符(`stat(drive.path).isDirectory()`)。
/// async + thread(运行时无关)。macOS/Linux 上无盘符 → 返回空。
pub async fn list_windows_drives() -> Vec<BrowsableDirectory> {
    let candidates = get_windows_drive_candidates();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result: Vec<BrowsableDirectory> = candidates
            .into_iter()
            .filter(|d| {
                std::fs::metadata(&d.path)
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            })
            .collect();
        let _ = tx.send(result);
    });
    rx.await.unwrap_or_default()
}

/// 对齐 `getBrowseStartDirectory`。
pub fn get_browse_start_directory(directory: Option<&str>) -> String {
    directory.map(String::from).unwrap_or_else(dirs_home)
}

/// 对齐 `normalizeDirectory`。展开 ~ 和 ~/,再经 `path.resolve`(绝对化 + 词法归一化)。
pub fn normalize_directory(directory: &str) -> PathBuf {
    let expanded = if directory == "~" {
        dirs_home()
    } else if let Some(rest) = directory.strip_prefix("~/") {
        format!("{}/{rest}", dirs_home())
    } else {
        directory.to_string()
    };
    PathBuf::from(crate::paths::resolve(&expanded))
}

/// 对齐 `getParentDirectory`。经 `path.resolve` 归一化后取父目录(`/a/b/../c` → `/a`)。
pub fn get_parent_directory(directory: &str) -> Option<String> {
    let resolved = crate::paths::resolve(directory);
    let path = PathBuf::from(&resolved);
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
        let result = std::fs::canonicalize(&normalized).map(|p| p.to_string_lossy().to_string());
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
            // 对齐 TS `name.localeCompare`:大小写不敏感比较(近似;无 ICU,重音排序可能微差)。
            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            Ok(dirs)
        })();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| std::io::Error::other("thread panicked"))?
}

fn dirs_home() -> String {
    // 对齐 os.homedir():HOME,缺失回退 getpwuid passwd;都无法确定时 "/"。
    crate::paths::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string())
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

    #[test]
    fn windows_drive_candidates() {
        // 对齐 TS:A-Z 共 26 个,首项 name="A:" path="A:\"
        let cands = get_windows_drive_candidates();
        assert_eq!(cands.len(), 26);
        assert_eq!(cands[0].name, "A:");
        assert_eq!(cands[0].path, "A:\\");
        assert_eq!(cands[25].name, "Z:");
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
