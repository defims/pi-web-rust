//! tools 模块 — 对齐 agegr/pi-web `lib/tool-presets.ts`。

use serde::{Deserialize, Serialize};

/// 对齐 `ToolEntry`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEntry {
    pub name: String,
    pub description: String,
    pub active: bool,
}

/// 对齐 `ToolPreset`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPreset {
    None,
    Default,
    Full,
}

/// 内置工具名(对齐 `PRESET_FULL`)。
pub const BUILTIN_TOOL_NAMES: &[&str] = &["bash", "read", "edit", "write", "grep", "find", "ls"];

/// 对齐 `getPresetFromTools`。根据激活的工具集推断预设。
pub fn get_preset_from_tools(tools: &[ToolEntry]) -> ToolPreset {
    let mut active: Vec<&str> = tools
        .iter()
        .filter(|t| t.active)
        .map(|t| t.name.as_str())
        .filter(|name| BUILTIN_TOOL_NAMES.contains(name))
        .collect();
    if active.is_empty() {
        return ToolPreset::None;
    }
    active.sort();
    let joined = active.join(",");

    let mut default_sorted = ["read", "bash", "edit", "write"];
    default_sorted.sort();
    let mut full_sorted = ["bash", "read", "edit", "write", "grep", "find", "ls"];
    full_sorted.sort();

    if joined == default_sorted.join(",") {
        ToolPreset::Default
    } else if joined == full_sorted.join(",") {
        ToolPreset::Full
    } else {
        ToolPreset::Default
    }
}

/// 对齐 `getToolNamesForPreset`。
pub fn get_tool_names_for_preset(preset: ToolPreset) -> Vec<&'static str> {
    match preset {
        ToolPreset::None => vec![],
        ToolPreset::Full => vec!["bash", "read", "edit", "write", "grep", "find", "ls"],
        ToolPreset::Default => vec!["read", "bash", "edit", "write"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_from_tools() {
        let full: Vec<ToolEntry> = ["bash", "read", "edit", "write", "grep", "find", "ls"]
            .iter()
            .map(|n| ToolEntry { name: n.to_string(), description: "".into(), active: true })
            .collect();
        assert_eq!(get_preset_from_tools(&full), ToolPreset::Full);

        let default: Vec<ToolEntry> = ["read", "bash", "edit", "write"]
            .iter()
            .map(|n| ToolEntry { name: n.to_string(), description: "".into(), active: true })
            .collect();
        assert_eq!(get_preset_from_tools(&default), ToolPreset::Default);

        let none: Vec<ToolEntry> = vec![];
        assert_eq!(get_preset_from_tools(&none), ToolPreset::None);
    }

    #[test]
    fn tool_names_for_preset() {
        assert!(get_tool_names_for_preset(ToolPreset::None).is_empty());
        assert_eq!(get_tool_names_for_preset(ToolPreset::Default).len(), 4);
        assert_eq!(get_tool_names_for_preset(ToolPreset::Full).len(), 7);
    }
}
