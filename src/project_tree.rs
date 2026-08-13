//! 对齐 `lib/project-tree.ts`。把会话树投影成发给客户端的浅层导航树。
//!
//! 保留根、分支点、叶子,同时收缩单子链(不递归遍历)。被收缩的 entry id 挂到
//! 下一个可见节点上(`compressedEntryIds`),UI 仍能识别链中的活动叶子。
//! 分支预览(`branchPreview`)取链中第一条消息预览(仅 role/text,剥离图片/思考/工具负载)。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 对齐 `BranchPreview`(lib/types.ts)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchPreview {
    /// `"user"` | `"assistant"`;其他 role 省略此字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub text: String,
}

/// 对齐 `ProjectableEntry`(project-tree.ts 内部类型)。
///
/// 用 `#[serde(flatten)]` 保留 entry 上所有额外字段(timestamp / parentId / name /
/// provider / modelId …),与 TS `cloneNode` 的 `{...node}` 展开语义一致。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectableEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 对齐 `ProjectableTreeNode<T>`(此处去泛型,用拥有的 children)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectableTreeNode {
    pub entry: ProjectableEntry,
    #[serde(default)]
    pub children: Vec<ProjectableTreeNode>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "compressedEntryIds"
    )]
    pub compressed_entry_ids: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "branchPreview"
    )]
    pub branch_preview: Option<BranchPreview>,
}

/// 对齐 `MAX_PROJECTED_TREE_DEPTH = 200`。
pub const MAX_PROJECTED_TREE_DEPTH: usize = 200;
/// 对齐 `MAX_BRANCH_PREVIEW_LENGTH = 40`。
const MAX_BRANCH_PREVIEW_LENGTH: usize = 40;

// ============================================================================
// UTF-16 单位辅助(JS 字符串 length / slice 以 UTF-16 码元计)
// ============================================================================

fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

/// 取前 `n` 个 UTF-16 码元(对齐 `s.slice(0, n)`)。
fn utf16_take(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for c in s.chars() {
        let l = c.len_utf16();
        if count + l > n {
            break;
        }
        out.push(c);
        count += l;
    }
    out
}

/// 对齐 `value.replace(/\s+/g, " ").trim()`。
fn collapse_whitespace(value: &str) -> String {
    use std::sync::OnceLock;
    static WS_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = WS_RE.get_or_init(|| regex::Regex::new(r"\s+").expect("valid whitespace regex"));
    re.replace_all(value, " ").trim().to_string()
}

// ============================================================================
// 预览文本
// ============================================================================

/// 对齐 `appendPreviewText(current, value)`。
///
/// `current.length > MAX` 时原样返回;否则把 `value`(经空白折叠+trim)接到
/// `current` 之后,总量封顶到 `MAX+1` 个 UTF-16 码元。
fn append_preview_text(current: &str, value: &str) -> String {
    if utf16_len(current) > MAX_BRANCH_PREVIEW_LENGTH {
        return current.to_string();
    }
    let normalized = collapse_whitespace(value);
    if normalized.is_empty() {
        return current.to_string();
    }
    let cur_len = utf16_len(current);
    let separator_len = if cur_len > 0 { 1 } else { 0 }; // " "
    let prefix_len = cur_len + separator_len;
    if prefix_len >= MAX_BRANCH_PREVIEW_LENGTH + 1 {
        // prefix.slice(0, MAX+1)
        let mut prefix = current.to_string();
        if separator_len > 0 {
            prefix.push(' ');
        }
        return utf16_take(&prefix, MAX_BRANCH_PREVIEW_LENGTH + 1);
    }
    let remaining = MAX_BRANCH_PREVIEW_LENGTH + 1 - prefix_len;
    let mut result = current.to_string();
    if separator_len > 0 {
        result.push(' ');
    }
    result.push_str(&utf16_take(&normalized, remaining));
    result
}

