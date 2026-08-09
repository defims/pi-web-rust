//! 对齐 `lib/file-dirent.ts`。目录条目类型判定。
//!
//! `Dirent` 不能直接判断目录时回退 `fs.statSync`;失败返回 null(对齐 TS)。

use std::io;
use std::path::Path;

/// 对齐 `resolveDirentIsDirectory`。
///
/// `dirent_is_directory` / `dirent_is_file` 注入 `fs.Dirent` 的判定;
/// 二者都不能确定时回退 `statSync`。失败(文件消失等)返回 None(对齐 null)。
pub fn resolve_dirent_is_directory(
    dirent_is_directory: bool,
    dirent_is_file: bool,
    full_path: &Path,
) -> Option<bool> {
    if dirent_is_directory {
        return Some(true);
    }
    if dirent_is_file {
        return Some(false);
    }
    match stat_is_directory(full_path) {
        Ok(is_dir) => Some(is_dir),
        Err(_) => None,
    }
}

fn stat_is_directory(path: &Path) -> io::Result<bool> {
    std::fs::metadata(path).map(|m| m.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirent_knows_directly() {
        assert_eq!(resolve_dirent_is_directory(true, false, Path::new("/x")), Some(true));
        assert_eq!(resolve_dirent_is_directory(false, true, Path::new("/x")), Some(false));
    }

    #[test]
    fn stat_fallback() {
        // 真实目录 → stat 判定 true
        let dir = std::env::temp_dir().join(format!("pi-dirent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(resolve_dirent_is_directory(false, false, &dir), Some(true));
        // 真实文件 → stat 判定 false
        let file = dir.join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(resolve_dirent_is_directory(false, false, &file), Some(false));
        // 不存在 → null
        assert_eq!(
            resolve_dirent_is_directory(false, false, &dir.join("missing")),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
