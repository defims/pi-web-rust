//! 对齐 `lib/file-fuzzy.ts`。聊天输入 `@` 文件自动补全(纯计算)。
//!
//! 镜像 pi TUI 的行为:`@` 在行首或空白后触发,条目用 TUI 的 scoreEntry
//! 阶梯打分,补全插入 `@relative/path `。语义按 Node 探针逐项对齐;
//! `localeCompare`(默认,无 options)用「大小写折叠比较 + 小写优先平局」
//! 近似 ICU en 排序(路径以 ASCII 为主,非 ASCII 可能微差)。

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// 对齐 TS `/(?:^|\s)@"([^"\n]*)$/`(引号形式)。
static QUOTED_AT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|\s)@"([^"\n]*)$"#).expect("valid quoted @ regex"));

/// 对齐 TS `/(?:^|\s)@([^\s"]*)$/`(普通形式)。
static PLAIN_AT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|\s)@([^\s"]*)$"#).expect("valid plain @ regex"));

/// JS 字符串 `.length`(UTF-16 码元数)。`start`/`cursorOffset` 返回给 JS 前端,
/// 须用 UTF-16 单位而非字节数,否则非 ASCII 文本会错位切片。
fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

/// 对齐 `AtQueryMatch`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtQueryMatch {
    /// `@` 在文本中的索引。
    pub start: usize,
    /// `@` 后输入的文本(引号形式已剥离),可能为空。
    pub query: String,
    /// 是否使用 `@"..."` 引号形式。
    pub quoted: bool,
}

/// 对齐 `FileIndexEntry`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIndexEntry {
    /// 相对 session cwd 的路径,"/" 分隔,无尾部斜杠。
    pub path: String,
    pub is_dir: bool,
}

/// 对齐 `AT_RESULT_LIMIT`。
pub const AT_RESULT_LIMIT: usize = 20;

/// 对齐 `extractAtQuery`。
///
/// `@` 必须在文本开头或空白之后(邮件地址不触发);支持进行中的
/// `@"my dir/fi` 引号形式(可钻入含空格路径)。
pub fn extract_at_query(text_before_cursor: &str) -> Option<AtQueryMatch> {
    // 引号形式 /(?:^|\s)@"([^"\n]*)$/
    if let Some(caps) = QUOTED_AT_RE.captures(text_before_cursor) {
        let query = caps.get(1)?.as_str();
        return Some(AtQueryMatch {
            // 对齐 TS `textBeforeCursor.length - (quoted[1].length + 2)`,以 UTF-16 单位计。
            start: utf16_len(text_before_cursor) - utf16_len(query) - 2,
            query: query.to_string(),
            quoted: true,
        });
    }
    // 普通形式 /(?:^|\s)@([^\s"]*)$/:正则匹配最左「空白/^ 后的 @」,query 可含后续 @。
    if let Some(caps) = PLAIN_AT_RE.captures(text_before_cursor) {
        let query = caps.get(1)?.as_str();
        return Some(AtQueryMatch {
            start: utf16_len(text_before_cursor) - utf16_len(query) - 1,
            query: query.to_string(),
            quoted: false,
        });
    }
    None
}

/// 对齐 `pathDepth`:`/` 出现次数。
fn path_depth(p: &str) -> usize {
    p.bytes().filter(|&b| b == b'/').count()
}

/// 对齐 `buildEntriesFromFiles`。
///
/// 从文件列表推导目录条目(索引 API 只返回文件),目录按出现顺序去重,
/// 排序:浅优先,再字母序。
pub fn build_entries_from_files(files: &[String]) -> Vec<FileIndexEntry> {
    let mut dirs: Vec<String> = Vec::new();
    for f in files {
        let mut idx = f.find('/');
        while let Some(i) = idx {
            let dir = &f[..i];
            if !dirs.iter().any(|d| d == dir) {
                dirs.push(dir.to_string());
            }
            idx = f[i + 1..].find('/').map(|j| i + 1 + j);
        }
    }
    let mut entries: Vec<FileIndexEntry> = dirs
        .into_iter()
        .map(|path| FileIndexEntry { path, is_dir: true })
        .collect();
    for f in files {
        if f.is_empty() {
            continue;
        }
        entries.push(FileIndexEntry {
            path: f.clone(),
            is_dir: false,
        });
    }
    entries.sort_by(|a, b| {
        path_depth(&a.path)
            .cmp(&path_depth(&b.path))
            .then_with(|| locale_compare_default(&a.path, &b.path))
    });
    entries
}

