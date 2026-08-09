//! file 模块 — 对齐 agegr/pi-web `lib/file-paths.ts` + `lib/file-types.ts`
//! + `lib/file-fuzzy.ts` 等。

pub mod fuzzy;
pub mod paths;
pub mod types;

pub use fuzzy::{
    AtInsertion, AtQueryMatch, FileIndexEntry, build_at_insert_text, build_at_mention_text,
    build_entries_from_files, build_file_at_mentions_text, build_file_line_mention_text,
    extract_at_query, filter_file_entries, locale_compare_default,
};
pub use paths::{
    normalize_file_path_slashes, encode_file_path_for_api, get_file_name,
    get_file_directory, get_relative_file_path, join_file_path,
};
pub use types::{
    DocumentPreviewKind, TEXT_PREVIEW_MAX_BYTES, IMAGE_PREVIEW_MAX_BYTES, DOCX_PREVIEW_MAX_BYTES,
    get_file_ext, get_image_mime, get_audio_mime, get_document_mime,
    document_preview_kind, is_image_path, is_audio_path, is_document_preview_path,
};
