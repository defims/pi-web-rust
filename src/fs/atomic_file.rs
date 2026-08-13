//! 对齐 `lib/atomic-file.ts`。
//!
//! 原子文件写入:写临时文件(wx 独占)→ rename → 清理。
//! async + std::thread(运行时无关)。

use std::io;
use std::path::Path;

/// 对齐 `writePrivateFileAtomicSync`。原子写入,mode 0600(Unix 私有权限)。
///
/// async 版:在线程里做阻塞 IO,经 oneshot 回传。
pub async fn write_private_file_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let path = path.to_path_buf();
    let contents = contents.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = write_private_file_atomic_blocking(&path, &contents);
        let _ = tx.send(result);
    });
    rx.await.map_err(|_| io::Error::other("thread panicked"))?
}

/// 同步版(供非 async 调用方直接用)。
pub fn write_private_file_atomic_blocking(path: &Path, contents: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let basename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let temp_path = dir.join(format!(".{basename}-{}.tmp", uuid::Uuid::new_v4()));

    let result: io::Result<()> = (|| {
        use std::io::Write;
        // O_WRONLY | O_CREAT | O_EXCL (wx flag)
        let mut file = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temp_path)?
            }
            #[cfg(not(unix))]
            {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp_path)?
            }
        };
        file.write_all(contents.as_bytes())?;
        file.sync_all()?; // flush = fsync(跨平台)
        drop(file);
        std::fs::rename(&temp_path, path)?;
        Ok(())
    })();

    // finally: 清理临时文件(rename 成功后 temp_path 已不存在,unlink 忽略 ENOENT)
    let _ = std::fs::remove_file(&temp_path);

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_atomic() {
        let dir = std::env::temp_dir();
        let path = dir.join("pi_web_rust_test_atomic.txt");
        write_private_file_atomic(&path, "hello world")
            .await
            .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn uuid_format() {
        // 对齐 crypto.randomUUID():密码学随机 UUID v4。
        let id = uuid::Uuid::new_v4().to_string();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[8], b'-');
        assert_eq!(id.as_bytes()[14], b'4'); // version 4
                                             // 两次生成应不同(非确定性)
        assert_ne!(uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    }
}
