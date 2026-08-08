//! image 模块 — 对齐 agegr/pi-web `lib/image-attachments.ts`。
//!
//! base64 图片附件校验。纯逻辑,无 IO 依赖。

pub const MAX_ATTACHED_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ATTACHED_IMAGES: usize = 10;

/// 对齐 `Base64ImageAttachment`。
#[derive(Debug, Clone)]
pub struct Base64ImageAttachment {
    pub data: String,
    pub mime_type: String,
}

/// 对齐 `isBase64DataChar`。
fn is_base64_data_char(code: u8) -> bool {
    code.is_ascii_alphanumeric() || code == b'+' || code == b'/'
}

/// 对齐 `getBase64DecodedByteLength`。返回解码后字节数;非法 base64 返回 None。
pub fn get_base64_decoded_byte_length(data: &str) -> Option<usize> {
    if data.is_empty() || data.len() % 4 != 0 {
        return None;
    }
    let padding = if data.ends_with("==") {
        2
    } else if data.ends_with('=') {
        1
    } else {
        0
    };
    let data_end = data.len() - padding;
    for byte in &data.as_bytes()[..data_end] {
        if !is_base64_data_char(*byte) {
            return None;
        }
    }
    for byte in &data.as_bytes()[data_end..] {
        if *byte != b'=' {
            return None;
        }
    }
    Some((data.len() / 4) * 3 - padding)
}

/// 对齐 `isBase64ImageWithinLimits`。宽松版:接受 (data, mime_type) 直接参数。
pub fn is_base64_image_within_limits(data: &str, mime_type: &str) -> bool {
    if !mime_type.starts_with("image/") {
        return false;
    }
    get_base64_decoded_byte_length(data)
        .map(|bytes| bytes <= MAX_ATTACHED_IMAGE_BYTES)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_length() {
        // "Hello" → SGVsbG8= (8 chars, 1 padding)
        assert_eq!(get_base64_decoded_byte_length("SGVsbG8="), Some(5));
        // "Hi" → SGk= (4 chars, 1 padding)
        assert_eq!(get_base64_decoded_byte_length("SGk="), Some(2));
        assert_eq!(get_base64_decoded_byte_length("invalid!!"), None);
        assert_eq!(get_base64_decoded_byte_length(""), None);
    }

    #[test]
    fn within_limits() {
        assert!(is_base64_image_within_limits("SGVsbG8=", "image/png"));
        assert!(!is_base64_image_within_limits("SGVsbG8=", "text/plain"));
    }
}
