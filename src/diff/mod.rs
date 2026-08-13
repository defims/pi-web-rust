//! diff 模块 — 对齐 agegr/pi-web `lib/patch.ts`。
//!
//! unified diff 解析为分屏视图模型。

use serde::{Deserialize, Serialize};

/// 对齐 `SplitDiffCellType`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDiffCellType {
    Context,
    Removed,
    Added,
    Empty,
}

/// 对齐 `SplitDiffCell`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitDiffCell {
    pub line_no: Option<i64>,
    pub text: String,
    pub r#type: SplitDiffCellType,
}

/// 对齐 `SplitDiffRow`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SplitDiffRow {
    Hunk {
        text: String,
    },
    Line {
        left: SplitDiffCell,
        right: SplitDiffCell,
    },
}

/// 对齐 `SplitDiffFile`。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SplitDiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub rows: Vec<SplitDiffRow>,
}

struct PendingChangeLine {
    line_no: i64,
    text: String,
}

/// 对齐 `parseUnifiedPatch`。解析 unified diff 为分屏文件列表。
pub fn parse_unified_patch(text: &str) -> Option<Vec<SplitDiffFile>> {
    let mut files: Vec<SplitDiffFile> = Vec::new();
    let mut current_idx: Option<usize> = None;
    let mut pending_old_path: Option<String> = None;
    let mut old_line_no: i64 = 0;
    let mut new_line_no: i64 = 0;
    let mut hunk_old_remaining: i64 = 0;
    let mut hunk_new_remaining: i64 = 0;
    let mut removed: Vec<PendingChangeLine> = Vec::new();
    let mut added: Vec<PendingChangeLine> = Vec::new();

    let empty_cell = || SplitDiffCell {
        line_no: None,
        text: String::new(),
        r#type: SplitDiffCellType::Empty,
    };

    let flush_changes = |files: &mut Vec<SplitDiffFile>,
                         current_idx: &mut Option<usize>,
                         removed: &mut Vec<PendingChangeLine>,
                         added: &mut Vec<PendingChangeLine>| {
        let Some(idx) = *current_idx else {
            removed.clear();
            added.clear();
            return;
        };
        let count = removed.len().max(added.len());
        for i in 0..count {
            let left = if i < removed.len() {
                SplitDiffCell {
                    line_no: Some(removed[i].line_no),
                    text: removed[i].text.clone(),
                    r#type: SplitDiffCellType::Removed,
                }
            } else {
                empty_cell()
            };
            let right = if i < added.len() {
                SplitDiffCell {
                    line_no: Some(added[i].line_no),
                    text: added[i].text.clone(),
                    r#type: SplitDiffCellType::Added,
                }
            } else {
                empty_cell()
            };
            files[idx].rows.push(SplitDiffRow::Line { left, right });
        }
        removed.clear();
        added.clear();
    };

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        let inside_hunk = hunk_old_remaining > 0 || hunk_new_remaining > 0;

        if !inside_hunk {
            if let Some(rest) = line.strip_prefix("--- ") {
                flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);
                pending_old_path = Some(clean_patch_path(rest));
                continue;
            }
            if let Some(rest) = line.strip_prefix("+++ ") {
                flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);
                files.push(SplitDiffFile {
                    old_path: pending_old_path.take(),
                    new_path: Some(clean_patch_path(rest)),
                    rows: vec![],
                });
                current_idx = Some(files.len() - 1);
                continue;
            }
        }

        // @@ header
        if let Some(hunk) = parse_hunk_header(line) {
            if current_idx.is_none() {
                files.push(SplitDiffFile::default());
                current_idx = Some(files.len() - 1);
            }
            flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);
            old_line_no = hunk.old_start;
            new_line_no = hunk.new_start;
            hunk_old_remaining = hunk.old_count;
            hunk_new_remaining = hunk.new_count;
            if let Some(idx) = current_idx {
                files[idx].rows.push(SplitDiffRow::Hunk {
                    text: line.to_string(),
                });
            }
            continue;
        }

        if current_idx.is_none() {
            continue;
        }

        if line.starts_with("\\ ") {
            flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);
            if let Some(idx) = current_idx {
                files[idx].rows.push(SplitDiffRow::Hunk {
                    text: line.to_string(),
                });
            }
            continue;
        }

        let prefix = line.as_bytes().first().copied();
        let content = &line[1.min(line.len())..];

        match prefix {
            Some(b' ') => {
                flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);
                if let Some(idx) = current_idx {
                    files[idx].rows.push(SplitDiffRow::Line {
                        left: SplitDiffCell {
                            line_no: Some(old_line_no),
                            text: content.to_string(),
                            r#type: SplitDiffCellType::Context,
                        },
                        right: SplitDiffCell {
                            line_no: Some(new_line_no),
                            text: content.to_string(),
                            r#type: SplitDiffCellType::Context,
                        },
                    });
                }
                old_line_no += 1;
                new_line_no += 1;
                if hunk_old_remaining > 0 {
                    hunk_old_remaining -= 1;
                }
                if hunk_new_remaining > 0 {
                    hunk_new_remaining -= 1;
                }
            }
            Some(b'-') => {
                removed.push(PendingChangeLine {
                    line_no: old_line_no,
                    text: content.to_string(),
                });
                old_line_no += 1;
                if hunk_old_remaining > 0 {
                    hunk_old_remaining -= 1;
                }
            }
            Some(b'+') => {
                added.push(PendingChangeLine {
                    line_no: new_line_no,
                    text: content.to_string(),
                });
                new_line_no += 1;
                if hunk_new_remaining > 0 {
                    hunk_new_remaining -= 1;
                }
            }
            _ if !line.is_empty() => {
                flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);
                if let Some(idx) = current_idx {
                    files[idx].rows.push(SplitDiffRow::Hunk {
                        text: line.to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    flush_changes(&mut files, &mut current_idx, &mut removed, &mut added);

    let parsed: Vec<SplitDiffFile> = files
        .into_iter()
        .filter(|f| {
            f.rows
                .iter()
                .any(|r| matches!(r, SplitDiffRow::Line { .. }))
        })
        .collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

struct HunkHeader {
    old_start: i64,
    old_count: i64,
    new_start: i64,
    new_count: i64,
}

/// 对齐 `/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/`(末尾 `@@` 后可跟上下文,无 `$` 锚定)。
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let line = line.strip_prefix("@@ -")?;
    // old 段到首个空格为止:`"1,2 +1,2 @@ ..."` → old="1,2", 余="+1,2 @@ ..."
    let (old_section, rest) = line.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    // new 段到 " @@" 为止(必须存在尾随 @@)
    let (new_section, _) = rest.split_once(" @@")?;
    let (os, oc) = old_section.split_once(',').unwrap_or((old_section, "1"));
    let (ns, nc) = new_section.split_once(',').unwrap_or((new_section, "1"));
    let old_start: i64 = os.parse().ok()?;
    let old_count: i64 = oc.parse().unwrap_or(1);
    let new_start: i64 = ns.parse().ok()?;
    let new_count: i64 = nc.parse().unwrap_or(1);
    Some(HunkHeader {
        old_start,
        old_count,
        new_start,
        new_count,
    })
}

/// 对齐 `cleanPatchPath`。
fn clean_patch_path(path: &str) -> String {
    path.split('\t').next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_patch() {
        let patch = "--- a/test.rs\n+++ b/test.rs\n@@ -1,2 +1,2 @@\n old line\n-new line\n+new modified\n context\n";
        let result = parse_unified_patch(patch).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].old_path.as_deref(), Some("a/test.rs"));
        assert_eq!(result[0].new_path.as_deref(), Some("b/test.rs"));
        // 至少有 line rows
        assert!(result[0]
            .rows
            .iter()
            .any(|r| matches!(r, SplitDiffRow::Line { .. })));
    }

    #[test]
    fn null_for_empty() {
        assert!(parse_unified_patch("").is_none());
        assert!(parse_unified_patch("not a patch").is_none());
    }
}
