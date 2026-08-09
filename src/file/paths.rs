//! 对齐 `lib/file-paths.ts`。
//!
//! 跨平台路径字符串工具(客户端安全,无 fs 依赖)。

/// 对齐 `normalizeFilePathSlashes`。Windows 路径(`C:\` 或 `\\`)反斜杠转正斜杠。
pub fn normalize_file_path_slashes(file_path: &str) -> String {
    // /^[a-zA-Z]:[\\/]/ 或 startsWith("\\")
    let is_windows = (file_path.len() >= 3
        && file_path.as_bytes()[0].is_ascii_alphabetic()
        && file_path.as_bytes()[1] == b':'
        && (file_path.as_bytes()[2] == b'\\' || file_path.as_bytes()[2] == b'/'))
        || file_path.starts_with("\\\\");
    if is_windows {
        file_path.replace('\\', "/")
    } else {
        file_path.to_string()
    }
}

/// 对齐 `encodeFilePathForApi`。路径分段 encodeURIComponent 后用 / 拼接。
pub fn encode_file_path_for_api(file_path: &str) -> String {
    normalize_file_path_slashes(file_path)
        .split('/')
        .filter(|s| !s.is_empty())
        .map(url_encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// 对齐 `getFileName`。取路径最后一段。
pub fn get_file_name(file_path: &str) -> String {
    let normalized = normalize_file_path_slashes(file_path);
    let trimmed = normalized.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// 对齐 `getFileDirectory`。取路径除最后一段外的部分。
pub fn get_file_directory(file_path: &str) -> String {
    let normalized = normalize_file_path_slashes(file_path);
    let trimmed = normalized.trim_end_matches('/');
    match trimmed.rfind('/') {
        None => String::new(),
        Some(0) => "/".to_string(),
        Some(2) if trimmed.len() >= 3
            && trimmed.as_bytes()[1] == b':'
            && trimmed.as_bytes()[2] == b'/' =>
        {
            trimmed[..3].to_string()
        }
        Some(idx) => trimmed[..idx].to_string(),
    }
}

/// 对齐 `getRelativeFilePath`。从 cwd 下取相对路径。
pub fn get_relative_file_path(file_path: &str, cwd: Option<&str>) -> String {
    let Some(cwd) = cwd else {
        return file_path.to_string();
    };
    let normalized_file = normalize_file_path_slashes(file_path);
    let normalized_cwd = normalize_file_path_slashes(cwd).trim_end_matches('/').to_string();
    if let Some(rest) = normalized_file.strip_prefix(&format!("{normalized_cwd}/")) {
        rest.to_string()
    } else {
        file_path.to_string()
    }
}

/// 对齐 `joinFilePath`。
pub fn join_file_path(parent: &str, child: &str) -> String {
    format!("{}/{}", normalize_file_path_slashes(parent).trim_end_matches('/'), child)
}

/// encodeURIComponent 等价(Rust 没有 builtin,手写 RFC 3986 unreserved 之外的转义)。
pub(crate) fn url_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_backslashes() {
        assert_eq!(normalize_file_path_slashes(r"C:\Users\test"), "C:/Users/test");
        assert_eq!(normalize_file_path_slashes("/unix/path"), "/unix/path");
    }

    #[test]
    fn file_name() {
        assert_eq!(get_file_name("/a/b/c.rs"), "c.rs");
        assert_eq!(get_file_name("trailing/"), "trailing");
    }

    #[test]
    fn file_directory() {
        assert_eq!(get_file_directory("/a/b/c.rs"), "/a/b");
        assert_eq!(get_file_directory("c.rs"), "");
        assert_eq!(get_file_directory("/root"), "/");
    }

    #[test]
    fn relative_path() {
        assert_eq!(get_relative_file_path("/a/b/c.rs", Some("/a/b")), "c.rs");
        assert_eq!(get_relative_file_path("/a/b/c.rs", None), "/a/b/c.rs");
    }

    #[test]
    fn join_path() {
        assert_eq!(join_file_path("/a/b", "c.rs"), "/a/b/c.rs");
        assert_eq!(join_file_path("/a/b/", "c.rs"), "/a/b/c.rs");
    }
}
