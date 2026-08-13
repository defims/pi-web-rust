//! 对齐 `lib/paths.ts`。跨平台路径原语:绝对路径判定 / 原生分隔符 / 正斜杠 / 同路径比较。
//!
//! 两种规范形式并存(按去向选用):
//! - `to_native_path()`:原生分隔符(Windows `D:\repo`)。用于触及 fs/path API、
//!   与 session cwd 比较、展示给用户。pi 以此形式记录 cwd。
//! - `to_slash_path()`:正斜杠(`D:/repo`)。仅用于内部记账(allowed-roots 集合、
//!   分隔符不敏感的文本匹配)。containment 校验本就会重新归一化输入。
//!
//! 比较一律走 `same_path()`,绝不用 `==`:git 即便在 Windows 也输出 POSIX 风格路径,
//! 且 Windows 文件系统大小写不敏感,裸字符串相等在两点上都会静默失败。
//!
//! 纯计算,无 fs/IO 依赖。`process.platform === "win32"` 对齐为编译期 `cfg!`。

use std::sync::LazyLock;

/// 对齐 `process.platform === "win32"`(编译期目标平台)。
const IS_WINDOWS: bool = cfg!(target_os = "windows");

/// 对齐 `WINDOWS_ABSOLUTE_RE = /^[a-zA-Z]:[\\/]/`。
static WINDOWS_ABSOLUTE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z]:[\\/]").expect("valid windows absolute regex"));

/// 对齐 `isWindowsAbsolutePath(filePath)`。
///
/// 匹配盘符路径(`D:\` / `d:/`)、UNC 前缀(`\\` 或 `//`)。
pub fn is_windows_absolute_path(file_path: &str) -> bool {
    WINDOWS_ABSOLUTE_RE.is_match(file_path)
        || file_path.starts_with("\\\\")
        || file_path.starts_with("//")
}

// ============================================================================
// Node `path.normalize` 忠实移植
// ============================================================================

/// 忠实移植 Node `path` 内部 `normalizeString`(`lib/path.js`)的段拼接算法。
///
/// `allow_above_root`:相对路径为 `true`(保留前导 `..`),绝对路径为 `false`
/// (根之上的 `..` 被丢弃)。
///
/// `sep_is`:判定分隔符字节(posix 仅 `/`;win32 `/` 与 `\`)。算法内部统一按 `/`
/// 处理,因此 win32 混入的 `\` 与 `/` 等价。
fn node_normalize_string(
    path: &str,
    allow_above_root: bool,
    sep_is: impl Fn(u8) -> bool,
) -> String {
    const SLASH: u8 = b'/';
    const DOT: u8 = b'.';

    let bytes = path.as_bytes();
    let len = bytes.len();
    let mut res: Vec<u8> = Vec::new();
    let mut last_segment_length: i64 = 0;
    let mut last_slash: i64 = -1;
    // Node 中 dots 初始为 0;遇分隔符重置为 0;遇 `.` 且 dots!=-1 时自增;否则 -1。
    let mut dots: i64 = 0;
    let mut i: usize = 0;

    while i <= len {
        let code: u8;
        if i < len {
            code = bytes[i];
        } else {
            // 末尾哨兵:若最后一个真实字符是分隔符则停止;否则注入一个虚拟分隔符 flush。
            if i > 0 && sep_is(bytes[i - 1]) {
                break;
            }
            code = SLASH;
        }

        if sep_is(code) {
            // 视作 `/`
            let cur = i as i64;
            if last_slash == cur - 1 || dots == 1 {
                // 空段(`//`)或单点段(`./`):noop
            } else if last_slash != cur - 1 && dots == 2 {
                // `..` 段
                if res.len() < 2
                    || last_segment_length != 2
                    || res[res.len() - 1] != DOT
                    || res[res.len() - 2] != DOT
                {
                    if res.len() > 2 {
                        let last_slash_index = res.iter().rposition(|&b| b == SLASH);
                        match last_slash_index {
                            Some(lsi) if lsi != res.len() - 1 => {
                                let new_last = res.iter().rposition(|&b| b == SLASH);
                                res.truncate(lsi);
                                last_segment_length = match new_last {
                                    Some(nl) => lsi as i64 - nl as i64 - 1,
                                    None => lsi as i64,
                                };
                                last_slash = cur;
                                dots = 0;
                                i += 1;
                                continue;
                            }
                            None => {
                                res.clear();
                                last_segment_length = 0;
                                last_slash = cur;
                                dots = 0;
                                i += 1;
                                continue;
                            }
                            _ => {}
                        }
                    } else if res.len() == 2 || res.len() == 1 {
                        res.clear();
                        last_segment_length = 0;
                        last_slash = cur;
                        dots = 0;
                        i += 1;
                        continue;
                    }
                }
                if allow_above_root {
                    if !res.is_empty() {
                        res.push(SLASH);
                        res.push(DOT);
                        res.push(DOT);
                    } else {
                        res.push(DOT);
                        res.push(DOT);
                    }
                    last_segment_length = 2;
                }
            } else {
                // 普通段:追加 path[last_slash+1 .. i]
                let seg_start = (last_slash + 1) as usize;
                if !res.is_empty() {
                    res.push(SLASH);
                }
                res.extend_from_slice(&bytes[seg_start..i]);
                last_segment_length = cur - last_slash - 1;
            }
            last_slash = cur;
            dots = 0;
        } else if code == DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }

        i += 1;
    }

    String::from_utf8(res).unwrap_or_default()
}

