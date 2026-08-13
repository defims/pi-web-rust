//! 对齐 `lib/file-types.ts`。
//!
//! 扩展名→MIME / 预览分类。纯查表逻辑,无 IO 依赖。

use serde::{Deserialize, Serialize};

pub const TEXT_PREVIEW_MAX_BYTES: usize = 256 * 1024;
pub const IMAGE_PREVIEW_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const DOCX_PREVIEW_MAX_BYTES: usize = 10 * 1024 * 1024;

/// 对齐 `DocumentPreviewKind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentPreviewKind {
    Pdf,
    Docx,
}

fn image_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

fn audio_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" | "oga" | "opus" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        "aac" => Some("audio/aac"),
        "flac" => Some("audio/flac"),
        "weba" | "webm" => Some("audio/webm"),
        _ => None,
    }
}

fn document_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "pdf" => Some("application/pdf"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        _ => None,
    }
}

fn get_base_name(file_path: &str) -> String {
    file_path
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// 对齐 `getFileExt`。
pub fn get_file_ext(file_path: &str) -> String {
    get_base_name(file_path)
        .to_lowercase()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_string()
}

/// 对齐 `getImageMime`。
pub fn get_image_mime(file_path: &str) -> Option<&'static str> {
    image_mime(&get_file_ext(file_path))
}

/// 对齐 `getAudioMime`。
pub fn get_audio_mime(file_path: &str) -> Option<&'static str> {
    audio_mime(&get_file_ext(file_path))
}

/// 对齐 `getDocumentMime`。
pub fn get_document_mime(file_path: &str) -> Option<&'static str> {
    document_mime(&get_file_ext(file_path))
}

/// 对齐 `documentPreviewKind`。
pub fn document_preview_kind(file_path: &str) -> Option<DocumentPreviewKind> {
    match get_file_ext(file_path).as_str() {
        "pdf" => Some(DocumentPreviewKind::Pdf),
        "docx" => Some(DocumentPreviewKind::Docx),
        _ => None,
    }
}

/// 对齐 `isImagePath`。
pub fn is_image_path(file_path: &str) -> bool {
    get_image_mime(file_path).is_some()
}

/// 对齐 `isAudioPath`。
pub fn is_audio_path(file_path: &str) -> bool {
    get_audio_mime(file_path).is_some()
}

/// 对齐 `isDocumentPreviewPath`。
pub fn is_document_preview_path(file_path: &str) -> bool {
    document_preview_kind(file_path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_ext() {
        assert_eq!(get_file_ext("/a/b/c.RS"), "rs");
        assert_eq!(get_file_ext("noext"), "noext");
    }

    #[test]
    fn image_mime_lookup() {
        assert_eq!(get_image_mime("test.png"), Some("image/png"));
        assert_eq!(get_image_mime("test.JPG"), Some("image/jpeg"));
        assert_eq!(get_image_mime("test.txt"), None);
    }

    #[test]
    fn document_kind() {
        assert_eq!(
            document_preview_kind("doc.pdf"),
            Some(DocumentPreviewKind::Pdf)
        );
        assert_eq!(
            document_preview_kind("doc.docx"),
            Some(DocumentPreviewKind::Docx)
        );
        assert_eq!(document_preview_kind("doc.txt"), None);
    }
}
