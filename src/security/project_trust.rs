//! 对齐 `lib/project-trust.ts`。项目信任状态。
//!
//! 依赖 TS SDK 的 hasTrustRequiringProjectResources + ProjectTrustStore。
//! Rust 版提供结构 + 桩实现(恒信任,对齐 moho-mate 的当前行为)。

use serde::{Deserialize, Serialize};

/// 对齐 `ProjectTrustStatus`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustStatus {
    pub requires_trust: bool,
    pub trusted: bool,
}

/// 对齐 `getProjectTrustStatus`。
///
/// 桩实现:恒信任。pi_agent_rust 的 `hasTrustRequiringProjectResources` 目前不暴露
/// 公开 API(扩展信任判定在 agent.rs 内部)。对于 moho-mate 的嵌入式 webview 场景,
/// 目录由用户显式选择(无远程 untrusted 请求),恒信任是安全正确的。
/// pi-web 远程部署场景未来需要补 hasTrustRequiringProjectResources 等价。
pub fn get_project_trust_status(_cwd: &str, _agent_dir: &str) -> ProjectTrustStatus {
    ProjectTrustStatus {
        requires_trust: false,
        trusted: true,
    }
}

/// 对齐 `trustProject`。
pub fn trust_project(_cwd: &str, _agent_dir: &str) -> ProjectTrustStatus {
    ProjectTrustStatus {
        requires_trust: false,
        trusted: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_trusted_stub() {
        let status = get_project_trust_status("/any/path", "/any/agent");
        assert!(status.trusted);
        assert!(!status.requires_trust);
    }
}
