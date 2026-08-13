//! compaction 模块 — 对齐 agegr/pi-web `lib/compaction-summary.ts`。

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// 对齐 `ParsedCompactionSummary`。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCompactionSummary {
    pub body: String,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// 对齐 `TRAILING_FILE_SECTIONS_RE`:匹配尾部的 `<read-files>`/`<modified-files>` 块。
static TRAILING_FILE_SECTIONS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:\r?\n){2,}((?:[ \t]*<(?:read-files|modified-files)>[ \t]*\r?\n[\s\S]*?\r?\n[ \t]*</(?:read-files|modified-files)>[ \t]*(?:\r?\n)?)+)\s*$",
    )
    .expect("valid trailing file sections regex")
});

/// 对齐 `FILE_SECTION_RE`:Rust regex 不支持反向引用 \1,拆为两个 pattern。
static READ_FILES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<read-files>\s*([\s\S]*?)\s*</read-files>").expect("valid read-files regex")
});

static MODIFIED_FILES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<modified-files>\s*([\s\S]*?)\s*</modified-files>")
        .expect("valid modified-files regex")
});

/// 对齐 `parseCompactionSummary`。解析压缩摘要文本,提取 read-files/modified-files 标签。
pub fn parse_compaction_summary(summary: &str) -> ParsedCompactionSummary {
    let mut read_files: Vec<String> = Vec::new();
    let mut modified_files: Vec<String> = Vec::new();

    let metadata_match = TRAILING_FILE_SECTIONS_RE.captures(summary);
    let metadata_block = metadata_match
        .as_ref()
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("");
    let body = match metadata_match.as_ref().and_then(|c| c.get(0)) {
        Some(m) => summary[..m.start()].trim().to_string(),
        None => summary.trim().to_string(),
    };

    for caps in READ_FILES_RE.captures_iter(metadata_block) {
        let content = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let files: Vec<String> = content
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        read_files.extend(files);
    }

    for caps in MODIFIED_FILES_RE.captures_iter(metadata_block) {
        let content = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let files: Vec<String> = content
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        modified_files.extend(files);
    }

    ParsedCompactionSummary {
        body,
        read_files,
        modified_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_parse() {
        let result = parse_compaction_summary("Summary text");
        assert_eq!(result.body, "Summary text");
        assert!(result.read_files.is_empty());
    }

    #[test]
    fn parse_with_file_sections() {
        let summary = "Conversation about fixing a bug.\n\n<read-files>\nsrc/main.rs\nsrc/lib.rs\n</read-files>\n\n<modified-files>\nsrc/main.rs\n</modified-files>";
        let result = parse_compaction_summary(summary);
        assert_eq!(result.body, "Conversation about fixing a bug.");
        assert_eq!(result.read_files, vec!["src/main.rs", "src/lib.rs"]);
        assert_eq!(result.modified_files, vec!["src/main.rs"]);
    }

    #[test]
    fn parse_empty_sections() {
        let summary = "Body text.\n\n<read-files>\n\n</read-files>";
        let result = parse_compaction_summary(summary);
        assert_eq!(result.body, "Body text.");
        assert!(result.read_files.is_empty());
    }

    #[test]
    fn no_sections_returns_full_body() {
        let result = parse_compaction_summary("Just a summary\nwith newlines\nbut no tags");
        assert_eq!(result.body, "Just a summary\nwith newlines\nbut no tags");
        assert!(result.read_files.is_empty());
        assert!(result.modified_files.is_empty());
    }
}
