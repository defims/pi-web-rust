//! security 模块 — 路径安全 + 信任 + 会话引用判定 + 请求防护。
//!
//! 对齐 `lib/session-file-references-core.ts` + `lib/session-file-references.ts`
//! + `lib/project-trust.ts` + `lib/request-security.ts`。

pub mod project_trust;
pub mod request_security;
pub mod session_references;

pub use project_trust::{get_project_trust_status, trust_project, ProjectTrustStatus};
pub use request_security::{
    canonical_origin, get_request_origin, has_json_content_type, hostname_from_authority,
    is_api_request_allowed, is_api_request_host_allowed, is_api_request_host_allowed_with,
    is_api_request_origin_allowed, is_ip,
};
pub use session_references::{
    is_bash_output_path_referenced_by_session, is_bash_output_path_referenced_by_session_async,
    is_file_path_referenced_by_session, is_file_path_referenced_by_session_async,
};

/// 对齐 `isValidSessionId`。UUID v4 格式校验。
pub fn is_valid_session_id(session_id: Option<&str>) -> bool {
    let Some(sid) = session_id else {
        return false;
    };
    if sid.len() != 36 {
        return false;
    }
    let bytes = sid.as_bytes();
    bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes[..8].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[9..13].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[14..18].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[19..23].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[24..36].iter().all(|b| b.is_ascii_hexdigit())
}

/// 对齐 TS `decodeURIComponent`:把 `%XX` 解码为 UTF-8 字节序列(支持多字节,
/// 如 `caf%C3%A9` → `café`);任何非法(余下不足两位 / 非十六进制 / 解码后非合法
/// UTF-8)则整串返回原值(等价 try/catch 回退)。`+` 不解码(非 form-encoding)。
fn safe_decode(value: &str) -> String {
    if !value.contains('%') {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // decodeURIComponent:% 后须恰好 2 个十六进制,否则整串非法 → 返回原值
            let Some(hi) = bytes.get(i + 1).copied() else {
                return value.to_string();
            };
            let Some(lo) = bytes.get(i + 2).copied() else {
                return value.to_string();
            };
            match (hex_val(hi), hex_val(lo)) {
                (Some(h), Some(l)) => {
                    out.push((h << 4) | l);
                    i += 3;
                }
                _ => return value.to_string(),
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

/// 单个十六进制字符 → 数值。
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn normalize_slashes(value: &str) -> String {
    value.replace('\\', "/")
}

/// 对齐 TS `/[A-Za-z0-9._~+%@/\\:-]/`。注意含 `:`(影响边界判定)。
fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '.' | '_' | '~' | '+' | '%' | '@' | '/' | '\\' | ':' | '-'
        )
}

fn has_reference_boundary_after(text: &str, byte_index: usize) -> bool {
    if byte_index >= text.len() {
        return true;
    }
    let rest = &text[byte_index..];
    let ch = rest.chars().next().unwrap();
    if ch == ':' {
        return rest[1..]
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false);
    }
    !is_path_char(ch)
}

fn contains_exact_path_reference(text: &str, file_path: &str) -> bool {
    let target = normalize_slashes(file_path);
    let targets: Vec<String> = if target.starts_with('/') {
        vec![target.clone(), format!("file://{target}")]
    } else {
        vec![target.clone()]
    };
    let haystack1 = normalize_slashes(text);
    let haystack2 = normalize_slashes(&safe_decode(text));
    for haystack in [&haystack1, &haystack2] {
        for t in &targets {
            let mut search_from = 0;
            while let Some(idx) = haystack[search_from..].find(t.as_str()) {
                let abs_idx = search_from + idx;
                let before_ok =
                    abs_idx == 0 || !is_path_char(haystack.as_bytes()[abs_idx - 1] as char);
                let after_idx = abs_idx + t.len();
                if before_ok && has_reference_boundary_after(haystack, after_idx) {
                    return true;
                }
                search_from = abs_idx + 1;
                if search_from >= haystack.len() {
                    break;
                }
            }
        }
    }
    false
}

fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_strings(item, out);
            }
        }
        serde_json::Value::Object(obj) => {
            for (_, v) in obj {
                collect_strings(v, out);
            }
        }
        _ => {}
    }
}

pub fn is_file_path_referenced_by_entries(file_path: &str, entries: &[serde_json::Value]) -> bool {
    for entry in entries {
        let mut strings = Vec::new();
        collect_strings(entry, &mut strings);
        if strings
            .iter()
            .any(|text| contains_exact_path_reference(text, file_path))
        {
            return true;
        }
    }
    false
}

pub fn is_bash_output_path_referenced_by_entries(
    file_path: &str,
    entries: &[serde_json::Value],
) -> bool {
    entries.iter().any(|entry| {
        entry.get("type").and_then(|v| v.as_str()) == Some("message")
            && entry
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("bashExecution")
            && entry
                .get("message")
                .and_then(|m| m.get("fullOutputPath"))
                .and_then(|p| p.as_str())
                == Some(file_path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_session_id() {
        assert!(is_valid_session_id(Some(
            "12345678-1234-1234-1234-123456789abc"
        )));
        assert!(!is_valid_session_id(Some("not-a-uuid")));
    }

    #[test]
    fn referenced_by_entries() {
        let entries =
            vec![json!({"type":"message","message":{"content":"edit /home/test/foo.rs"}})];
        assert!(is_file_path_referenced_by_entries(
            "/home/test/foo.rs",
            &entries
        ));
    }

    #[test]
    fn safe_decode_multibyte_utf8() {
        // 对齐 decodeURIComponent:`caf%C3%A9` → `café`(多字节 %XX 序列)
        assert_eq!(safe_decode("caf%C3%A9"), "café");
        // 无 % 原样返回
        assert_eq!(safe_decode("/a/b/c"), "/a/b/c");
        // 非法 % (非十六进制) → 原样返回
        assert_eq!(safe_decode("/a/%ZZ/b"), "/a/%ZZ/b");
        // 截断的 % → 原样返回
        assert_eq!(safe_decode("/a/%4"), "/a/%4");
        // %20 → 空格
        assert_eq!(safe_decode("a%20b"), "a b");
    }

    #[test]
    fn is_path_char_includes_colon() {
        // 对齐 TS char class:`:` 是路径字符(影响边界判定)。
        assert!(is_path_char(':'));
        assert!(is_path_char('/'));
        assert!(is_path_char('-'));
        assert!(is_path_char('.'));
        assert!(!is_path_char(' '));
        assert!(!is_path_char('('));
    }

    #[test]
    fn colon_before_target_blocks_reference() {
        // `:` 紧贴目标前 → 非边界 → 不算引用(对齐 TS isPathChar 含 `:`)。
        let entries =
            vec![json!({"type":"message","message":{"content":"see C:/home/test/foo.rs"}})];
        // "C:/home/test/foo.rs" 中,/home/test/foo.rs 前是 ':'(C:) → 非边界
        assert!(!is_file_path_referenced_by_entries(
            "/home/test/foo.rs",
            &entries
        ));
        // 但作为完整 file:// 或带空格前缀时仍应命中
        let entries2 =
            vec![json!({"type":"message","message":{"content":"edit /home/test/foo.rs done"}})];
        assert!(is_file_path_referenced_by_entries(
            "/home/test/foo.rs",
            &entries2
        ));
    }
}
