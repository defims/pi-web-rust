//! 对齐 `lib/rpc-manager.ts` 的纯逻辑部分。
//!
//! rpc-manager 整体是引擎绑定的 AgentSessionWrapper 注册表 + RPC 命令分发器
//! (moho-mate 的 `chat_thread.rs` 已覆盖该层)。这里移植其可独立测试的部分:
//! - `with_extension_tools`:把扩展工具名并入请求的工具列表(去重保序)
//! - `normalize_rpc_cwd`:`resolve` + `realpath` 归一化,失败回退 resolve
//! - 运行状态/空闲重置事件类型判定
//! - `track_starting_session`:起始会话按 cwd 的引用计数

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// 对齐 `CODING_TOOL_NAMES`。
pub const CODING_TOOL_NAMES: [&str; 7] = ["read", "bash", "edit", "write", "grep", "find", "ls"];

/// 对齐 `RUNNING_STATE_EVENT_TYPES`。
pub const RUNNING_STATE_EVENT_TYPES: [&str; 7] = [
    "agent_start",
    "agent_end",
    "agent_settled",
    "auto_compaction_start",
    "auto_compaction_end",
    "compaction_start",
    "compaction_end",
];

/// 对齐 `IDLE_RESET_EVENT_TYPES`。
pub const IDLE_RESET_EVENT_TYPES: [&str; 4] = [
    "agent_end",
    "agent_settled",
    "auto_compaction_end",
    "compaction_end",
];

/// 对齐 `withExtensionTools`。
///
/// `extension_tool_names` 注入 `session.getAllTools()` 里非 coding 工具的名字;
/// 返回 `[...new Set([...toolNames, ...extensionToolNames])]`(保序去重)。
pub fn with_extension_tools(tool_names: &[String], extension_tool_names: &[String]) -> Vec<String> {
    if tool_names.is_empty() {
        return Vec::new();
    }
    let coding: HashSet<&str> = CODING_TOOL_NAMES.iter().copied().collect();
    let extension: Vec<&str> = extension_tool_names
        .iter()
        .map(|s| s.as_str())
        .filter(|name| !coding.contains(name))
        .collect();

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for name in tool_names.iter().map(|s| s.as_str()).chain(extension.iter().copied()) {
        if seen.insert(name) {
            out.push(name.to_string());
        }
    }
    out
}

/// 对齐 `normalizeRpcCwd`:`resolve(cwd)` 后尝试 realpath,失败回退 resolve 结果。
pub fn normalize_rpc_cwd(cwd: &str) -> String {
    let resolved = resolve_path(cwd);
    std::fs::canonicalize(&resolved)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(resolved)
}

/// `path.resolve`(POSIX 语义;相对路径按进程 cwd,`..`/`.` 折叠)。
fn resolve_path(value: &str) -> String {
    let (absolute, rest) = if let Some(stripped) = value.strip_prefix('/') {
        (true, stripped)
    } else {
        (false, value)
    };

    let mut stack: Vec<String> = Vec::new();
    if !absolute {
        // 相对路径先以进程 cwd 的段作为初始栈(对齐 resolve 拼接 cwd 再折叠)
        let cwd = std::env::current_dir()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());
        for seg in cwd.split('/').filter(|s| !s.is_empty()) {
            stack.push(seg.to_string());
        }
    }

    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if !stack.is_empty() {
                    stack.pop();
                }
            }
            seg => stack.push(seg.to_string()),
        }
    }

    format!("/{}", stack.join("/"))
}

/// 对齐 `RUNNING_STATE_EVENT_TYPES.has(eventType)`。
pub fn is_running_state_event(event_type: &str) -> bool {
    RUNNING_STATE_EVENT_TYPES.contains(&event_type)
}

/// 对齐 `IDLE_RESET_EVENT_TYPES.has(eventType)`。
pub fn is_idle_reset_event(event_type: &str) -> bool {
    IDLE_RESET_EVENT_TYPES.contains(&event_type)
}