/// 对齐 `previewForEntry(entry)`。非消息/无 role 返回 `None`。
fn preview_for_entry(entry: &ProjectableEntry) -> Option<BranchPreview> {
    if entry.entry_type != "message" {
        return None;
    }
    let message = entry.message.as_ref()?;
    let msg = message.as_object()?;
    let role = msg.get("role")?.as_str()?;

    let content = msg.get("content");
    let mut text = String::new();
    let mut has_image = false;
    match content {
        Some(Value::String(s)) => {
            text = append_preview_text(&text, s);
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(b) = block.as_object() else { continue };
                if b.get("type").and_then(Value::as_str) == Some("image") {
                    has_image = true;
                }
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        text = append_preview_text(&text, t);
                    }
                }
                if utf16_len(&text) > MAX_BRANCH_PREVIEW_LENGTH {
                    break;
                }
            }
        }
        _ => {}
    }

    let text_len = utf16_len(&text);
    if text_len > MAX_BRANCH_PREVIEW_LENGTH {
        text = format!("{}…", utf16_take(&text, MAX_BRANCH_PREVIEW_LENGTH));
    } else if text.is_empty() {
        text = if has_image {
            "[image]".to_string()
        } else if role == "assistant" {
            "[assistant]".to_string()
        } else {
            "message".to_string()
        };
    }

    let role_out = if role == "user" || role == "assistant" {
        Some(role.to_string())
    } else {
        None
    };
    Some(BranchPreview {
        role: role_out,
        text,
    })
}

// ============================================================================
// 树投影(对齐 projectTreeForResponse)
// ============================================================================

/// 把输入树扁平化为按索引访问的模板 + 子索引表(递归,树深度有限)。
struct FlatTree {
    /// 每个节点的模板(entry + 原始 compressed_entry_ids / branch_preview,children 清空)。
    templates: Vec<ProjectableTreeNode>,
    /// 子节点索引表。
    children: Vec<Vec<usize>>,
    /// 根索引。
    roots: Vec<usize>,
}

impl FlatTree {
    fn from_nodes(nodes: Vec<ProjectableTreeNode>) -> Self {
        let mut tree = FlatTree {
            templates: Vec::new(),
            children: Vec::new(),
            roots: Vec::new(),
        };
        for node in nodes {
            let idx = tree.flatten(node);
            tree.roots.push(idx);
        }
        tree
    }

    fn flatten(&mut self, mut node: ProjectableTreeNode) -> usize {
        let idx = self.templates.len();
        // 模板保留 entry + 原始字段,children 单独抽出
        let drained = std::mem::take(&mut node.children);
        self.templates.push(node);
        self.children.push(Vec::with_capacity(drained.len()));
        for child in drained {
            let child_idx = self.flatten(child);
            self.children[idx].push(child_idx);
        }
        idx
    }

    fn child_count(&self, idx: usize) -> usize {
        self.children[idx].len()
    }
}

/// 投影构建器:维护投影节点表与父子关系,最后物化为拥有的树。
struct Projection {
    tree: FlatTree,
    /// keep[idx] = 该节点是否保留(根 或 子数 ≠ 1)。
    keep: Vec<bool>,
    /// 投影节点(已 clone,children 待填)。
    proj_nodes: Vec<ProjectableTreeNode>,
    /// 投影节点的子索引表。
    proj_children: Vec<Vec<usize>>,
}

impl Projection {
    /// 对齐 TS `cloneNode(node, compressedEntryIds?, branchPreview?)`。
    /// `compressed` = `None` 或空 → 不覆盖原 compressedEntryIds;非空 → 覆盖。
    /// `preview` = `Some` → 覆盖 branchPreview。
    fn clone_node(
        &mut self,
        src: usize,
        compressed: Option<&[String]>,
        preview: Option<BranchPreview>,
    ) -> usize {
        let mut node = self.tree.templates[src].clone();
        node.children = Vec::new();
        if let Some(c) = compressed {
            if !c.is_empty() {
                node.compressed_entry_ids = c.to_vec();
            }
        }
        if let Some(p) = preview {
            node.branch_preview = Some(p);
        }
        let pi = self.proj_nodes.len();
        self.proj_nodes.push(node);
        self.proj_children.push(Vec::new());
        pi
    }

    /// 物化投影树为拥有的节点。
    fn materialize(&self, proj_idx: usize) -> ProjectableTreeNode {
        let mut node = self.proj_nodes[proj_idx].clone();
        node.children = self.proj_children[proj_idx]
            .iter()
            .map(|&c| self.materialize(c))
            .collect();
        node
    }

