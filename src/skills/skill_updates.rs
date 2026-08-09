//! 对齐 `lib/skill-updates.ts`。技能更新检测。
//!
//! - 纯函数:`skill_update_key` / `skill_slug` / `skill_name_from_package` /
//!   `skill_folder` / `build_skill_update_args` / `result`
//! - 编排:`check_skill_update`(guard → 按 scope 分流 → catch 成 error 结果)、
//!   `check_skill_updates`(按 URL 去重的共享 fetcher)
//! - 网络/git 操作抽象为 [`SkillUpdateIo`] trait 注入(运行时无关):
//!   GitHub trees API(401/403/429 → 回退 git rev-parse)与 skills.sh snapshot

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::file::paths::url_encode_component;
use crate::skills::skill_lock::SkillInstallInfo;

/// 对齐 `CHECK_TIMEOUT_MS`。
pub const CHECK_TIMEOUT_MS: u64 = 15_000;
/// 对齐 `GIT_CHECK_TIMEOUT_MS`。
pub const GIT_CHECK_TIMEOUT_MS: u64 = 30_000;
/// 对齐 `DEFAULT_SKILLS_API_BASE`。
pub const DEFAULT_SKILLS_API_BASE: &str = "https://skills.sh";

/// 对齐 `SkillUpdateState`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillUpdateState {
    UpToDate,
    UpdateAvailable,
    Unsupported,
    Error,
}

/// 对齐 `SkillUpdateResult`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdateResult {
    pub package: String,
    pub scope: String,
    pub state: SkillUpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 对齐 `skillUpdateKey`:`scope\0package`。
pub fn skill_update_key(scope: &str, package: &str) -> String {
    format!("{scope}\0{package}")
}

/// 对齐 `skillSlug`:小写 → 空白/下划线转 `-` → 去非法字符 → 压缩 `-` → 去首尾 `-`。
pub fn skill_slug(name: &str) -> String {
    let lower = name.to_lowercase();
    let dashed = lower
        .replace(|c: char| c.is_whitespace() || c == '_', "-")
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect::<String>();
    let collapsed = dashed.replace("--", "-");
    let mut collapsed = collapsed;
    while collapsed.contains("--") {
        collapsed = collapsed.replace("--", "-");
    }
    collapsed.trim_matches('-').to_string()
}

/// 对齐 `skillNameFromPackage`:最后一个 `@` 之后的部分。
pub fn skill_name_from_package(pkg: &str) -> String {
    match pkg.rfind('@') {
        Some(at) => pkg[at + 1..].to_string(),
        None => pkg.to_string(),
    }
}

/// 对齐 `skillFolder`:反斜杠转正斜杠,去掉尾部 `/skill.md`(大小写不敏感)或
/// 无目录形式的 `skill.md`,再去尾部 `/`。
pub fn skill_folder(skill_path: &str) -> String {
    let mut folder = skill_path.replace('\\', "/");
    let lower = folder.to_lowercase();
    if lower.ends_with("/skill.md") {
        folder = folder[..folder.len() - 9].to_string();
    } else if lower.ends_with("skill.md") {
        folder = folder[..folder.len() - 8].to_string();
    }
    folder.trim_end_matches('/').to_string()
}

/// 对齐 `buildSkillUpdateArgs`。
pub fn build_skill_update_args(install: &SkillInstallInfo) -> Vec<String> {
    let folder = skill_folder(install.skill_path.as_deref().unwrap_or(""));
    let source = if folder.is_empty() {
        install.source.clone()
    } else {
        format!("{}/{}", install.source, folder)
    };
    let ref_ = install
        .ref_
        .as_deref()
        .map(|r| format!("#{}", url_encode_component(r)))
        .unwrap_or_default();
    let mut args = vec![
        "skills".to_string(),
        "add".to_string(),
        format!("{source}{ref_}"),
        "--skill".to_string(),
        skill_name_from_package(&install.package),
        "-y".to_string(),
        "--agent".to_string(),
        "pi".to_string(),
    ];
    if install.scope == "global" {
        args.push("-g".to_string());
    }
    args
}

/// 对齐 `result(...)`。
pub fn result(
    install: &SkillInstallInfo,
    state: SkillUpdateState,
    latest_version: Option<String>,
    message: Option<String>,
) -> SkillUpdateResult {
    SkillUpdateResult {
        package: install.package.clone(),
        scope: install.scope.clone(),
        state,
        current_version: install.version_hash.clone(),
        latest_version,
        message,
    }
}

/// 对齐 `HttpError(status)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError(pub u16);

