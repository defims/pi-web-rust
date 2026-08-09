//! 对齐 `lib/file-upload.ts`。
//!
//! 上传冲突策略 + 文件名校验 + 目标检查。IO 部分(async)用 lstat 检查现有文件。

use std::path::Path;

use serde::{Deserialize, Serialize};

/// 对齐 `UploadConflictStrategy`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadConflictStrategy {
    Error,
    Overwrite,
    Skip,
}

/// 对齐 `parseUploadConflictStrategy`。
pub fn parse_upload_conflict_strategy(value: Option<&str>) -> Option<UploadConflictStrategy> {
    match value.unwrap_or("error") {
        "error" => Some(UploadConflictStrategy::Error),
        "overwrite" => Some(UploadConflictStrategy::Overwrite),
        "skip" => Some(UploadConflictStrategy::Skip),
        _ => None,
    }
}

/// 对齐 `validateUploadFileNames`。返回错误信息(None = 合法)。
pub fn validate_upload_file_names(file_names: &[String]) -> Option<String> {
    if file_names.is_empty() {
        return Some("No files selected".to_string());
    }

    let mut seen = std::collections::HashSet::new();
    for name in file_names {
        if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
            return Some(format!("Invalid file name: {}", if name.is_empty() { "(empty)" } else { name }));
        }
        if name.contains('/') || name.contains('\\') {
            return Some(format!("File names must not contain a path: {name}"));
        }
        // basename 检查:文件名不能含路径分隔符(已查,再确认 basename == name)
        if Path::new(name).file_name().map(|f| f != name.as_str()).unwrap_or(true) {
            return Some(format!("File names must not contain a path: {name}"));
        }
        if !seen.insert(name.clone()) {
            return Some(format!("Duplicate file name in upload: {name}"));
        }
    }
    None
}

/// 对齐 `UploadTargetInspection`。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UploadTargetInspection {
    pub conflicts: Vec<String>,
    pub non_replaceable: Vec<String>,
}

/// 对齐 `inspectUploadTargets`。检查目录下哪些文件已存在(conflicts)
/// 以及哪些不能覆盖(符号链接或非普通文件)。
pub async fn inspect_upload_targets(
    directory: &str,
    file_names: Vec<String>,
) -> std::io::Result<UploadTargetInspection> {
    let dir = directory.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let mut result = UploadTargetInspection::default();
        for name in &file_names {
            let dest = Path::new(&dir).join(name);
            let Ok(meta) = std::fs::symlink_metadata(&dest) else {
                continue; // ENOENT → 不冲突
            };
            result.conflicts.push(name.clone());
            // 符号链接或非普通文件不能覆盖
            if meta.is_symlink() || !meta.is_file() {
                result.non_replaceable.push(name.clone());
            }
        }
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| std::io::Error::other("thread panicked"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strategy() {
        assert_eq!(parse_upload_conflict_strategy(Some("error")), Some(UploadConflictStrategy::Error));
        assert_eq!(parse_upload_conflict_strategy(Some("overwrite")), Some(UploadConflictStrategy::Overwrite));
        assert_eq!(parse_upload_conflict_strategy(None), Some(UploadConflictStrategy::Error));
        assert_eq!(parse_upload_conflict_strategy(Some("invalid")), None);
    }

    #[test]
    fn validate_names() {
        assert!(validate_upload_file_names(&[]).is_some()); // empty
        assert!(validate_upload_file_names(&["a.rs".into(), "b.rs".into()]).is_none());
        assert!(validate_upload_file_names(&["../evil".into()]).is_some());
        assert!(validate_upload_file_names(&["a/b".into()]).is_some());
        assert!(validate_upload_file_names(&["a.rs".into(), "a.rs".into()]).is_some()); // duplicate
    }

    #[tokio::test]
    async fn inspect_targets() {
        let dir = std::env::temp_dir();
        let result = inspect_upload_targets(
            dir.to_str().unwrap(),
            vec!["nonexistent_file_for_test.txt".into()],
        )
        .await
        .unwrap();
        assert!(result.conflicts.is_empty());
    }
}
