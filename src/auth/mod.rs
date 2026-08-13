//! auth 模块 — 对齐 agegr/pi-web `lib/web-auth.ts`(HTTP Basic 鉴权)。
//!
//! timing-safe SHA256 比较,base64 解码 Basic Auth header。

use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const PI_WEB_AUTH_USERNAME: &str = "pi";

/// 对齐 TS `/^Basic\s+(\S+)$/i`:大小写不敏感的 `Basic` + 任意空白 + 非空白 token。
static BASIC_AUTH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^Basic\s+(\S+)$").expect("valid basic auth regex"));

/// SHA256 哈希。返回 32 字节。
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn secrets_equal(actual: &str, expected: &str) -> bool {
    let h1 = sha256(actual.as_bytes());
    let h2 = sha256(expected.as_bytes());
    // 常数时间比较
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= h1[i] ^ h2[i];
    }
    diff == 0
}

/// 对齐 `isWebPasswordEnabled`。
pub fn is_web_password_enabled(password: Option<&str>) -> bool {
    password.map(|p| !p.is_empty()).unwrap_or(false)
}

/// 对齐 `isValidBasicAuthorization`。
pub fn is_valid_basic_authorization(authorization: Option<&str>, password: Option<&str>) -> bool {
    if !is_web_password_enabled(password) {
        return false;
    }
    let Some(auth) = authorization else {
        return false;
    };

    // 对齐 `match = /^Basic\s+(\S+)$/i.exec(authorization)`。
    let token = match BASIC_AUTH_RE.captures(auth) {
        Some(caps) => caps.get(1).map(|m| m.as_str()),
        None => None,
    };
    let Some(token) = token else { return false };

    // base64 解码(严格:非法字符返回 None)。
    let decoded = match base64_decode(token) {
        Some(d) => d,
        None => return false,
    };
    // 对齐 TS `decoded.toString("base64") !== match[1]`:非规范 base64(多/少 padding、
    // 非标准字母表)在此被拒。合法 token 必为规范编码。
    if base64_encode(&decoded) != token {
        return false;
    }
    let credentials = match String::from_utf8(decoded) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let Some(idx) = credentials.find(':') else {
        return false;
    };

    let username = &credentials[..idx];
    let supplied_password = &credentials[idx + 1..];
    let password = password.unwrap();

    secrets_equal(username, PI_WEB_AUTH_USERNAME) && secrets_equal(supplied_password, password)
}

/// base64 字母表值;`=` padding 返回 None(由调用方单独处理)。
fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None, // 含 '=' 与非法字符
    }
}

/// 标准 base64 解码(严格:长度须为 4 倍数、仅合法字符、padding 仅在末尾)。
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim();
    if input.is_empty() || !input.len().is_multiple_of(4) {
        return None;
    }
    let bytes = input.as_bytes();
    // 全部字符必须在字母表或 padding 内(对齐 TS 严格性,非法字符直接拒)。
    if !bytes.iter().all(|&b| b64_val(b).is_some() || b == b'=') {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut i = 0;
    while i < bytes.len() {
        let chunk = &bytes[i..i + 4];
        // padding 字节按 0 取值(仅末段有效,padding 计数控制是否写入)。
        let v = |b: u8| b64_val(b).unwrap_or(0);
        let padding = chunk.iter().filter(|&&b| b == b'=').count();
        // padding 只允许在 chunk 末尾连续出现(对齐 canonical)。
        if padding > 0 {
            let non_pad = 4 - padding;
            if chunk[non_pad..].iter().any(|&b| b != b'=') {
                return None;
            }
        }
        let v0 = v(chunk[0]);
        let v1 = v(chunk[1]);
        let v2 = v(chunk[2]);
        let v3 = v(chunk[3]);
        out.push((v0 << 2) | (v1 >> 4));
        if padding < 2 {
            out.push((v1 << 4) | (v2 >> 2));
        }
        if padding < 1 {
            out.push((v2 << 6) | v3);
        }
        i += 4;
    }
    Some(out)
}

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 base64 编码(规范形式,用于 canonical 校验)。对齐 `Buffer.toString("base64")`。
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(B64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(B64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(B64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_enabled() {
        assert!(!is_web_password_enabled(None));
        assert!(!is_web_password_enabled(Some("")));
        assert!(is_web_password_enabled(Some("secret")));
    }

    #[test]
    fn valid_auth() {
        // Basic base64("pi:secret") = "cGk6c2VjcmV0"
        let result = is_valid_basic_authorization(Some("Basic cGk6c2VjcmV0"), Some("secret"));
        assert!(result);
    }

    #[test]
    fn invalid_auth() {
        assert!(!is_valid_basic_authorization(None, Some("secret")));
        assert!(!is_valid_basic_authorization(
            Some("Bearer xyz"),
            Some("secret")
        ));
        assert!(!is_valid_basic_authorization(
            Some("Basic cGk6d3Jvbmc="),
            Some("secret")
        ));
    }

    #[test]
    fn basic_scheme_case_and_whitespace_insensitive() {
        // 对齐 TS `/^Basic\s+(\S+)$/i`:任意大小写 + 任意空白分隔符。
        assert!(is_valid_basic_authorization(
            Some("Basic cGk6c2VjcmV0"),
            Some("secret")
        ));
        assert!(is_valid_basic_authorization(
            Some("BASIC cGk6c2VjcmV0"),
            Some("secret")
        ));
        assert!(is_valid_basic_authorization(
            Some("basic\tcGk6c2VjcmV0"),
            Some("secret")
        ));
        assert!(is_valid_basic_authorization(
            Some("Basic  cGk6c2VjcmV0"),
            Some("secret")
        ));
    }

    #[test]
    fn rejects_non_canonical_base64() {
        // 对齐 TS canonical 校验:token 须为规范编码。
        // "cGk6c2VjcmV0" 是 "pi:secret" 的规范编码 → 接受。
        assert!(is_valid_basic_authorization(
            Some("Basic cGk6c2VjcmV0"),
            Some("secret")
        ));
        // 多余 padding / 非法字符 → 拒。
        assert!(!is_valid_basic_authorization(
            Some("Basic cGk6c2VjcmV0===="),
            Some("secret")
        ));
        assert!(!is_valid_basic_authorization(
            Some("Basic cGk6c2VjcmV0!"),
            Some("secret")
        ));
    }
}
