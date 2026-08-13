//! image 模块 — 对齐 agegr/pi-web `lib/image-attachments.ts`。
//!
//! base64 图片附件校验。纯逻辑,无 IO 依赖。

use serde_json::Value;

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
    if data.is_empty() || !data.len().is_multiple_of(4) {
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

/// 对齐 `isBase64ImageWithinLimits(value)`(整对象版):自行做对象/data/mimeType 守卫。
pub fn is_base64_image_within_limits_value(image: &Value) -> bool {
    let Some(obj) = image.as_object() else {
        return false;
    };
    let Some(data) = obj.get("data").and_then(Value::as_str) else {
        return false;
    };
    let Some(mime_type) = obj.get("mimeType").and_then(Value::as_str) else {
        return false;
    };
    if !mime_type.starts_with("image/") {
        return false;
    }
    get_base64_decoded_byte_length(data)
        .map(|bytes| bytes <= MAX_ATTACHED_IMAGE_BYTES)
        .unwrap_or(false)
}

/// 对齐 `validateAgentImages(value)`。prompt / steering / follow-up 图片数组的 API 错误。
///
/// 返回错误文案(对齐 TS 字符串),合法返回 `None`。`value` 为 `None`(undefined)视为未提供 → 合法。
pub fn validate_agent_images(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    let Some(arr) = value.as_array() else {
        return Some("images must be an array".to_string());
    };
    if arr.len() > MAX_ATTACHED_IMAGES {
        return Some(format!(
            "A message can include at most {MAX_ATTACHED_IMAGES} images"
        ));
    }
    for image in arr {
        // 对齐 `!image || typeof image !== "object" || image.type !== "image"`
        let type_is_image = image
            .as_object()
            .is_some_and(|o| o.get("type").and_then(Value::as_str) == Some("image"));
        if image.is_null() || !image.is_object() || !type_is_image {
            return Some("Each attachment must be an image".to_string());
        }
        if !is_base64_image_within_limits_value(image) {
            return Some(format!(
                "Each image must be valid base64 image data of {}MB or smaller",
                MAX_ATTACHED_IMAGE_BYTES / (1024 * 1024)
            ));
        }
    }
    None
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

    #[test]
    fn validate_agent_images_rules() {
        use serde_json::json;
        // undefined → 合法
        assert_eq!(validate_agent_images(None), None);
        // 非数组
        assert_eq!(
            validate_agent_images(Some(&json!("x"))),
            Some("images must be an array".to_string())
        );
        // 合法空数组 / 单张图片
        assert_eq!(validate_agent_images(Some(&json!([]))), None);
        let ok = json!([{ "type": "image", "data": "SGVsbG8=", "mimeType": "image/png" }]);
        assert_eq!(validate_agent_images(Some(&ok)), None);
        // 非 image 类型
        let bad_type = json!([{ "type": "text", "data": "SGVsbG8=", "mimeType": "image/png" }]);
        assert_eq!(
            validate_agent_images(Some(&bad_type)),
            Some("Each attachment must be an image".to_string())
        );
        // 超过上限(11 > 10)
        let too_many = json!((0..11)
            .map(|_| { json!({ "type": "image", "data": "SGVsbG8=", "mimeType": "image/png" }) })
            .collect::<Vec<_>>());
        assert_eq!(
            validate_agent_images(Some(&too_many)).as_deref(),
            Some("A message can include at most 10 images")
        );
        // 非 image/ mime
        let bad_mime = json!([{ "type": "image", "data": "SGVsbG8=", "mimeType": "text/plain" }]);
        assert!(validate_agent_images(Some(&bad_mime))
            .as_deref()
            .unwrap_or("")
            .contains("valid base64 image data"));
    }
}
