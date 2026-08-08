//! security 模块 — 对齐 agegr/pi-web `lib/session-file-references-core.ts`。
//!
//! 判断文件路径是否被会话 entry 引用(纯逻辑,无 fs)。用于 IPC 访问控制的
//! sessionId 旁路:若文件被会话内容引用,即使不在白名单也放行。
//!
//! 用 `serde_json::Value` 泛化处理 entry(避免引全套 types.ts 类型)。

/// 对齐 `isValidSessionId`。UUID v4 格式校验。
pub fn is_valid_session_id(session_id: Option<&str>) -> bool {
    let Some(sid) = session_id else { return false; };
    if sid.len() != 36 {
        return false;
    }
    let bytes = sid.as_bytes();
    // 8-4-4-4-12 hex,带连字符
    bytes[8] == b'-' && bytes[13] == b'-' && bytes[18] == b'-' && bytes[23] == b'-'
        && bytes[..8].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[9..13].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[14..18].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[19..23].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[24..36].iter().all(|b| b.is_ascii_hexdigit())
}

fn safe_decode(value: &str) -> String {
    // 简化:percent-decoding(对齐 decodeURIComponent,失败返回原值)
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(s) = std::str::from_utf8(&[u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00"),
                16,
            )
            .unwrap_or(0)]) {
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

fn normalize_slashes(value: &str) -> String {
    value.replace('\\', "/")
}

fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '.' | '_' | '~' | '+' | '%' | '@' | '/' | '\\' | '-'
        )
}

fn has_reference_boundary_after(text: &str, byte_index: usize) -> bool {
    if byte_index >= text.len() {
        return true;
    }
    let rest = &text[byte_index..];
    let ch = rest.chars().next().unwrap();
    if ch == ':' {
        // :后面跟数字不算边界(如 C:8080)
        let after_colon = rest[1..].chars().next();
        return after_colon.map(|c| c.is_ascii_digit()).unwrap_or(false);
    }
    !is_path_char(ch)
}

/// 对齐 `containsExactPathReference`。
fn contains_exact_path_reference(text: &str, file_path: &str) -> bool {
    let target = normalize_slashes(file_path);
    let targets: Vec<&str> = if target.starts_with('/') {
        vec![target.as_str(), "file://"]
    } else {
        vec![target.as_str()]
    };
    let targets: Vec<String> = if target.starts_with('/') {
        vec![target.clone(), format!("file://{target}")]
    } else {
        vec![target.clone()]
    };

    let haystack1 = normalize_slashes(text);
    let haystack2 = normalize_slashes(&safe_decode(text));
    let haystacks = [haystack1.as_str(), haystack2.as_str()];

    for haystack in &haystacks {
        for t in &targets {
            let mut search_from = 0;
            while let Some(idx) = haystack[search_from..].find(t.as_str()) {
                let abs_idx = search_from + idx;
                let before_ok = abs_idx == 0
                    || !is_path_char(haystack.as_bytes()[abs_idx - 1] as char);
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

/// 对齐 `collectStrings`。递归收集 JSON 值里的所有字符串。
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

/// 对齐 `isFilePathReferencedByEntries`。entries 是 JSON 数组(session entries)。
pub fn is_file_path_referenced_by_entries(file_path: &str, entries: &[serde_json::Value]) -> bool {
    for entry in entries {
        let mut strings = Vec::new();
        collect_strings(entry, &mut strings);
        if strings.iter().any(|text| contains_exact_path_reference(text, file_path)) {
            return true;
        }
    }
    false
}

/// 对齐 `isBashOutputPathReferencedByEntries`。检查 bashExecution entry 的 fullOutputPath。
pub fn is_bash_output_path_referenced_by_entries(file_path: &str, entries: &[serde_json::Value]) -> bool {
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
        assert!(is_valid_session_id(Some("12345678-1234-1234-1234-123456789abc")));
        assert!(!is_valid_session_id(Some("not-a-uuid")));
        assert!(!is_valid_session_id(None));
    }

    #[test]
    fn exact_path_reference() {
        assert!(contains_exact_path_reference("/a/b/main.rs", "/a/b/main.rs"));
        assert!(!contains_exact_path_reference("/a/b/main", "/a/b/main.rs"));
    }

    #[test]
    fn referenced_by_entries() {
        let entries = vec![json!({
            "type": "message",
            "message": { "content": "edit /home/test/foo.rs" }
        })];
        assert!(is_file_path_referenced_by_entries("/home/test/foo.rs", &entries));
    }

    #[test]
    fn bash_output_reference() {
        let entries = vec![json!({
            "type": "message",
            "message": { "role": "bashExecution", "fullOutputPath": "/tmp/pi-bash-abc.log" }
        })];
        assert!(is_bash_output_path_referenced_by_entries("/tmp/pi-bash-abc.log", &entries));
        assert!(!is_bash_output_path_referenced_by_entries("/tmp/other.log", &entries));
    }
}