/// 对齐 `getStartingSessionCwds` + `trackStartingSession` 的计数语义。
///
/// 用 `RefCell` 内部可变性(对齐 TS 的 globalThis Map 语义),`track` 只借 `&self`。
#[derive(Debug, Default)]
pub struct StartingSessionTracker {
    counts: RefCell<HashMap<String, usize>>,
}

impl StartingSessionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 开始跟踪一次会话启动,返回释放句柄(对齐返回的 `() => void`)。
    pub fn track(&self, cwd: &str) -> StartingSessionGuard<'_> {
        let key = normalize_rpc_cwd(cwd);
        let mut counts = self.counts.borrow_mut();
        let count = counts.entry(key.clone()).or_insert(0);
        *count += 1;
        StartingSessionGuard { tracker: self, key }
    }

    /// 对齐 `getStartingSessionCwds().get(key)`(仅测试/诊断)。
    pub fn count_for(&self, cwd: &str) -> usize {
        let key = normalize_rpc_cwd(cwd);
        self.counts.borrow().get(&key).copied().unwrap_or(0)
    }

    fn release(&self, key: &str) {
        let mut counts = self.counts.borrow_mut();
        let remaining = match counts.get_mut(key) {
            Some(count) if *count > 1 => {
                *count -= 1;
                *count
            }
            _ => 0,
        };
        if remaining == 0 {
            counts.remove(key);
        }
    }
}

/// track 返回的释放句柄(RAII)。
pub struct StartingSessionGuard<'a> {
    tracker: &'a StartingSessionTracker,
    key: String,
}

impl Drop for StartingSessionGuard<'_> {
    fn drop(&mut self) {
        self.tracker.release(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_tools_merge() {
        let tools = vec!["bash".to_string(), "custom_a".to_string()];
        let extensions = vec!["custom_a".to_string(), "custom_b".to_string(), "bash".to_string()];
        let out = with_extension_tools(&tools, &extensions);
        assert_eq!(out, vec!["bash", "custom_a", "custom_b"]);
    }

    #[test]
    fn extension_tools_empty_short_circuit() {
        assert_eq!(with_extension_tools(&[], &["x".to_string()]), Vec::<String>::new());
    }

    #[test]
    fn coding_tools_filtered_from_extensions() {
        let extensions = vec!["read".to_string(), "bash".to_string(), "ext".to_string()];
        let out = with_extension_tools(&["bash".to_string()], &extensions);
        assert_eq!(out, vec!["bash", "ext"]);
    }

    #[test]
    fn normalize_cwd_resolves() {
        // 相对路径含 `..` → resolve 折叠
        let cwd = std::env::current_dir().unwrap();
        let parent = cwd.parent().unwrap();
        let resolved = normalize_rpc_cwd("..");
        assert_eq!(std::path::Path::new(&resolved), parent);
        // 绝对路径(可能 realpath 相同)
        let abs = normalize_rpc_cwd(&cwd.to_string_lossy());
        assert!(std::path::Path::new(&abs).is_absolute());
    }

    #[test]
    fn event_type_sets() {
        assert!(is_running_state_event("agent_start"));
        assert!(is_running_state_event("compaction_end"));
        assert!(!is_running_state_event("message_update"));
        assert!(is_idle_reset_event("agent_end"));
        assert!(!is_idle_reset_event("agent_start"));
    }

    #[test]
    fn starting_tracker_refcounts() {
        let mut tracker = StartingSessionTracker::new();
        let _g1 = tracker.track("/tmp/a");
        let _g2 = tracker.track("/tmp/a");
        assert_eq!(tracker.count_for("/tmp/a"), 2);
        drop(_g1);
        assert_eq!(tracker.count_for("/tmp/a"), 1);
        drop(_g2);
        assert_eq!(tracker.count_for("/tmp/a"), 0);
    }
}
