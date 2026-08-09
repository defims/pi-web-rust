//! 对齐 `lib/bash-output.ts`。bash 输出文件读取(O_NOFOLLOW + 限字节)。

use std::io::Read;
use std::path::Path;

pub const MAX_INLINE_BASH_OUTPUT_BYTES: usize = 5 * 1024 * 1024;

/// 对齐 `resolveBashOutputPath`。校验路径在 tempRoot 下且匹配 pi-bash-*.log。
pub fn resolve_bash_output_path(file_path: &str, temp_root: &str) -> Option<String> {
    let resolved = Path::new(file_path);
    let parent = resolved.parent()?;
    let root = Path::new(temp_root);
    if parent != root {
        return None;
    }
    let basename = resolved.file_name()?.to_string_lossy().to_string();
    if !is_valid_bash_output_name(&basename) {
        return None;
    }
    Some(resolved.to_string_lossy().to_string())
}

fn is_valid_bash_output_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("pi-bash-") else { return false; };
    let Some(stem) = rest.strip_suffix(".log") else { return false; };
    !stem.is_empty()
        && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 对齐 `readUtf8FileWithinLimit`。读文件,限字节,symlink_metadata(不跟随)。
pub async fn read_utf8_file_within_limit(
    file_path: &str,
    max_bytes: Option<usize>,
) -> std::io::Result<ReadResult> {
    let max = max_bytes.unwrap_or(MAX_INLINE_BASH_OUTPUT_BYTES);
    let path = file_path.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<ReadResult> {
            let meta = std::fs::symlink_metadata(&path)?;
            if !meta.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Bash output path is not a regular file",
                ));
            }
            let size = meta.len() as usize;
            if size > max {
                return Ok(ReadResult::TooLarge { size });
            }
            let mut file = std::fs::File::open(&path)?;
            let mut buf = Vec::with_capacity(size);
            file.read_to_end(&mut buf)?;
            let content = String::from_utf8_lossy(&buf).to_string();
            Ok(ReadResult::Content { content, size: buf.len() })
        })();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| std::io::Error::other("thread panicked"))?
}

/// 对齐返回联合类型。
#[derive(Debug)]
pub enum ReadResult {
    TooLarge { size: usize },
    Content { content: String, size: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_valid_path() {
        assert_eq!(
            resolve_bash_output_path("/tmp/pi-bash-abc123.log", "/tmp"),
            Some("/tmp/pi-bash-abc123.log".to_string())
        );
    }

    #[test]
    fn reject_invalid_names() {
        assert!(resolve_bash_output_path("/tmp/not-bash.log", "/tmp").is_none());
        assert!(resolve_bash_output_path("/tmp/pi-bash-evil../../.ssh/id_rsa", "/tmp").is_none());
        assert!(resolve_bash_output_path("/other/pi-bash-abc.log", "/tmp").is_none());
    }

    #[tokio::test]
    async fn read_small_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("pi_web_rust_bash_output_test.log");
        std::fs::write(&path, "hello output").unwrap();
        let result =
            read_utf8_file_within_limit(path.to_str().unwrap(), None)
                .await
                .unwrap();
        match result {
            ReadResult::Content { content, .. } => assert_eq!(content, "hello output"),
            _ => panic!("expected content"),
        }
        let _ = std::fs::remove_file(&path);
    }
}