/// 对齐 `isSubsequence`。
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut i = 0;
    for ch in haystack.chars() {
        if i < needle.len() && ch == needle.chars().nth(i).unwrap() {
            i += 1;
        }
    }
    i == needle.len()
}

/// 对齐 `scoreEntry`(TUI 阶梯:精确 100 / 前缀 80 / 子串 50 / 路径子串 30,
/// 目录 +10;子序列兜底 10)。
fn score_entry(entry: &FileIndexEntry, lower_query: &str) -> i32 {
    let lower_path = entry.path.to_lowercase();
    let mut score = 0;
    if lower_query.contains('/') {
        if lower_path == lower_query {
            score = 100;
        } else if lower_path.starts_with(lower_query) {
            score = 80;
        } else if lower_path.contains(lower_query) {
            score = 50;
        } else if is_subsequence(lower_query, &lower_path) {
            score = 10;
        }
    } else {
        let slash = lower_path.rfind('/');
        let lower_name = match slash {
            Some(s) => &lower_path[s + 1..],
            None => lower_path.as_str(),
        };
        if lower_name == lower_query {
            score = 100;
        } else if lower_name.starts_with(lower_query) {
            score = 80;
        } else if lower_name.contains(lower_query) {
            score = 50;
        } else if lower_path.contains(lower_query) {
            score = 30;
        } else if is_subsequence(lower_query, &lower_path) {
            score = 10;
        }
    }
    if entry.is_dir && score > 0 {
        score += 10;
    }
    score
}

/// 对齐 `localeCompare` 默认行为(无 options)的近似:大小写折叠比较,
/// 折叠相等时小写优先于大写(ICU en 排序),其余按字节。
pub fn locale_compare_default(a: &str, b: &str) -> std::cmp::Ordering {
    let la = a.to_lowercase();
    let lb = b.to_lowercase();
    la.cmp(&lb).then_with(|| case_sensitive_cmp(a, b))
}