    /// 对齐 `appendFlattenedKeptDescendants(source, projectedParent)`。
    fn append_flattened_kept_descendants(&mut self, source: usize, projected_parent: usize) {
        // pending: (node_idx, compressed, preview)
        let mut pending: Vec<(usize, Vec<String>, Option<BranchPreview>)> =
            vec![(source, Vec::new(), None)];
        let mut flattened_seen = vec![false; self.tree.templates.len()];

        while let Some((node, compressed, branch_preview)) = pending.pop() {
            if flattened_seen[node] {
                continue;
            }
            flattened_seen[node] = true;
            let next_preview = branch_preview
                .clone()
                .or_else(|| preview_for_entry(&self.tree.templates[node].entry));

            if self.keep[node] {
                let pi = self.clone_node(
                    node,
                    if compressed.is_empty() {
                        None
                    } else {
                        Some(&compressed)
                    },
                    next_preview.clone(),
                );
                self.proj_children[projected_parent].push(pi);
            }

            // 对齐 TS 反向压栈(子节点正序处理)
            for &child in self.tree.children[node].iter().rev() {
                let (child_compressed, child_preview) = if self.keep[node] {
                    (Vec::new(), None)
                } else {
                    let mut c = compressed.clone();
                    c.push(self.tree.templates[node].entry.id.clone());
                    (c, next_preview.clone())
                };
                pending.push((child, child_compressed, child_preview));
            }
        }
    }
}