/// Node posix `normalize(path)`。
fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let is_absolute = path.as_bytes().first() == Some(&b'/');
    let trailing_separator = path.as_bytes().last() == Some(&b'/');

    let normalized = node_normalize_string(path, !is_absolute, |b| b == b'/');

    if normalized.is_empty() && !is_absolute {
        let mut res = ".".to_string();
        if trailing_separator {
            res.push('/');
        }
        return res;
    }
    let mut res = if is_absolute {
        format!("/{normalized}")
    } else {
        normalized
    };
    if trailing_separator && !res.is_empty() && res != "/" {
        res.push('/');
    }
    res
}

/// 计算 win32 根长度(盘符根 `C:\` = 3;UNC 根 `\\server\share\`;否则 0)。
fn win32_root_length(p: &str) -> usize {
    let bytes = p.as_bytes();
    // UNC: `\\server\share\` 或 `//server/share/`
    if p.starts_with("\\\\") || p.starts_with("//") {
        let sep = if p.starts_with("\\\\") { b'\\' } else { b'/' };
        // 跳过前两个 sep,找 server 结束,再跳过 share。
        let mut idx = 2usize;
        // server
        while idx < bytes.len() && bytes[idx] != sep {
            idx += 1;
        }
        // 跨过 server 与 share 之间的分隔符
        while idx < bytes.len() && bytes[idx] == sep {
            idx += 1;
        }
        // share
        while idx < bytes.len() && bytes[idx] != sep {
            idx += 1;
        }
        // 包含尾随分隔符(若有)
        if idx < bytes.len() && bytes[idx] == sep {
            idx += 1;
        }
        return idx;
    }
    // 盘符根 `C:\` / `c:/`
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return 3;
    }
    0
}

/// Node win32 `normalize(path)`(忠实近似:双分隔符等价 + 盘符/UNC 根保留)。
fn win32_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let root_len = win32_root_length(path);
    let root = &path[..root_len];
    let rest = &path[root_len..];
    let is_absolute = !root.is_empty();
    // 原始尾部分隔符(双分隔符任一)
    let trailing_separator = path
        .as_bytes()
        .last()
        .map_or(false, |b| *b == b'\\' || *b == b'/');

    let normalized = node_normalize_string(rest, !is_absolute, |b| b == b'\\' || b == b'/');

    let mut res = if is_absolute {
        // 根统一为反斜杠形式(对齐 Node win32 输出)
        let root_bs = root.replace('/', "\\");
        if normalized.is_empty() {
            root_bs
        } else {
            format!("{root_bs}{normalized}")
        }
    } else if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized.replace('/', "\\")
    };
    if trailing_separator && !res.is_empty() && !res.ends_with('\\') && !res.ends_with('/') {
        res.push('\\');
    }
    res
}

/// Node `normalize`(按当前平台)。
fn node_normalize(path: &str) -> String {
    if IS_WINDOWS {
        win32_normalize(path)
    } else {
        posix_normalize(path)
    }
}

/// 对齐 Node `path.normalize`(平台相关)。供其他模块复用(如 session-reader 的
/// `cacheSessionPath` 经 `normalize(filePath)` 归一化后再缓存)。
pub fn normalize(path: &str) -> String {
    node_normalize(path)
}

/// 对齐 Node `path.resolve(p)`(单参数):相对路径以 cwd 绝对化,再 `normalize`。
/// 供 bash-output / path-security 等复用。
pub fn resolve(p: &str) -> String {
    let abs = if std::path::Path::new(p).is_absolute() {
        p.to_string()
    } else {
        let cwd = std::env::current_dir()
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_default();
        format!("{cwd}/{p}")
    };
    normalize(&abs)
}

/// 对齐 Node `os.homedir()`:优先 `HOME` 环境变量;unix 下缺失则回退
/// `getpwuid(getuid()).pw_dir`(passwd)。返回 `None` 表示无法确定。
pub fn home_dir() -> Option<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return Some(home.into());
        }
    }
    #[cfg(unix)]
    {
        // unsafe: libc FFI。getpwuid 返回指向静态 passwd 结构的指针(线程局部于部分实现,
        // 但单次读取 pw_dir 安全)。null 守卫齐全。
        unsafe {
            let uid = libc::getuid();
            let pw = libc::getpwuid(uid);
            if !pw.is_null() {
                let pw_dir = (*pw).pw_dir;
                if !pw_dir.is_null() {
                    if let Ok(s) = std::ffi::CStr::from_ptr(pw_dir).to_str() {
                        if !s.is_empty() {
                            return Some(s.into());
                        }
                    }
                }
            }
        }
    }
    None
}

