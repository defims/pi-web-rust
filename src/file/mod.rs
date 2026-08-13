//! file 模块 — 对齐 agegr/pi-web `lib/file-paths.ts` + `lib/file-types.ts`
//! + `lib/file-fuzzy.ts` 等。

pub mod fuzzy;
pub mod paths;
pub mod types;

pub use fuzzy::{
    build_at_insert_text, build_at_mention_text, build_entries_from_files,
    build_file_at_mentions_text, build_file_line_mention_text, extract_at_query,
    filter_file_entries, locale_compare_default, AtInsertion, AtQueryMatch, FileIndexEntry,
};
pub use paths::{
    encode_file_path_for_api, get_file_directory, get_file_name, get_relative_file_path,
    join_file_path, normalize_file_path_slashes,
};
pub use types::{
    document_preview_kind, get_audio_mime, get_document_mime, get_file_ext, get_image_mime,
    is_audio_path, is_document_preview_path, is_image_path, DocumentPreviewKind,
    DOCX_PREVIEW_MAX_BYTES, IMAGE_PREVIEW_MAX_BYTES, TEXT_PREVIEW_MAX_BYTES,
};
