//! auth 模块 — 对齐 agegr/pi-web `lib/web-auth.ts`(HTTP Basic 鉴权)。
//!
//! timing-safe SHA256 比较,base64 解码 Basic Auth header。

use sha2::{Digest, Sha256};

pub const PI_WEB_AUTH_USERNAME: &str = "pi";

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
    if !is_web_password_enabled(password) || authorization.is_none() {
        return false;
    }
    let auth = authorization.unwrap();
    let token = match auth.strip_prefix("Basic ").or_else(|| auth.strip_prefix("basic ")) {
        Some(t) => t.trim(),
        None => return false,
    };

    // base64 解码
    let decoded = match base64_decode(token) {
        Some(d) => d,
        None => return false,
    };
    let credentials = match String::from_utf8(decoded) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let separator = credentials.find(':');
    let Some(idx) = separator else { return false };

    let username = &credentials[..idx];
    let supplied_password = &credentials[idx + 1..];
    let password = password.unwrap();

    secrets_equal(username, PI_WEB_AUTH_USERNAME)
        && secrets_equal(supplied_password, password)
}

/// 简单 base64 解码(标准字母表,带 padding)。
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let lookup = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let input = input.trim();
    if !input.len().is_multiple_of(4) {
        return None;
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut i = 0;
    while i < bytes.len() {
        let chunk = &bytes[i..i + 4];
        let vals: Vec<u8> = chunk.iter().map(|&b| lookup(b).unwrap_or(0)).collect();
        let padding = chunk.iter().filter(|&&b| b == b'=').count();
        out.push((vals[0] << 2) | (vals[1] >> 4));
        if padding < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if padding < 1 {
            out.push((vals[2] << 6) | vals[3]);
        }
        i += 4;
    }
    Some(out)
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
        assert!(!is_valid_basic_authorization(Some("Bearer xyz"), Some("secret")));
        assert!(!is_valid_basic_authorization(Some("Basic cGk6d3Jvbmc="), Some("secret")));
    }
}