/// 当前平台路径分隔符字节。
const SEP: u8 = if cfg!(target_os = "windows") {
    b'\\'
} else {
    b'/'
};

// ============================================================================
// 对齐 `lib/paths.ts` 公开函数
// ============================================================================

/// 对齐 `toNativePath(p)`。
///
/// git 即便在 Windows 也打印 POSIX 风格绝对路径(`D:/repo/sub`),永不会与 Node/pi
/// 产生的原生路径字符串相等。此函数在 Windows 上返回 `path.normalize(p)`;其他平台
/// 原样返回(空串亦原样返回)。
///
/// 仅传路径——分支名 `feature/x` 会被错误地变成 `feature\x`。
pub fn to_native_path(p: &str) -> String {
    if p.is_empty() || !IS_WINDOWS {
        return p.to_string();
    }
    node_normalize(p)
}

/// 对齐 `toSlashPath(p)`。反斜杠 → 正斜杠。
pub fn to_slash_path(p: &str) -> String {
    p.replace('\\', "/")
}

/// 对齐 `normalizeForComparison(p)`。
///
/// `normalize(toNativePath(p))` 后剥去除根之外的尾随分隔符(仅当前平台 sep)。
fn normalize_for_comparison(p: &str) -> String {
    let native = to_native_path(p);
    let normalized = node_normalize(&native);
    let bytes = normalized.as_bytes();
    let root_len = if IS_WINDOWS {
        win32_root_length(&normalized)
    } else if bytes.first() == Some(&b'/') {
        1
    } else {
        0
    };
    let mut end = bytes.len();
    while end > root_len && bytes[end - 1] == SEP {
        end -= 1;
    }
    normalized[..end].to_string()
}

/// 对齐 `samePath(a, b)`。
///
/// 判定两条路径是否指向同一位置,容忍分隔符风格;Windows 上额外容忍大小写
/// (含盘符 `d:\repo` vs `D:\repo`,因文件系统大小写不敏感)。
///
/// 词法比较:需要解析符号链接的调用方应先 realpath。
pub fn same_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let normalized_a = normalize_for_comparison(a);
    let normalized_b = normalize_for_comparison(b);
    if IS_WINDOWS {
        normalized_a.to_ascii_lowercase() == normalized_b.to_ascii_lowercase()
    } else {
        normalized_a == normalized_b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_native_path_passthrough_on_non_windows() {
        if !IS_WINDOWS {
            assert_eq!(to_native_path("/repo/sub"), "/repo/sub");
        }
        assert_eq!(to_native_path(""), "");
    }

    #[test]
    fn to_slash_path_normalizes_to_forward_slashes() {
        assert_eq!(to_slash_path("D:\\repo\\sub"), "D:/repo/sub");
        assert_eq!(to_slash_path("/repo/sub"), "/repo/sub");
    }

    #[test]
    fn is_windows_absolute_path_recognizes_drive_and_unc() {
        assert!(is_windows_absolute_path("D:\\repo"));
        assert!(is_windows_absolute_path("d:/repo"));
        assert!(is_windows_absolute_path("\\\\server\\share"));
        assert!(!is_windows_absolute_path("relative/path"));
    }

    #[test]
    fn same_path_ignores_separator_style() {
        assert!(same_path("/a/b", "/a/b"));
        assert!(
            same_path("/a/b/", "/a/b"),
            "trailing separators must not matter"
        );
        assert!(same_path("/a/./b", "/a/b"), "dot segments must not matter");
        assert!(!same_path("/a/b", "/a/c"));
        assert!(same_path("", ""));
        assert!(!same_path("", "/a"));
    }

    #[test]
    fn same_path_posix_case_sensitive_and_backslash_literal() {
        if !IS_WINDOWS {
            assert!(!same_path("/Repo", "/repo"));
            assert!(!same_path("/a\\b", "/a/b"));
        }
    }

    #[test]
    fn posix_normalize_matches_node() {
        // 与 Node path.posix.normalize 逐一对齐
        assert_eq!(posix_normalize(""), ".");
        assert_eq!(posix_normalize("."), ".");
        assert_eq!(posix_normalize("./a"), "a");
        assert_eq!(posix_normalize("a/./b"), "a/b");
        assert_eq!(posix_normalize("a//b"), "a/b");
        assert_eq!(posix_normalize("/a/b/"), "/a/b/");
        assert_eq!(posix_normalize("/a/b"), "/a/b");
        assert_eq!(posix_normalize("a/b/.."), "a");
        assert_eq!(posix_normalize("a/../"), "./");
        assert_eq!(posix_normalize("/a/b/.."), "/a");
        assert_eq!(posix_normalize("/../a"), "/a");
        assert_eq!(posix_normalize("/.."), "/");
        assert_eq!(posix_normalize(".."), "..");
        assert_eq!(posix_normalize("../a"), "../a");
        assert_eq!(posix_normalize("//"), "/");
        assert_eq!(posix_normalize("..."), "...");
        assert_eq!(posix_normalize("/a/..."), "/a/...");
    }
}
