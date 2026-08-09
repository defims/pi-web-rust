//! 对齐 `lib/skill-lock.ts`。skills-lock.json 的安装信息标注。
//!
//! 纯字符串/路径计算(除 lock 文件读取与文件存在性检查外无 IO);
//! `path.relative`/`path.resolve` 语义按 Node 探针逐项对齐。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::file::paths::url_encode_component;

/// 对齐 `SkillInstallScope`。
pub type SkillInstallScope = String;

/// 对齐 `SkillInstallInfo`(annotate 输出)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallInfo {
    pub package: String,
    pub scope: SkillInstallScope,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_sh_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_hash: Option<String>,
    #[serde(rename = "canCheckForUpdates")]
    pub can_check_for_updates: bool,
}

/// 对齐 `SkillInfo`(annotate 输入/输出)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
    pub disable_model_invocation: bool,
    pub source_info: SourceInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<SkillInstallInfo>,
}

/// 对齐 `SkillInfo.sourceInfo`。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// 对齐 `SkillLockEntry`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLockEntry {
    pub source: Option<String>,
    pub source_type: Option<String>,
    pub skill_path: Option<String>,
    pub ref_: Option<String>,
    pub skill_folder_hash: Option<String>,
    pub computed_hash: Option<String>,
}

/// 对齐 `SkillLockFile`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLockFile {
    pub skills: Option<BTreeMap<String, SkillLockEntry>>,
}

/// 对齐 `getGlobalSkillsLockPath`。
pub fn get_global_skills_lock_path(home_dir: &str, xdg_state_home: Option<&str>) -> String {
    match xdg_state_home {
        Some(xdg) if !xdg.trim().is_empty() => {
            format!("{}/skills/.skill-lock.json", xdg.trim_end_matches('/'))
        }
        _ => format!("{}/.agents/.skill-lock.json", home_dir.trim_end_matches('/')),
    }
}

/// 对齐 `readSkillLock`。读取失败/解析失败 → 空 map。
pub fn read_skill_lock(path: &str) -> BTreeMap<String, SkillLockEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match serde_json::from_str::<SkillLockFile>(&content) {
        Ok(parsed) => parsed.skills.unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    }
}

/// 对齐 `isWithin`:`path.relative(resolve(root), resolve(path))` 非空、
/// 非 `..`、不以 `../` 开头、非绝对路径。
pub fn is_within(path: &str, root: &str) -> bool {
    let rel = path_relative(&path_resolve(root), &path_resolve(path));
    !rel.is_empty() && rel != ".." && !rel.starts_with("../") && !is_absolute(&rel)
}

/// 对齐 `findLockEntry`:精确名优先,否则大小写不敏感回退。
pub fn find_lock_entry<'a>(
    entries: &'a BTreeMap<String, SkillLockEntry>,
    skill_name: &str,
) -> Option<&'a SkillLockEntry> {
    if let Some(entry) = entries.get(skill_name) {
        return Some(entry);
    }
    let normalized = skill_name.to_lowercase();
    let key = entries
        .keys()
        .find(|name| name.to_lowercase() == normalized)?;
    entries.get(key)
}

/// 对齐 `normalizeSource`。github 源剥离 `git+`/`https://github.com/`/
/// `git@github.com:`/`.git`/尾部 `/`;其他源只去尾部 `/`。
pub fn normalize_source(source: &str, source_type: Option<&str>) -> String {
    if source_type != Some("github") {
        return source.trim_end_matches('/').to_string();
    }
    let mut out = source.to_string();
    for prefix in [
        "git+",
        "https://github.com/",
        "http://github.com/",
        "git@github.com:",
    ] {
        if let Some(rest) = out.strip_prefix(prefix) {
            out = rest.to_string();
        }
    }
    for suffix in [".git", "/"] {
        if let Some(rest) = out.strip_suffix(suffix) {
            out = rest.to_string();
        }
    }
    out
}

