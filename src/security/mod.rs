//! security 模块 — 路径安全 + 信任 + 会话引用判定 + 请求防护。
//!
//! 对齐 `lib/session-file-references-core.ts` + `lib/project-trust.ts`
//! + `lib/request-security.ts`。

pub mod project_trust;
pub mod request_security;

pub use project_trust::{ProjectTrustStatus, get_project_trust_status, trust_project};
pub use request_security::{
    canonical_origin, get_request_origin, has_json_content_type, hostname_from_authority,
    is_api_request_allowed, is_api_request_host_allowed, is_api_request_host_allowed_with,
    is_api_request_origin_allowed, is_ip,
};

/// 对齐 `isValidSessionId`。UUID v4 格式校验。
pub fn is_valid_session_id(session_id: Option<&str>) -> bool {
    let Some(sid) = session_id else { return false; };
    if sid.len() != 36 {
        return false;
    }
    let bytes = sid.as_bytes();
    bytes[8] == b'-' && bytes[13] == b'-' && bytes[18] == b'-' && bytes[23] == b'-'
        && bytes[..8].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[9..13].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[14..18].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[19..23].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[24..36].iter().all(|b| b.is_ascii_hexdigit())
}

fn safe_decode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(s) = std::str::from_utf8(&[u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00"),
                16,
            ).unwrap_or(0)]) {
                out.push_str(s);
            }
            i += 3;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn normalize_slashes(value: &str) -> String { value.replace('\\', "/") }

fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '+' | '%' | '@' | '/' | '\\' | '-')
}

fn has_reference_boundary_after(text: &str, byte_index: usize) -> bool {
    if byte_index >= text.len() { return true; }
    let rest = &text[byte_index..];
    let ch = rest.chars().next().unwrap();
    if ch == ':' {
        return rest[1..].chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
    }
    !is_path_char(ch)
}

fn contains_exact_path_reference(text: &str, file_path: &str) -> bool {
    let target = normalize_slashes(file_path);
    let targets: Vec<String> = if target.starts_with('/') {
        vec![target.clone(), format!("file://{target}")]
    } else { vec![target.clone()] };
    let haystack1 = normalize_slashes(text);
    let haystack2 = normalize_slashes(&safe_decode(text));
    for haystack in [&haystack1, &haystack2] {
        for t in &targets {
            let mut search_from = 0;
            while let Some(idx) = haystack[search_from..].find(t.as_str()) {
                let abs_idx = search_from + idx;
                let before_ok = abs_idx == 0 || !is_path_char(haystack.as_bytes()[abs_idx - 1] as char);
                let after_idx = abs_idx + t.len();
                if before_ok && has_reference_boundary_after(haystack, after_idx) { return true; }
                search_from = abs_idx + 1;
                if search_from >= haystack.len() { break; }
            }
        }
    }
    false
}

fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(arr) => { for item in arr { collect_strings(item, out); } }
        serde_json::Value::Object(obj) => { for (_, v) in obj { collect_strings(v, out); } }
        _ => {}
    }
}

pub fn is_file_path_referenced_by_entries(file_path: &str, entries: &[serde_json::Value]) -> bool {
    for entry in entries {
        let mut strings = Vec::new();
        collect_strings(entry, &mut strings);
        if strings.iter().any(|text| contains_exact_path_reference(text, file_path)) { return true; }
    }
    false
}

pub fn is_bash_output_path_referenced_by_entries(file_path: &str, entries: &[serde_json::Value]) -> bool {
    entries.iter().any(|entry| {
        entry.get("type").and_then(|v| v.as_str()) == Some("message")
            && entry.get("message").and_then(|m| m.get("role")).and_then(|r| r.as_str()) == Some("bashExecution")
            && entry.get("message").and_then(|m| m.get("fullOutputPath")).and_then(|p| p.as_str()) == Some(file_path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_session_id() {
        assert!(is_valid_session_id(Some("12345678-1234-1234-1234-123456789abc")));
        assert!(!is_valid_session_id(Some("not-a-uuid")));
    }

    #[test]
    fn referenced_by_entries() {
        let entries = vec![json!({"type":"message","message":{"content":"edit /home/test/foo.rs"}})];
        assert!(is_file_path_referenced_by_entries("/home/test/foo.rs", &entries));
    }
}