/// 对齐 `fetchJson` 的语义:非 2xx → [`HttpError`]。
#[derive(Debug, Clone)]
pub enum SkillUpdateIoError {
    Http(HttpError),
    Transport(String),
}

impl std::fmt::Display for SkillUpdateIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillUpdateIoError::Http(e) => write!(f, "HTTP {}", e.0),
            SkillUpdateIoError::Transport(msg) => f.write_str(msg),
        }
    }
}

impl From<HttpError> for SkillUpdateIoError {
    fn from(e: HttpError) -> Self {
        SkillUpdateIoError::Http(e)
    }
}

/// 网络/git 操作面(trait,供宿主接线;async 运行时无关)。
pub trait SkillUpdateIo {
    /// `fetchJson(url, headers)` 等价:失败返回 Err(HTTP 状态或传输错误)。
    fn fetch_json(
        &self,
        url: &str,
        github_token: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, SkillUpdateIoError>> + Send + '_>>;

    /// `resolveGitTreeHash` 等价(深度 1 fetch + rev-parse)。
    fn resolve_git_tree_hash(
        &self,
        install: &SkillInstallInfo,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>;
}

/// 对齐 `GitHubTreeEntry` / `GitHubTreeResponse` 的读取。
fn tree_entry_sha_for_folder(raw: &Value, folder: &str) -> Option<String> {
    let tree = raw.get("tree")?.as_array()?;
    tree.iter().find(|item| {
        item.get("type").and_then(|t| t.as_str()) == Some("tree")
            && item.get("path").and_then(|p| p.as_str()) == Some(folder)
    })
    .and_then(|item| item.get("sha").and_then(|s| s.as_str()).map(|s| s.to_string()))
}

/// 对齐 `checkGlobalSkill`(不含 Io 注入部分)。
pub async fn check_global_skill(
    install: &SkillInstallInfo,
    io: &dyn SkillUpdateIo,
    github_token: Option<&str>,
) -> Result<SkillUpdateResult, SkillUpdateIoError> {
    let ref_ = install.ref_.as_deref().unwrap_or("HEAD");
    let url = format!(
        "https://api.github.com/repos/{}/git/trees/{}?recursive=1",
        install.source,
        url_encode_component(ref_)
    );
    let folder = skill_folder(install.skill_path.as_deref().unwrap_or(""));
    let raw = match io.fetch_json(&url, github_token).await {
        Ok(raw) => raw,
        Err(SkillUpdateIoError::Http(HttpError(status)))
            if matches!(status, 401 | 403 | 429) =>
        {
            // 回退 git rev-parse(私有仓库 / 限流)
            let hash = io.resolve_git_tree_hash(install).await.map_err(SkillUpdateIoError::Transport)?;
            return Ok(result(
                install,
                if hash == install.version_hash.as_deref().unwrap_or("") {
                    SkillUpdateState::UpToDate
                } else {
                    SkillUpdateState::UpdateAvailable
                },
                Some(hash),
                None,
            ));
        }
        Err(e) => return Err(e),
    };

    let latest_version = if folder.is_empty() {
        raw.get("sha").and_then(|s| s.as_str()).map(|s| s.to_string())
    } else {
        tree_entry_sha_for_folder(&raw, &folder)
    };

    match latest_version {
        Some(latest) => Ok(result(
            install,
            if latest == install.version_hash.as_deref().unwrap_or("") {
                SkillUpdateState::UpToDate
            } else {
                SkillUpdateState::UpdateAvailable
            },
            Some(latest),
            None,
        )),
        None => Ok(result(
            install,
            SkillUpdateState::Error,
            None,
            Some("Remote skill path was not found.".to_string()),
        )),
    }
}

/// 对齐 `checkProjectSkill`。
pub async fn check_project_skill(
    install: &SkillInstallInfo,
    io: &dyn SkillUpdateIo,
    skills_api_base: &str,
) -> Result<SkillUpdateResult, SkillUpdateIoError> {
    let (owner, repo) = match install.source.split_once('/') {
        Some(pair) => pair,
        None => {
            return Ok(result(
                install,
                SkillUpdateState::Error,
                None,
                Some("Invalid source for project skill.".to_string()),
            ));
        }
    };
    let name = skill_slug(&skill_name_from_package(&install.package));
    let url = format!(
        "{}/api/download/{}/{}/{}",
        skills_api_base.trim_end_matches('/'),
        url_encode_component(owner),
        url_encode_component(repo),
        url_encode_component(&name),
    );
    let raw = io.fetch_json(&url, None).await?;
    let latest_version = raw.get("hash").and_then(|h| h.as_str()).map(|h| h.to_string());

    match latest_version {
        Some(latest) => Ok(result(
            install,
            if latest == install.version_hash.as_deref().unwrap_or("") {
                SkillUpdateState::UpToDate
            } else {
                SkillUpdateState::UpdateAvailable
            },
            Some(latest),
            None,
        )),
        None => Ok(result(
            install,
            SkillUpdateState::Error,
            None,
            Some("skills.sh did not return a version hash.".to_string()),
        )),
    }
}

/// 对齐 `checkSkillUpdate`(guard + 分流 + catch 成 error 结果)。
pub async fn check_skill_update(
    install: &SkillInstallInfo,
    io: &dyn SkillUpdateIo,
    skills_api_base: &str,
    github_token: Option<&str>,
) -> SkillUpdateResult {
    if !install.can_check_for_updates || install.version_hash.is_none() || install.skill_path.is_none() {
        return result(
            install,
            SkillUpdateState::Unsupported,
            None,
            Some("This lock entry cannot be checked automatically.".to_string()),
        );
    }

    let outcome = if install.scope == "global" {
        check_global_skill(install, io, github_token).await
    } else {
        check_project_skill(install, io, skills_api_base).await
    };

    match outcome {
        Ok(r) => r,
        Err(e) => result(install, SkillUpdateState::Error, None, Some(e.to_string())),
    }
}

/// 对齐 `checkSkillUpdates`(按 URL 去重的共享 fetcher → 并行检查)。
pub async fn check_skill_updates(
    installs: &[SkillInstallInfo],
    io: &dyn SkillUpdateIo,
    skills_api_base: &str,
    github_token: Option<&str>,
) -> Vec<SkillUpdateResult> {
    let installs = installs.to_vec();
    let mut handles = Vec::with_capacity(installs.len());
    for install in installs {
        handles.push(async move {
            check_skill_update(&install, io, skills_api_base, github_token).await
        });
    }
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        out.push(handle.await);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn install(
        package: &str,
        scope: &str,
        source: &str,
        skill_path: Option<&str>,
        ref_: Option<&str>,
        version_hash: Option<&str>,
    ) -> SkillInstallInfo {
        SkillInstallInfo {
            package: package.to_string(),
            scope: scope.to_string(),
            source: source.to_string(),
            source_type: Some("github".to_string()),
            skills_sh_url: None,
            skill_path: skill_path.map(|s| s.to_string()),
            ref_: ref_.map(|s| s.to_string()),
            version_hash: version_hash.map(|s| s.to_string()),
            can_check_for_updates: true,
        }
    }

    #[test]
    fn update_key() {
        assert_eq!(skill_update_key("global", "o/r@s"), "global\0o/r@s");
        assert_eq!(skill_update_key("project", "x"), "project\0x");
    }

    #[test]
    fn slug_cases() {
        assert_eq!(skill_slug("My Skill Name"), "my-skill-name");
        assert_eq!(skill_slug("A  B___C!"), "a-b-c");
        assert_eq!(skill_slug("-leading-trailing-"), "leading-trailing");
        assert_eq!(skill_slug("   "), "");
        assert_eq!(skill_slug("already-kebab"), "already-kebab");
        assert_eq!(skill_slug("数字123"), "123");
    }

    #[test]
    fn name_from_package() {
        assert_eq!(skill_name_from_package("org/repo@my-skill"), "my-skill");
        assert_eq!(skill_name_from_package("no-at-sign"), "no-at-sign");
        assert_eq!(skill_name_from_package("a@b@c"), "c");
    }

    #[test]
    fn folder_cases() {
        assert_eq!(skill_folder("/agent/skills/demo/SKILL.md"), "/agent/skills/demo");
        assert_eq!(skill_folder("/agent/skills/demo/skill.md"), "/agent/skills/demo");
        assert_eq!(skill_folder("demo/skill.md"), "demo");
        assert_eq!(skill_folder(r"C:\skills\demo\SKILL.md"), "C:/skills/demo");
        // 尾部带斜杠时不命中 skill.md 分支 → 只去尾部斜杠(对齐 TS)
        assert_eq!(skill_folder("/a/b/skill.md/"), "/a/b/skill.md");
        assert_eq!(skill_folder("/plain/dir/"), "/plain/dir");
    }

    #[test]
    fn build_args() {
        // 带 skillPath 与 ref:TS 原样拼接 `${source}/${folder}`,允许双斜杠
        let i = install("org/repo@demo", "global", "org/repo", Some("/x/SKILL.md"), Some("v1.2"), Some("h"));
        let args = build_skill_update_args(&i);
        assert_eq!(args, vec![
            "skills", "add", "org/repo//x#v1.2", "--skill", "demo", "-y", "--agent", "pi", "-g",
        ]);

        // 无 skillPath → 直接用 source;ref 空格编码
        let i = install("org/repo@demo", "project", "org/repo", None, Some("feat/x"), Some("h"));
        let args = build_skill_update_args(&i);
        assert_eq!(args, vec![
            "skills", "add", "org/repo#feat%2Fx", "--skill", "demo", "-y", "--agent", "pi",
        ]);

        // 无 ref:skillFolder("/x/SKILL.md") → "/x",拼接为双斜杠(对齐 TS)
        let i = install("org/repo@demo", "global", "org/repo", Some("/x/SKILL.md"), None, Some("h"));
        let args = build_skill_update_args(&i);
        assert_eq!(args[2], "org/repo//x");
    }

    #[test]
    fn result_builder() {
        let i = install("o/r@s", "global", "o/r", Some("/x/SKILL.md"), None, Some("abc"));
        let r = result(&i, SkillUpdateState::UpToDate, Some("abc".to_string()), None);
        assert_eq!(r.package, "o/r@s");
        assert_eq!(r.scope, "global");
        assert_eq!(r.state, SkillUpdateState::UpToDate);
        assert_eq!(r.current_version.as_deref(), Some("abc"));
        assert_eq!(r.latest_version.as_deref(), Some("abc"));
        assert_eq!(r.message, None);
    }

    #[test]
    fn tree_entry_lookup() {
        let raw = json!({
            "sha": "root-sha",
            "tree": [
                { "path": "not-folder", "type": "blob", "sha": "b1" },
                { "path": "demo", "type": "tree", "sha": "tree-sha" },
                { "path": "demo/nested", "type": "tree", "sha": "nested" }
            ]
        });
        assert_eq!(tree_entry_sha_for_folder(&raw, "demo"), Some("tree-sha".to_string()));
        // 只匹配 type === "tree" 且 path 精确相等
        assert_eq!(tree_entry_sha_for_folder(&raw, "not-folder"), None);
        assert_eq!(tree_entry_sha_for_folder(&raw, "missing"), None);
        assert_eq!(tree_entry_sha_for_folder(&json!({"tree": "x"}), "demo"), None);
    }

    struct FakeIo {
        global_response: Result<Value, SkillUpdateIoError>,
        tree_hash: Result<String, String>,
        project_response: Result<Value, SkillUpdateIoError>,
    }

    impl SkillUpdateIo for FakeIo {
        fn fetch_json(
            &self,
            _url: &str,
            _token: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, SkillUpdateIoError>> + Send + '_>> {
            let response = if _url.contains("api.github.com") {
                self.global_response.clone()
            } else {
                self.project_response.clone()
            };
            Box::pin(async move { response })
        }

        fn resolve_git_tree_hash(
            &self,
            _install: &SkillInstallInfo,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let hash = self.tree_hash.clone();
            Box::pin(async move { hash })
        }
    }

    #[tokio::test]
    async fn global_skill_up_to_date() {
        let io = FakeIo {
            global_response: Ok(json!({ "sha": "abc", "tree": [] })),
            tree_hash: Err("unused".to_string()),
            project_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
        };
        // skillPath "SKILL.md"(根级)→ folder 为空 → 用根 sha 比较
        let i = install("o/r@s", "global", "o/r", Some("SKILL.md"), None, Some("abc"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::UpToDate);
        assert_eq!(r.latest_version.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn global_skill_update_available() {
        let io = FakeIo {
            global_response: Ok(json!({ "sha": "new-sha", "tree": [] })),
            tree_hash: Err("unused".to_string()),
            project_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
        };
        let i = install("o/r@s", "global", "o/r", Some("SKILL.md"), None, Some("old"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::UpdateAvailable);
        assert_eq!(r.latest_version.as_deref(), Some("new-sha"));
    }

    #[tokio::test]
    async fn global_skill_folder_tree_hash() {
        let io = FakeIo {
            // folder 与 tree entry 的 path 精确匹配(TS: path === folder)
            global_response: Ok(json!({
                "tree": [{ "path": "/x/demo", "type": "tree", "sha": "tree-hash" }]
            })),
            tree_hash: Err("unused".to_string()),
            project_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
        };
        let i = install("o/r@s", "global", "o/r", Some("/x/demo/SKILL.md"), None, Some("tree-hash"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::UpToDate);
    }

    #[tokio::test]
    async fn global_skill_http_ratelimit_falls_back_to_git() {
        let io = FakeIo {
            global_response: Err(SkillUpdateIoError::Http(HttpError(403))),
            tree_hash: Ok("git-hash".to_string()),
            project_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
        };
        let i = install("o/r@s", "global", "o/r", Some("/x/SKILL.md"), None, Some("git-hash"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::UpToDate);
        assert_eq!(r.latest_version.as_deref(), Some("git-hash"));
    }

    #[tokio::test]
    async fn global_skill_http_500_errors() {
        let io = FakeIo {
            global_response: Err(SkillUpdateIoError::Http(HttpError(500))),
            tree_hash: Ok("x".to_string()),
            project_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
        };
        let i = install("o/r@s", "global", "o/r", Some("/x/SKILL.md"), None, Some("abc"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::Error);
        assert!(r.message.as_deref().unwrap_or("").contains("HTTP 500"));
    }

    #[tokio::test]
    async fn global_skill_path_not_found() {
        let io = FakeIo {
            global_response: Ok(json!({ "tree": [{ "path": "other", "type": "tree", "sha": "x" }] })),
            tree_hash: Err("unused".to_string()),
            project_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
        };
        let i = install("o/r@s", "global", "o/r", Some("/x/missing/SKILL.md"), None, Some("abc"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::Error);
        assert_eq!(r.message.as_deref(), Some("Remote skill path was not found."));
    }

    #[tokio::test]
    async fn project_skill_check() {
        let io = FakeIo {
            global_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
            tree_hash: Err("unused".to_string()),
            project_response: Ok(json!({ "hash": "snap-hash" })),
        };
        let i = install("org/repo@My Skill", "project", "org/repo", Some("/x/SKILL.md"), None, Some("old"));
        let r = check_skill_update(&i, &io, "https://skills.sh", None).await;
        assert_eq!(r.state, SkillUpdateState::UpdateAvailable);
        assert_eq!(r.latest_version.as_deref(), Some("snap-hash"));
        // slug 后的 name 会进 URL(URL 内用编码,无需在此断言具体值)
    }

    #[tokio::test]
    async fn unsupported_when_guard_fails() {
        let io = FakeIo {
            global_response: Ok(json!({ "sha": "abc" })),
            tree_hash: Ok("abc".to_string()),
            project_response: Ok(json!({ "hash": "abc" })),
        };
        // canCheckForUpdates = false
        let mut i = install("o/r@s", "global", "o/r", Some("/x/SKILL.md"), None, Some("abc"));
        i.can_check_for_updates = false;
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::Unsupported);
        assert_eq!(
            r.message.as_deref(),
            Some("This lock entry cannot be checked automatically.")
        );
    }

    #[tokio::test]
    async fn project_skill_no_hash() {
        let io = FakeIo {
            global_response: Err(SkillUpdateIoError::Transport("unused".to_string())),
            tree_hash: Err("unused".to_string()),
            project_response: Ok(json!({ "nope": true })),
        };
        let i = install("org/repo@My Skill", "project", "org/repo", Some("/x/SKILL.md"), None, Some("old"));
        let r = check_skill_update(&i, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(r.state, SkillUpdateState::Error);
        assert_eq!(
            r.message.as_deref(),
            Some("skills.sh did not return a version hash.")
        );
    }

    #[tokio::test]
    async fn multiple_updates_parallel() {
        let io = FakeIo {
            global_response: Ok(json!({ "sha": "same" })),
            tree_hash: Ok("same".to_string()),
            project_response: Ok(json!({ "hash": "same" })),
        };
        let installs = vec![
            install("o/r@a", "global", "o/r", Some("SKILL.md"), None, Some("same")),
            install("o/r@b", "global", "o/r", Some("SKILL.md"), None, Some("diff")),
        ];
        let results = check_skill_updates(&installs, &io, DEFAULT_SKILLS_API_BASE, None).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].state, SkillUpdateState::UpToDate);
        assert_eq!(results[1].state, SkillUpdateState::UpdateAvailable);
    }

    #[test]
    fn serialize_shapes() {
        let r = result(
            &install("o/r@s", "global", "o/r", Some("/x/SKILL.md"), None, Some("abc")),
            SkillUpdateState::UpdateAvailable,
            Some("def".to_string()),
            None,
        );
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["state"], "update-available");
        assert_eq!(json["currentVersion"], "abc");
        assert_eq!(json["latestVersion"], "def");
        assert!(json.get("message").is_none());

        let r2 = result(&install("o/r@s", "global", "o/r", Some("/x"), None, None), SkillUpdateState::Error, None, Some("m".to_string()));
        let json = serde_json::to_value(&r2).unwrap();
        assert_eq!(json["state"], "error");
        assert_eq!(json["message"], "m");
        assert!(json.get("currentVersion").is_none());
    }
}