fn case_sensitive_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let len = ab.len().min(bb.len());
    for i in 0..len {
        let x = ab[i];
        let y = bb[i];
        let ord = match (x.is_ascii_lowercase(), y.is_ascii_lowercase()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => x.cmp(&y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ab.len().cmp(&bb.len())
}

/// 对齐 `filterFileEntries`。
pub fn filter_file_entries(
    entries: &[FileIndexEntry],
    query: &str,
    limit: usize,
) -> Vec<FileIndexEntry> {
    let lower_query = query.to_lowercase();
    // 对齐 TS:limit 由调用方传入(默认 AT_RESULT_LIMIT 在调用点);显式 0 → 返回空。
    if lower_query.is_empty() {
        return entries.iter().take(limit).cloned().collect();
    }

    let mut scored: Vec<(i32, &FileIndexEntry)> = entries
        .iter()
        .filter_map(|entry| {
            let score = score_entry(entry, &lower_query);
            if score > 0 {
                Some((score, entry))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| path_depth(&a.1.path).cmp(&path_depth(&b.1.path)))
            .then_with(|| locale_compare_default(&a.1.path, &b.1.path))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry.clone())
        .collect()
}

/// 对齐 `AtInsertion`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtInsertion {
    /// 替换 @token 的文本。
    pub text: String,
    /// 插入后相对 `text` 起始的 caret 位置。
    pub cursor_offset: usize,
}

/// 对齐 `buildAtInsertText`。
///
/// 文件:闭合 token(`@path `,含空格时引号形式),caret 在尾部空格后。
/// 目录:保持菜单打开(`@dir/`);引号形式目录插入闭合形式(`@"my dir/"`),
/// caret 放在闭合引号前。
pub fn build_at_insert_text(entry_path: &str, is_dir: bool, force_quotes: bool) -> AtInsertion {
    let p = if is_dir {
        format!("{entry_path}/")
    } else {
        entry_path.to_string()
    };
    let needs_quotes = force_quotes || p.contains(' ');
    if is_dir {
        let text = if needs_quotes {
            format!("@\"{p}\"")
        } else {
            format!("@{p}")
        };
        return AtInsertion {
            cursor_offset: if needs_quotes {
                text.len() - 1
            } else {
                text.len()
            },
            text,
        };
    }
    let text = if needs_quotes {
        format!("@\"{p}\" ")
    } else {
        format!("@{p} ")
    };
    let cursor_offset = text.len();
    AtInsertion {
        text,
        cursor_offset,
    }
}

/// 对齐 `buildAtMentionText`。闭合 `@mention`(文件浏览器 @ 按钮)。
pub fn build_at_mention_text(entry_path: &str, is_dir: bool) -> String {
    let p = if is_dir {
        format!("{entry_path}/")
    } else {
        entry_path.to_string()
    };
    if p.contains(' ') {
        format!("@\"{p}\" ")
    } else {
        format!("@{p} ")
    }
}

/// 对齐 `buildFileLineMentionText`。限定单行或行范围的闭合 `@mention`。
pub fn build_file_line_mention_text(
    entry_path: &str,
    start_line: usize,
    end_line: usize,
) -> String {
    // Math.max(1, Math.min(a, b)) / Math.max(1, Math.max(a, b))
    let first_line = start_line.min(end_line).max(1);
    let last_line = start_line.max(end_line).max(1);
    let path_mention = if entry_path.contains(' ') {
        format!("@\"{entry_path}\"")
    } else {
        format!("@{entry_path}")
    };
    let line_suffix = if first_line == last_line {
        format!(":{first_line}")
    } else {
        format!(":{first_line}-{last_line}")
    };
    format!("{path_mention}{line_suffix} ")
}

/// 对齐 `buildFileAtMentionsText`。批量闭合 `@mention`(均为文件)。
pub fn build_file_at_mentions_text(entry_paths: &[String]) -> String {
    entry_paths
        .iter()
        .map(|p| build_at_mention_text(p, false))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> FileIndexEntry {
        FileIndexEntry {
            path: path.to_string(),
            is_dir,
        }
    }

    #[test]
    fn extract_at_query_plain() {
        assert_eq!(
            extract_at_query("@"),
            Some(AtQueryMatch {
                start: 0,
                query: String::new(),
                quoted: false
            })
        );
        assert_eq!(
            extract_at_query("  @foo"),
            Some(AtQueryMatch {
                start: 2,
                query: "foo".to_string(),
                quoted: false
            })
        );
        assert_eq!(
            extract_at_query("hi @foo"),
            Some(AtQueryMatch {
                start: 3,
                query: "foo".to_string(),
                quoted: false
            })
        );
        assert_eq!(
            extract_at_query("\t@tab"),
            Some(AtQueryMatch {
                start: 1,
                query: "tab".to_string(),
                quoted: false
            })
        );
        assert_eq!(extract_at_query("a@b"), None);
        assert_eq!(extract_at_query("foo@bar.com"), None);
        assert_eq!(extract_at_query("@foo bar"), None);
        // 回归:query 内可含 @。TS `(?:^|\s)@([^\s"]*)$` 锚定空白后的首个 @,
        // 之后的 @ 属于 query。Rust 曾锚定最右 @ 而漏匹配。
        assert_eq!(
            extract_at_query(" @a@b"),
            Some(AtQueryMatch {
                start: 1,
                query: "a@b".to_string(),
                quoted: false
            })
        );
    }

    #[test]
    fn extract_at_query_quoted() {
        assert_eq!(
            extract_at_query("@\"my dir/"),
            Some(AtQueryMatch {
                start: 0,
                query: "my dir/".to_string(),
                quoted: true
            })
        );
        assert_eq!(
            extract_at_query("hi @\"a b"),
            Some(AtQueryMatch {
                start: 3,
                query: "a b".to_string(),
                quoted: true
            })
        );
        // 空查询的引号形式
        assert_eq!(
            extract_at_query("@\""),
            Some(AtQueryMatch {
                start: 0,
                query: String::new(),
                quoted: true
            })
        );
        // 引号形式要求 @" 后到末尾不含 "(已闭合 → null)
        assert_eq!(extract_at_query("@\"my dir/x\""), None);
        assert_eq!(extract_at_query("@\"x\""), None);
        assert_eq!(extract_at_query("@\"quoted\"rest"), None);
    }

    #[test]
    fn build_entries_from_files_behavior() {
        let files = vec![
            "src/App.tsx".to_string(),
            "src/components/Chat.tsx".to_string(),
            "README.md".to_string(),
        ];
        let entries = build_entries_from_files(&files);
        // 排序:深度优先,平局按 localeCompare。
        // README.md(0) 与 src(0) 平局 → "README.md" < "src";src/App.tsx(1) 在
        // src/components(1) 前("src/App.tsx" < "src/components")。
        assert_eq!(
            entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec![
                "README.md",
                "src",
                "src/App.tsx",
                "src/components",
                "src/components/Chat.tsx"
            ]
        );
        assert_eq!(entries[0].is_dir, false); // README.md 是文件
        assert_eq!(entries[1].is_dir, true); // src 是目录
                                             // 空文件跳过
        let entries = build_entries_from_files(&["".to_string()]);
        assert!(entries.is_empty());
    }

    #[test]
    fn filter_empty_query_returns_slice() {
        let entries = vec![entry("a.ts", false), entry("b.ts", false)];
        assert_eq!(
            filter_file_entries(&entries, "", 1),
            vec![entry("a.ts", false)]
        );
        // 对齐 TS:显式 limit=0 → 返回空(不再被重解释为 AT_RESULT_LIMIT)。
        assert_eq!(filter_file_entries(&entries, "", 0).len(), 0);
    }

    #[test]
    fn filter_scoring_ladder() {
        let entries = vec![
            entry("src/App.tsx", false),
            entry("app/main.ts", false),
            entry("src/App.css", false),
        ];
        // "app":src/App.tsx basename "app.tsx" 前缀 80;src/App.css 前缀 80;
        // app/main.ts basename "main.ts" 不含 app → path 子串 30
        let out = filter_file_entries(&entries, "app", 20);
        assert!(out[0].path.starts_with("src/App."));
        assert!(out[1].path.starts_with("src/App."));
        assert_eq!(out[2].path, "app/main.ts");
        // "src/":含 "/" → 按完整路径匹配;src 目录自身不匹配 "src/"
        let entries = vec![
            entry("src", true),
            entry("src/App.tsx", false),
            entry("src/App.css", false),
        ];
        let out = filter_file_entries(&entries, "src/", 20);
        assert!(!out.iter().any(|e| e.path == "src"));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn filter_dir_bonus_and_subsequence() {
        // 目录 +10:src(前缀 80+10=90)排在 src/App.tsx(前缀 80)前
        let entries = vec![entry("src/App.tsx", false), entry("src", true)];
        let out = filter_file_entries(&entries, "src", 20);
        assert_eq!(out[0].path, "src");
        assert_eq!(out[0].is_dir, true);
        // 子序列:chinp 匹配 components/ChatInput.tsx
        let entries = vec![entry("components/ChatInput.tsx", false)];
        let out = filter_file_entries(&entries, "chinp", 20);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn insert_text_variants() {
        assert_eq!(
            build_at_insert_text("src/App.tsx", false, false),
            AtInsertion {
                text: "@src/App.tsx ".to_string(),
                cursor_offset: 13,
            }
        );
        assert_eq!(
            build_at_insert_text("src", true, false),
            AtInsertion {
                text: "@src/".to_string(),
                cursor_offset: 5,
            }
        );
        // 含空格目录 → 引号形式,caret 在闭合引号前
        let ins = build_at_insert_text("my dir", true, false);
        assert_eq!(ins.text, "@\"my dir/\"");
        assert_eq!(ins.cursor_offset, ins.text.len() - 1);
        // forceQuotes 强制引号
        let ins = build_at_insert_text("plain", false, true);
        assert_eq!(ins.text, "@\"plain\" ");
    }

    #[test]
    fn mention_text_variants() {
        assert_eq!(build_at_mention_text("src/App.tsx", false), "@src/App.tsx ");
        assert_eq!(build_at_mention_text("my dir", true), "@\"my dir/\" ");
        assert_eq!(
            build_file_line_mention_text("a b.ts", 3, 3),
            "@\"a b.ts\":3 "
        );
        assert_eq!(build_file_line_mention_text("c.ts", 2, 5), "@c.ts:2-5 ");
        // 行范围归一化(start>end 时交换)
        assert_eq!(build_file_line_mention_text("c.ts", 5, 2), "@c.ts:2-5 ");
        // 0 归一化为 1
        assert_eq!(build_file_line_mention_text("c.ts", 0, 0), "@c.ts:1 ");
        assert_eq!(
            build_file_at_mentions_text(&["a.ts".to_string(), "b.ts".to_string()]),
            "@a.ts @b.ts "
        );
    }

    #[test]
    fn locale_compare_approximation() {
        // ICU en:大小写折叠优先,折叠相等时小写在前
        assert_eq!(locale_compare_default("a", "b"), std::cmp::Ordering::Less);
        assert_eq!(
            locale_compare_default("A", "a"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            locale_compare_default("B", "a"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            locale_compare_default("a-b", "ab"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            locale_compare_default("src/a", "src/b"),
            std::cmp::Ordering::Less
        );
        assert_eq!(locale_compare_default("a", "a"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn serde_shapes() {
        let m = AtQueryMatch {
            start: 2,
            query: "foo".to_string(),
            quoted: false,
        };
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["start"], 2);
        assert_eq!(json["quoted"], false);

        let e = FileIndexEntry {
            path: "a/b".to_string(),
            is_dir: true,
        };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["path"], "a/b");
        assert_eq!(json["isDir"], true);
    }
}