/// 对齐 `buildSkillsShUrl`。含 `://` 或 `git@` 前缀 → None;
/// 路径分段 encodeURIComponent 后拼接。
pub fn build_skills_sh_url(source: &str, skill_name: &str) -> Option<String> {
    if source.is_empty() || source.contains("://") || source.starts_with("git@") {
        return None;
    }
    let source_path = source
        .split('/')
        .filter(|s| !s.is_empty())
        .map(url_encode_component)
        .collect::<Vec<_>>()
        .join("/");
    if source_path.is_empty() {
        return None;
    }
    Some(format!(
        "https://skills.sh/{source_path}/{}",
        url_encode_component(skill_name)
    ))
}

/// 对齐 `getInstallInfo`。
pub fn get_install_info(
    entries: &BTreeMap<String, SkillLockEntry>,
    skill_name: &str,
    scope: &str,
) -> Option<SkillInstallInfo> {
    let entry = find_lock_entry(entries, skill_name)?;
    let source_raw = entry.source.as_ref()?;
    let source_trimmed = source_raw.trim();
    if source_trimmed.is_empty() {
        return None;
    }

    let source_type = entry.source_type.clone();
    let source = normalize_source(source_trimmed, source_type.as_deref());
    if source.is_empty() {
        return None;
    }
    let skill_path = entry.skill_path.clone();
    let ref_ = entry.ref_.clone();
    let raw_version_hash = if scope == "global" {
        entry.skill_folder_hash.clone()
    } else {
        entry.computed_hash.clone()
    };
    let version_hash = raw_version_hash.filter(|h| !h.is_empty());
    let is_github_source =
        source_type.as_deref() == Some("github") && is_github_repo_path(&source);
    let has_comparable_version = scope == "global" || ref_.is_none();

    Some(SkillInstallInfo {
        package: format!("{source}@{skill_name}"),
        scope: scope.to_string(),
        source: source.clone(),
        source_type: source_type.clone(),
        skills_sh_url: if source_type.as_deref() == Some("local") {
            None
        } else {
            build_skills_sh_url(&source, skill_name)
        },
        skill_path: skill_path.clone(),
        ref_,
        version_hash: version_hash.clone(),
        can_check_for_updates: is_github_source && skill_path.is_some() && version_hash.is_some() && has_comparable_version,
    })
}

/// `^[\w.-]+\/[\w.-]+$` 对齐。
fn is_github_repo_path(source: &str) -> bool {
    let Some((owner, repo)) = source.split_once('/') else {
        return false;
    };
    is_word_dot_dash(owner) && is_word_dot_dash(repo) && !repo.contains('/')
}

fn is_word_dot_dash(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// 对齐 `annotateSkillsWithInstallInfo`。
pub fn annotate_skills_with_install_info(
    skills: &mut [SkillInfo],
    cwd: &str,
    agent_dir: &str,
    global_lock_path: &str,
    project_lock_path: &str,
) {
    let global_entries = read_skill_lock(global_lock_path);
    let project_entries = read_skill_lock(project_lock_path);
    let global_skills_root = format!("{}/skills", agent_dir.trim_end_matches('/'));
    let project_skills_root = format!("{}/.pi/skills", cwd.trim_end_matches('/'));

    for skill in skills.iter_mut() {
        if !std::path::Path::new(&skill.file_path).exists() {
            continue;
        }
        let install = if is_within(&skill.file_path, &global_skills_root) {
            get_install_info(&global_entries, &skill.name, "global")
        } else if is_within(&skill.file_path, &project_skills_root) {
            get_install_info(&project_entries, &skill.name, "project")
        } else {
            None
        };
        if install.is_some() {
            skill.install = install;
        }
    }
}

// ============================================================================
// path.resolve / path.relative 对齐(Node 探针验证)
// ============================================================================

/// 对齐 `path.resolve(path)`:相对路径按进程 cwd 解析,`..`/`.` 折叠。
/// 仅支持 POSIX 语义(上游在服务端通常为 POSIX;Windows 需 path.win32 变体)。
pub fn path_resolve(value: &str) -> String {
    let (absolute, rest) = if let Some(stripped) = value.strip_prefix('/') {
        (true, stripped)
    } else {
        (false, value)
    };
    let mut stack: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if !stack.is_empty() {
                    stack.pop();
                }
            }
            seg => stack.push(seg),
        }
    }
    if absolute {
        format!("/{}", stack.join("/"))
    } else {
        // 相对路径:resolve 会拼上进程 cwd
        let cwd = std::env::current_dir()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());
        let cwd_abs = if cwd.starts_with('/') { cwd } else { format!("/{cwd}") };
        let mut combined = cwd_abs;
        for seg in &stack {
            combined.push('/');
            combined.push_str(seg);
        }
        combined
    }
}

