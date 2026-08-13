//! 对齐 `lib/skills-service.ts`。技能加载编排。
//!
//! 上游:DefaultResourceLoader.reload → getSkills → annotateSkillsWithInstallInfo
//! → getProjectTrustStatus。Rust 版把 DefaultResourceLoader(引擎侧)抽象成
//! 注入回调,annotation 与 trust 状态在本模块忠实实现。

use serde::{Deserialize, Serialize};

use super::skill_lock::{annotate_skills_with_install_info, SkillInfo};

/// 对齐 `ResourceDiagnostic`(TS SDK 类型)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDiagnostic {
    pub r#type: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 对齐 `SkillsResponse`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsResponse {
    pub skills: Vec<SkillInfo>,
    pub diagnostics: Vec<ResourceDiagnostic>,
    pub project_resources_loaded: bool,
}

/// 引擎(DefaultResourceLoader)操作面,供 pi_agent_rust 接线。
pub trait SkillsLoader {
    /// `loader.reload(...)` + `loader.getSkills()`,返回技能与诊断。
    fn load_skills(&self) -> (Vec<SkillInfo>, Vec<ResourceDiagnostic>);
}

/// 对齐 `loadSkillsWithInstallInfo`。
pub fn load_skills_with_install_info(
    cwd: &str,
    agent_dir: &str,
    global_lock_path: &str,
    project_lock_path: &str,
    loader: &dyn SkillsLoader,
    trusted: bool,
) -> SkillsResponse {
    let (mut skills, diagnostics) = loader.load_skills();
    annotate_skills_with_install_info(
        &mut skills,
        cwd,
        agent_dir,
        global_lock_path,
        project_lock_path,
    );
    SkillsResponse {
        skills,
        diagnostics,
        project_resources_loaded: trusted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLoader;

    impl SkillsLoader for FakeLoader {
        fn load_skills(&self) -> (Vec<SkillInfo>, Vec<ResourceDiagnostic>) {
            (
                vec![SkillInfo {
                    name: "demo".to_string(),
                    description: "d".to_string(),
                    file_path: "/tmp/out-of-scope/SKILL.md".to_string(),
                    base_dir: "b".to_string(),
                    disable_model_invocation: false,
                    source_info: Default::default(),
                    install: None,
                }],
                vec![ResourceDiagnostic {
                    r#type: "warning".to_string(),
                    message: "pattern matched nothing".to_string(),
                    source: None,
                    path: None,
                }],
            )
        }
    }

    #[test]
    fn load_annotates_and_passes_trust() {
        let response = load_skills_with_install_info(
            "/cwd",
            "/agent",
            "/nonexistent/global.json",
            "/nonexistent/project.json",
            &FakeLoader,
            true,
        );
        assert_eq!(response.skills.len(), 1);
        // 文件不存在 → 不标注 install
        assert!(response.skills[0].install.is_none());
        assert_eq!(response.diagnostics[0].r#type, "warning");
        assert_eq!(response.project_resources_loaded, true);
    }

    #[test]
    fn serialize_shapes() {
        let response = SkillsResponse {
            skills: vec![],
            diagnostics: vec![ResourceDiagnostic {
                r#type: "error".to_string(),
                message: "m".to_string(),
                source: Some("s".to_string()),
                path: None,
            }],
            project_resources_loaded: false,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["diagnostics"][0]["type"], "error");
        assert_eq!(json["diagnostics"][0]["source"], "s");
        assert_eq!(json["projectResourcesLoaded"], false);
        assert_eq!(json["skills"], serde_json::json!([]));
    }
}