/// 对齐 `projectTreeForResponse<T>(nodes: T[]): T[]`。
pub fn project_tree_for_response(nodes: Vec<ProjectableTreeNode>) -> Vec<ProjectableTreeNode> {
    let tree = FlatTree::from_nodes(nodes);
    let n = tree.templates.len();
    let roots: std::collections::HashSet<usize> = tree.roots.iter().copied().collect();

    // 计算 keep:遍历所有可达节点(显式栈,避免递归,对齐 TS)。
    let mut keep = vec![false; n];
    let mut seen = vec![false; n];
    let mut stack: Vec<usize> = tree.roots.clone();
    while let Some(node) = stack.pop() {
        if seen[node] {
            continue;
        }
        seen[node] = true;
        if roots.contains(&node) || tree.child_count(node) != 1 {
            keep[node] = true;
        }
        for &child in &tree.children[node] {
            stack.push(child);
        }
    }

    let mut proj = Projection {
        tree,
        keep,
        proj_nodes: Vec::new(),
        proj_children: Vec::new(),
    };

    // 投影根:cloneNode(node, undefined, previewForEntry(node.entry))
    let mut projected_roots: Vec<usize> = Vec::new();
    let root_indices: Vec<usize> = proj.tree.roots.clone();
    for root in root_indices {
        let preview = preview_for_entry(&proj.tree.templates[root].entry);
        let pi = proj.clone_node(root, None, preview);
        projected_roots.push(pi);
    }

    // tasks: (source_idx, projected_idx, depth) — 栈
    let mut tasks: Vec<(usize, usize, usize)> = proj
        .tree
        .roots
        .iter()
        .zip(projected_roots.iter())
        .map(|(&src, &p)| (src, p, 1usize))
        .collect();

    while let Some((source, projected, depth)) = tasks.pop() {
        let source_children: Vec<usize> = proj.tree.children[source].clone();
        for child in source_children {
            if depth >= MAX_PROJECTED_TREE_DEPTH {
                proj.append_flattened_kept_descendants(child, projected);
                continue;
            }

            let mut compressed: Vec<String> = Vec::new();
            let mut current = child;
            let mut branch_preview = preview_for_entry(&proj.tree.templates[current].entry);
            // 收缩单子非保留链
            while !proj.keep[current] && proj.tree.child_count(current) == 1 {
                compressed.push(proj.tree.templates[current].entry.id.clone());
                current = proj.tree.children[current][0];
                if branch_preview.is_none() {
                    branch_preview = preview_for_entry(&proj.tree.templates[current].entry);
                }
            }

            if !proj.keep[current] {
                continue;
            }

            let projected_child = proj.clone_node(
                current,
                if compressed.is_empty() {
                    None
                } else {
                    Some(&compressed)
                },
                branch_preview,
            );
            proj.proj_children[projected].push(projected_child);
            tasks.push((current, projected_child, depth + 1));
        }
    }

    projected_roots
        .iter()
        .map(|&pi| proj.materialize(pi))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(id: &str, role: impl Into<String>, content: Value) -> ProjectableTreeNode {
        ProjectableTreeNode {
            entry: ProjectableEntry {
                id: id.to_string(),
                entry_type: "message".to_string(),
                message: Some(json!({ "role": role.into(), "content": content })),
                extra: Map::new(),
            },
            children: Vec::new(),
            compressed_entry_ids: Vec::new(),
            branch_preview: None,
        }
    }

    fn info(id: &str) -> ProjectableTreeNode {
        ProjectableTreeNode {
            entry: ProjectableEntry {
                id: id.to_string(),
                entry_type: "session_info".to_string(),
                message: None,
                extra: {
                    let mut m = Map::new();
                    m.insert("name".to_string(), json!("x"));
                    m
                },
            },
            children: Vec::new(),
            compressed_entry_ids: Vec::new(),
            branch_preview: None,
        }
    }

    fn node(entry: ProjectableEntry, children: Vec<ProjectableTreeNode>) -> ProjectableTreeNode {
        ProjectableTreeNode {
            entry,
            children,
            compressed_entry_ids: Vec::new(),
            branch_preview: None,
        }
    }

    fn find_projected<'a>(
        nodes: &'a [ProjectableTreeNode],
        id: &str,
    ) -> Option<&'a ProjectableTreeNode> {
        let mut pending: Vec<&ProjectableTreeNode> = nodes.iter().collect();
        while let Some(current) = pending.pop() {
            if current.entry.id == id {
                return Some(current);
            }
            pending.extend(current.children.iter());
        }
        None
    }

    #[test]
    fn attaches_first_branch_message_preview_to_contracted_representative() {
        let arm1_leaf = node(info("a2").entry, vec![]);
        let arm1 = node(
            info("s1").entry,
            vec![node(
                msg(
                    "u2",
                    "user",
                    json!([
                        { "type": "text", "text": "分支一的问题" },
                        { "type": "image", "data": "base64-secret", "mimeType": "image/png" },
                    ]),
                )
                .entry,
                vec![arm1_leaf],
            )],
        );
        let arm2 = node(
            msg("u2b", "user", json!("分支二的问题")).entry,
            vec![node(msg("a2b", "assistant", json!("答二")).entry, vec![])],
        );
        let a1 = node(msg("a1", "assistant", json!("答")).entry, vec![arm1, arm2]);
        let root = node(msg("u1", "user", json!("第一问")).entry, vec![a1]);

        let projected = project_tree_for_response(vec![root]);
        let projected_root = &projected[0];
        let projected_a1 = &projected_root.children[0];
        assert_eq!(projected_a1.entry.id, "a1");
        assert_eq!(projected_a1.children[0].entry.id, "a2");
        assert_eq!(
            projected_a1.children[0].compressed_entry_ids,
            vec!["s1", "u2"]
        );
        assert_eq!(
            projected_a1.children[0].branch_preview.as_ref().unwrap(),
            &BranchPreview {
                role: Some("user".into()),
                text: "分支一的问题".into(),
            }
        );
        assert_eq!(projected_a1.children[1].entry.id, "a2b");
        assert_eq!(projected_a1.children[1].compressed_entry_ids, vec!["u2b"]);
        assert_eq!(
            projected_a1.children[1].branch_preview.as_ref().unwrap(),
            &BranchPreview {
                role: Some("user".into()),
                text: "分支二的问题".into(),
            }
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("base64-secret"));
    }

    #[test]
    fn linear_sessions_project_to_root_and_leaf_only() {
        let root = node(
            msg("u1", "user", json!("第一问")).entry,
            vec![node(msg("a1", "assistant", json!("答")).entry, vec![])],
        );
        let projected = project_tree_for_response(vec![root]);
        assert_eq!(projected[0].entry.id, "u1");
        assert_eq!(projected[0].children.len(), 1);
        assert_eq!(projected[0].children[0].entry.id, "a1");
    }

    #[test]
    fn does_not_copy_thinking_or_tool_payloads_from_compressed_source() {
        let assistant = node(
            msg(
                "a1",
                "assistant",
                json!([
                    { "type": "thinking", "thinking": "thinking-secret" },
                    { "type": "text", "text": "可见回答" },
                    { "type": "toolCall", "id": "tc1", "name": "read", "arguments": { "value": "tool-secret" } },
                ]),
            )
            .entry,
            vec![node(info("leaf1").entry, vec![])],
        );
        let sibling = node(
            msg("u2", "user", json!("另一个分支")).entry,
            vec![node(info("leaf2").entry, vec![])],
        );
        let projected =
            project_tree_for_response(vec![node(info("root").entry, vec![assistant, sibling])]);
        let leaf = find_projected(&projected, "leaf1").unwrap();
        assert_eq!(
            leaf.branch_preview.as_ref().unwrap(),
            &BranchPreview {
                role: Some("assistant".into()),
                text: "可见回答".into(),
            }
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("thinking-secret"));
        assert!(!serialized.contains("tool-secret"));
    }

    #[test]
    fn normalizes_and_bounds_preview_text_and_labels_image_only_messages() {
        let long_text = format!("  第一行\n\n第二行 {}", "x".repeat(80));
        let text_arm = node(
            msg("u1", "user", json!(long_text)).entry,
            vec![node(info("leaf1").entry, vec![])],
        );
        let image_arm = node(
            msg(
                "u2",
                "user",
                json!([{ "type": "image", "data": "secret-image", "mimeType": "image/png" }]),
            )
            .entry,
            vec![node(info("leaf2").entry, vec![])],
        );
        let projected =
            project_tree_for_response(vec![node(info("root").entry, vec![text_arm, image_arm])]);
        let text_preview = find_projected(&projected, "leaf1")
            .unwrap()
            .branch_preview
            .as_ref()
            .unwrap();
        let image_preview = find_projected(&projected, "leaf2")
            .unwrap()
            .branch_preview
            .as_ref()
            .unwrap();

        assert!(text_preview.text.starts_with("第一行 第二行 "));
        assert_eq!(text_preview.text.chars().count(), 41);
        assert!(text_preview.text.ends_with('…'));
        assert_eq!(
            image_preview,
            &BranchPreview {
                role: Some("user".into()),
                text: "[image]".into(),
            }
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("secret-image"));
    }

    #[test]
    fn does_not_copy_unknown_message_roles_into_previews() {
        let unknown_role = format!("unknown-role-{}", "x".repeat(80));
        let arm = node(
            msg("m1", unknown_role.clone(), json!("可见内容")).entry,
            vec![node(info("leaf1").entry, vec![])],
        );
        let sibling = node(
            msg("u2", "user", json!("另一个分支")).entry,
            vec![node(info("leaf2").entry, vec![])],
        );
        let projected =
            project_tree_for_response(vec![node(info("root").entry, vec![arm, sibling])]);
        let preview = find_projected(&projected, "leaf1")
            .unwrap()
            .branch_preview
            .as_ref()
            .unwrap();
        assert_eq!(
            preview,
            &BranchPreview {
                role: None,
                text: "可见内容".into()
            }
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("unknown-role-"));
    }

    #[test]
    fn carries_previews_through_depth_limit_flattening() {
        let mut deep_arm = node(
            info("prefix").entry,
            vec![node(
                msg("deep-user", "user", json!("深层问题")).entry,
                vec![node(info("deep-leaf").entry, vec![])],
            )],
        );
        for i in 0..(MAX_PROJECTED_TREE_DEPTH + 2) {
            deep_arm = node(
                info(&format!("branch-{i}")).entry,
                vec![deep_arm, node(info(&format!("side-{i}")).entry, vec![])],
            );
        }
        let projected = project_tree_for_response(vec![deep_arm]);
        let leaf = find_projected(&projected, "deep-leaf").unwrap();
        assert_eq!(
            leaf.branch_preview.as_ref().unwrap(),
            &BranchPreview {
                role: Some("user".into()),
                text: "深层问题".into(),
            }
        );
        assert_eq!(leaf.compressed_entry_ids, vec!["prefix", "deep-user"]);
        assert!(find_projected(&projected, "deep-user").is_none());
    }

    #[test]
    fn carries_previews_through_non_message_roots_in_multi_root_trees() {
        let root1 = node(
            info("m1").entry,
            vec![node(
                msg("u1", "user", json!("第一个问题")).entry,
                vec![node(msg("a1", "assistant", json!("回答一")).entry, vec![])],
            )],
        );
        // root1 root entry should be model_change-like; reuse info shape for the test.
        let _ = root1;
        let r1 = node(
            info("m1").entry,
            vec![node(
                msg("u1", "user", json!("第一个问题")).entry,
                vec![node(msg("a1", "assistant", json!("回答一")).entry, vec![])],
            )],
        );
        let r2 = node(
            info("s2").entry,
            vec![node(
                msg("u2", "user", json!("第二个问题")).entry,
                vec![node(msg("a2", "assistant", json!("回答二")).entry, vec![])],
            )],
        );
        let projected = project_tree_for_response(vec![r1, r2]);
        assert_eq!(
            projected[0].children[0].branch_preview.as_ref().unwrap(),
            &BranchPreview {
                role: Some("user".into()),
                text: "第一个问题".into(),
            }
        );
        assert_eq!(
            projected[1].children[0].branch_preview.as_ref().unwrap(),
            &BranchPreview {
                role: Some("user".into()),
                text: "第二个问题".into(),
            }
        );
        assert_eq!(projected[0].children[0].compressed_entry_ids, vec!["u1"]);
        assert_eq!(projected[1].children[0].compressed_entry_ids, vec!["u2"]);
    }
}