/// 对齐 `path.relative(from, to)`(POSIX 语义)。
fn path_relative(from: &str, to: &str) -> String {
    let from_parts: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();

    let mut common = 0;
    while common < from_parts.len()
        && common < to_parts.len()
        && from_parts[common] == to_parts[common]
    {
        common += 1;
    }

    let mut out: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        out.push("..".to_string());
    }
    for part in &to_parts[common..] {
        out.push(part.to_string());
    }
    out.join("/")
}

fn is_absolute(value: &str) -> bool {
    value.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: Option<&str>, source_type: Option<&str>, skill_path: Option<&str>, ref_: Option<&str>, folder_hash: Option<&str>, computed_hash: Option<&str>) -> SkillLockEntry {
        SkillLockEntry {
            source: source.map(|s| s.to_string()),
            source_type: source_type.map(|s| s.to_string()),
            skill_path: skill_path.map(|s| s.to_string()),
            ref_: ref_.map(|s| s.to_string()),
            skill_folder_hash: folder_hash.map(|s| s.to_string()),
            computed_hash: computed_hash.map(|s| s.to_string()),
        }
    }

    #[test]
    fn lock_paths() {
        assert_eq!(
            get_global_skills_lock_path("/home/u", None),
            "/home/u/.agents/.skill-lock.json"
        );
        assert_eq!(
            get_global_skills_lock_path("/home/u", Some("/xdg")),
            "/xdg/skills/.skill-lock.json"
        );
        assert_eq!(
            get_global_skills_lock_path("/home/u/", Some("/xdg/")),
            "/xdg/skills/.skill-lock.json"
        );
    }

    #[test]
    fn read_lock_missing_is_empty() {
        let entries = read_skill_lock("/nonexistent/definitely-missing.json");
        assert!(entries.is_empty());
    }

    #[test]
    fn is_within_semantics() {
        assert!(is_within("/a/b", "/a"));
        assert!(is_within("/home/u/.pi/skills", "/home/u"));
        assert!(!is_within("/a/b", "/a/b")); // rel = "" → false
        assert!(!is_within("/a", "/a/b")); // rel = ".." → false
        assert!(!is_within("/ab/c", "/a")); // rel = "../ab/c" → false
        assert!(!is_within("/aa", "/a")); // rel = "../aa" → false
        assert!(is_within("/a/b/c/d", "/a/b/c"));
    }

    #[test]
    fn find_entry_case_insensitive() {
        let mut entries = BTreeMap::new();
        entries.insert("MySkill".to_string(), entry(Some("org/repo"), None, None, None, None, None));
        assert!(find_lock_entry(&entries, "MySkill").is_some());
        assert!(find_lock_entry(&entries, "myskill").is_some());
        assert!(find_lock_entry(&entries, "other").is_none());
    }

    #[test]
    fn normalize_source_cases() {
        assert_eq!(normalize_source("git+https://github.com/foo/bar.git", Some("github")), "foo/bar");
        assert_eq!(normalize_source("https://github.com/foo/bar", Some("github")), "foo/bar");
        assert_eq!(normalize_source("git@github.com:foo/bar.git", Some("github")), "foo/bar");
        assert_eq!(normalize_source("foo/bar/", Some("github")), "foo/bar");
        assert_eq!(normalize_source("foo/bar", None), "foo/bar");
        assert_eq!(normalize_source("local-dir/", None), "local-dir");
        assert_eq!(normalize_source("https://example.com/x/", None), "https://example.com/x");
        // 非 github 源不去前缀
        assert_eq!(normalize_source("https://github.com/foo/bar", None), "https://github.com/foo/bar");
    }

    #[test]
    fn skills_sh_url_building() {
        assert_eq!(
            build_skills_sh_url("org/repo", "my skill"),
            Some("https://skills.sh/org/repo/my%20skill".to_string())
        );
        assert_eq!(build_skills_sh_url("https://x", "s"), None);
        assert_eq!(build_skills_sh_url("git@github.com:o/r", "s"), None);
        assert_eq!(build_skills_sh_url("", "s"), None);
        assert_eq!(build_skills_sh_url("/", "s"), None);
        // 带空格与斜杠的源分段编码
        assert_eq!(
            build_skills_sh_url("a b/c", "s"),
            Some("https://skills.sh/a%20b/c/s".to_string())
        );
    }

    #[test]
    fn install_info_global() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "cli-tools".to_string(),
            entry(
                Some("https://github.com/acme/cli-tools"),
                Some("github"),
                Some("/agent/skills/cli-tools"),
                Some("v1.0.0"),
                Some("abc123"),
                Some("ignored-for-global"),
            ),
        );
        let info = get_install_info(&entries, "cli-tools", "global").unwrap();
        assert_eq!(info.package, "acme/cli-tools@cli-tools");
        assert_eq!(info.scope, "global");
        assert_eq!(info.source, "acme/cli-tools");
        assert_eq!(info.source_type.as_deref(), Some("github"));
        assert_eq!(info.skill_path.as_deref(), Some("/agent/skills/cli-tools"));
        assert_eq!(info.ref_.as_deref(), Some("v1.0.0"));
        assert_eq!(info.version_hash.as_deref(), Some("abc123"));
        assert_eq!(info.can_check_for_updates, true);
        // global + skillPath + versionHash → skills.sh URL 存在
        assert!(info.skills_sh_url.is_some());
    }

    #[test]
    fn install_info_project_uses_computed_hash() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "s".to_string(),
            entry(
                Some("org/repo"),
                Some("github"),
                Some("/cwd/.pi/skills/s"),
                Some("abc123"),
                None,
                Some("computed-hash"),
            ),
        );
        let info = get_install_info(&entries, "s", "project").unwrap();
        assert_eq!(info.version_hash.as_deref(), Some("computed-hash"));
        // project scope + 有 ref → 不可检查更新
        assert_eq!(info.can_check_for_updates, false);
    }

    #[test]
    fn install_info_can_check_rules() {
        // 缺 skillPath → false
        let mut entries = BTreeMap::new();
        entries.insert("a".to_string(), entry(Some("o/r"), Some("github"), None, None, Some("h"), None));
        assert_eq!(get_install_info(&entries, "a", "global").unwrap().can_check_for_updates, false);
        // 缺 versionHash → false
        let mut entries = BTreeMap::new();
        entries.insert("a".to_string(), entry(Some("o/r"), Some("github"), Some("/x"), None, None, None));
        assert_eq!(get_install_info(&entries, "a", "global").unwrap().can_check_for_updates, false);
        // 非 github → false
        let mut entries = BTreeMap::new();
        entries.insert("a".to_string(), entry(Some("o/r"), None, Some("/x"), None, Some("h"), None));
        assert_eq!(get_install_info(&entries, "a", "global").unwrap().can_check_for_updates, false);
    }

    #[test]
    fn install_info_local_no_sh_url() {
        let mut entries = BTreeMap::new();
        entries.insert("a".to_string(), entry(Some("/local/path"), Some("local"), Some("/x"), None, None, None));
        let info = get_install_info(&entries, "a", "global").unwrap();
        assert_eq!(info.skills_sh_url, None);
    }

    #[test]
    fn install_info_missing_parts() {
        let mut entries = BTreeMap::new();
        // source 为空 → undefined
        entries.insert("a".to_string(), entry(Some("  "), None, None, None, None, None));
        assert!(get_install_info(&entries, "a", "global").is_none());
        // 完全缺失 → undefined
        assert!(get_install_info(&entries, "nope", "global").is_none());
    }

    #[test]
    fn annotate_skips_missing_and_unscoped() {
        let mut skills = vec![
            SkillInfo {
                name: "s1".to_string(),
                description: "d".to_string(),
                file_path: "/nonexistent/nope".to_string(),
                base_dir: "b".to_string(),
                disable_model_invocation: false,
                source_info: SourceInfo::default(),
                install: None,
            },
            SkillInfo {
                name: "s2".to_string(),
                description: "d".to_string(),
                file_path: "/some/other/path".to_string(),
                base_dir: "b".to_string(),
                disable_model_invocation: false,
                source_info: SourceInfo::default(),
                install: None,
            },
        ];
        // 两个 lock 文件都不存在 → read 返回空
        annotate_skills_with_install_info(
            &mut skills,
            "/cwd",
            "/agent",
            "/nonexistent/global-lock.json",
            "/nonexistent/project-lock.json",
        );
        assert!(skills[0].install.is_none()); // 文件不存在 → 跳过
        assert!(skills[1].install.is_none()); // 不在任何 root 内
    }

    #[test]
    fn annotate_global_install() {
        // 先写一个全局 lock 文件
        let dir = std::env::temp_dir().join(format!("pi-skill-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("agent/skills")).unwrap();
        let global_lock = dir.join("global-lock.json");
        std::fs::write(
            &global_lock,
            r#"{"skills":{"demo":{"source":"org/demo","sourceType":"github","skillPath":"/agent/skills/demo","ref":"v1","skillFolderHash":"h1"}}}"#,
        )
        .unwrap();
        let skill_file = dir.join("agent/skills/demo/SKILL.md");
        std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        std::fs::write(&skill_file, "# demo").unwrap();

        let mut skills = vec![SkillInfo {
            name: "demo".to_string(),
            description: "d".to_string(),
            file_path: skill_file.to_string_lossy().to_string(),
            base_dir: "b".to_string(),
            disable_model_invocation: false,
            source_info: SourceInfo::default(),
            install: None,
        }];
        annotate_skills_with_install_info(
            &mut skills,
            "/cwd",
            dir.join("agent").to_str().unwrap(),
            global_lock.to_str().unwrap(),
            "/nonexistent/project-lock.json",
        );
        let install = skills[0].install.as_ref().expect("global install annotated");
        assert_eq!(install.scope, "global");
        assert_eq!(install.source, "org/demo");
        assert_eq!(install.version_hash.as_deref(), Some("h1"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serialize_shapes() {
        let info = SkillInstallInfo {
            package: "o/r@s".to_string(),
            scope: "global".to_string(),
            source: "o/r".to_string(),
            source_type: Some("github".to_string()),
            skills_sh_url: Some("https://skills.sh/o/r/s".to_string()),
            skill_path: Some("/x".to_string()),
            ref_: Some("v1".to_string()),
            version_hash: Some("h".to_string()),
            can_check_for_updates: true,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["canCheckForUpdates"], true);
        assert_eq!(json["scope"], "global");
        // ref 字段序列化为 ref(serde 自动)
        assert_eq!(json["ref"], "v1");

        let empty = SkillInstallInfo {
            package: "p".to_string(),
            scope: "project".to_string(),
            source: "s".to_string(),
            source_type: None,
            skills_sh_url: None,
            skill_path: None,
            ref_: None,
            version_hash: None,
            can_check_for_updates: false,
        };
        let json = serde_json::to_value(&empty).unwrap();
        assert!(json.get("sourceType").is_none());
        assert!(json.get("skillPath").is_none());
    }
}
