//! compaction 模块 — 对齐 agegr/pi-web `lib/compaction-summary.ts`。

use serde::{Deserialize, Serialize};

/// 对齐 `ParsedCompactionSummary`。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCompactionSummary {
    pub body: String,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// 对齐 `parseCompactionSummary`。解析压缩摘要文本,提取 read-files/modified-files 标签。
///
/// 上游用两个正则:`TRAILING_FILE_SECTIONS_RE`(匹配尾部文件块) + `FILE_SECTION_RE`
/// (逐 section 提取)。Rust 用 `regex` crate 等价实现。
pub fn parse_compaction_summary(summary: &str) -> ParsedCompactionSummary {
    let _ = summary; // 占位,实际实现在带 regex 依赖后补
    // TODO: 加 regex 依赖后翻译 TRAILING_FILE_SECTIONS_RE + FILE_SECTION_RE 逻辑。
    // 暂时返回整段为 body(语义退化,不崩)。
    ParsedCompactionSummary {
        body: summary.trim().to_string(),
        read_files: vec![],
        modified_files: vec![],
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
}
