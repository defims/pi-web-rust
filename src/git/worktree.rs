//! 对齐 `lib/worktree.ts`。
//!
//! git worktree 项目解析 + worktree 增删查。async + std::process::Command。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// 对齐 TS `/[\/\\:*?"<>|\s]+/g`(特殊字符或空白「连续段」折叠为单个 `-`)。
static SANITIZE_SPECIAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\/\\:*?"<>|\s]+"#).expect("valid sanitize special regex"));
/// 对齐 TS `/^-+|-+$/g`(剥离开头/结尾的连字符)。
static SANITIZE_EDGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-+|-+$").expect("valid sanitize edge regex"));

use crate::fs::allowed_roots::allow_file_root;

const PROJECT_CACHE_TTL: Duration = Duration::from_secs(60);

/// 对齐 `ProjectInfo`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub project_root: String,
    pub branch: Option<String>,
    pub is_worktree: bool,
    pub is_top_level: bool,
}

/// 对齐 `WorktreeInfo`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub is_main: bool,
}

// ── 项目缓存 ─────────────────────────────────────────────────────────────

struct ProjectCache {
    entries: HashMap<String, (ProjectInfo, Instant)>,
}

static PROJECT_CACHE: LazyLock<Mutex<ProjectCache>> = LazyLock::new(|| {
    Mutex::new(ProjectCache {
        entries: HashMap::new(),
    })
});

/// 对齐 `invalidateProjectCache`。
pub fn invalidate_project_cache() {
    if let Ok(mut cache) = PROJECT_CACHE.lock() {
        cache.entries.clear();
    }
}

// ── git 执行 ─────────────────────────────────────────────────────────────

fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let full: Vec<&str> = ["-C", cwd]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    let output = Command::new("git")
        .args(&full)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "git command failed".to_string()
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ── 项目解析 ─────────────────────────────────────────────────────────────

/// 对齐 TS `realPathOrSelf(filePath)`:`realpathSync`(canonicalize,解析符号链接),
/// 失败回退原值。project_root / repo_root 等用它消除 symlink 差异,保证同一仓库分组一致。
fn real_path_or_self(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string())
}

/// 对齐 `inferRemovedWorktree`。
fn infer_removed_worktree(cwd: &str) -> Option<ProjectInfo> {
    let parent = Path::new(cwd).parent()?;
    let parent_str = parent.to_string_lossy();
    if !parent_str.ends_with("-worktrees") {
        return None;
    }
    let repo_root = parent_str.trim_end_matches("-worktrees");
    if repo_root.is_empty() || !Path::new(&format!("{repo_root}/.git")).exists() {
        return None;
    }
    Some(ProjectInfo {
        // 对齐 TS `projectRoot: realPathOrSelf(repoRoot)`。
        project_root: real_path_or_self(repo_root),
        branch: Some(Path::new(cwd).file_name()?.to_string_lossy().to_string()),
        is_worktree: true,
        is_top_level: true,
    })
}

/// 对齐 `resolveProject`。cwd → {projectRoot, branch, isWorktree, isTopLevel}。
pub async fn resolve_project(cwd: &str) -> ProjectInfo {
    // 检查缓存
    {
        if let Ok(cache) = PROJECT_CACHE.lock() {
            if let Some((info, expires)) = cache.entries.get(cwd) {
                if *expires > Instant::now() {
                    return info.clone();
                }
            }
        }
    }

    let cwd_owned = cwd.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let info = resolve_project_blocking(&cwd_owned);
        let _ = tx.send(info);
    });
    let info = rx.await.unwrap_or_else(|_| ProjectInfo {
        project_root: cwd.to_string(),
        branch: None,
        is_worktree: false,
        is_top_level: false,
    });

    // 写缓存
    if let Ok(mut cache) = PROJECT_CACHE.lock() {
        cache.entries.insert(
            cwd.to_string(),
            (info.clone(), Instant::now() + PROJECT_CACHE_TTL),
        );
    }
    info
}

fn resolve_project_blocking(cwd: &str) -> ProjectInfo {
    if !Path::new(cwd).exists() {
        return infer_removed_worktree(cwd).unwrap_or(ProjectInfo {
            project_root: cwd.to_string(),
            branch: None,
            is_worktree: false,
            is_top_level: false,
        });
    }

    let out = match git(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
            "--show-toplevel",
            "--abbrev-ref",
            "HEAD",
        ],
    ) {
        Ok(s) => s,
        Err(_) => {
            return ProjectInfo {
                project_root: cwd.to_string(),
                branch: None,
                is_worktree: false,
                is_top_level: false,
            }
        }
    };

    let lines: Vec<&str> = out.lines().collect();
    if lines.len() < 4 {
        return ProjectInfo {
            project_root: cwd.to_string(),
            branch: None,
            is_worktree: false,
            is_top_level: false,
        };
    }

    let common_dir = lines[0].trim();
    let git_dir = lines[1].trim();
    let toplevel = lines[2].trim();
    let ref_name = lines[3].trim();

    let real_cwd = real_path_or_self(cwd);

    // 对齐 TS `samePath(toplevel, realCwd)` / `!samePath(gitDir, commonDir)`:
    // 大小写/分隔符不敏感比较(Windows 上尤为重要),而非裸字符串相等。
    let is_top_level = crate::paths::same_path(toplevel, &real_cwd);
    let is_worktree_top_level = !crate::paths::same_path(git_dir, common_dir) && is_top_level;

    ProjectInfo {
        project_root: if is_worktree_top_level {
            // 对齐 TS `realPathOrSelf(repoRoot)`(repoRoot = dirname(commonDir))。
            Path::new(common_dir)
                .parent()
                .map(|p| real_path_or_self(&p.to_string_lossy()))
                .unwrap_or_else(|| cwd.to_string())
        } else if is_top_level {
            // 对齐 TS `isTopLevel ? realPathOrSelf(topLevelProjectRoot) : cwd`。
            real_path_or_self(toplevel)
        } else {
            cwd.to_string()
        },
        branch: if ref_name != "HEAD" && !ref_name.is_empty() {
            Some(ref_name.to_string())
        } else {
            None
        },
        is_worktree: is_worktree_top_level,
        is_top_level,
    }
}

// ── worktree 操作 ────────────────────────────────────────────────────────

/// 对齐 `listWorktrees`。
pub async fn list_worktrees(cwd: &str) -> Result<Vec<WorktreeInfo>, String> {
    let cwd = cwd.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<WorktreeInfo>, String> {
            let out = git(&cwd, &["worktree", "list", "--porcelain"])?;
            let mut worktrees = Vec::new();
            let mut current_path: Option<String> = None;
            let mut current_branch: Option<String> = None;
            let mut prunable = false;

            let flush = |path: &mut Option<String>,
                         branch: &mut Option<String>,
                         prunable: &mut bool,
                         worktrees: &mut Vec<WorktreeInfo>| {
                if let Some(path) = path.take() {
                    if !*prunable && Path::new(&path).exists() {
                        worktrees.push(WorktreeInfo {
                            path: path.clone(),
                            branch: branch.take(),
                            is_main: worktrees.is_empty(),
                        });
                    }
                }
                *branch = None;
                *prunable = false;
            };

            for line in out.lines() {
                if let Some(rest) = line.strip_prefix("worktree ") {
                    flush(
                        &mut current_path,
                        &mut current_branch,
                        &mut prunable,
                        &mut worktrees,
                    );
                    current_path = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("branch ") {
                    current_branch = Some(
                        rest.trim()
                            .strip_prefix("refs/heads/")
                            .unwrap_or(rest.trim())
                            .to_string(),
                    );
                } else if line.trim() == "prunable" {
                    prunable = true;
                } else if line.trim().is_empty() {
                    flush(
                        &mut current_path,
                        &mut current_branch,
                        &mut prunable,
                        &mut worktrees,
                    );
                }
            }
            flush(
                &mut current_path,
                &mut current_branch,
                &mut prunable,
                &mut worktrees,
            );
            Ok(worktrees)
        })();
        let _ = tx.send(result);
    });
    rx.await.map_err(|_| "thread panicked".to_string())?
}

/// 对齐 `sanitizeBranchForDir`。
fn sanitize_branch_for_dir(branch: &str) -> String {
    // 对齐 TS `branch.replace(/[\/\\:*?"<>|\s]+/g, "-").replace(/^-+|-+$/g, "")`:
    // 连续特殊字符/空白折叠为单个 `-`,再剥离首尾连字符。
    let collapsed = SANITIZE_SPECIAL_RE.replace_all(branch, "-");
    SANITIZE_EDGE_RE.replace_all(&collapsed, "").to_string()
}

/// 对齐 `addWorktree`。
pub async fn add_worktree(cwd: &str, branch: &str) -> Result<(String, String), String> {
    let trimmed = branch.trim();
    if trimmed.is_empty() {
        return Err("Branch name is required".to_string());
    }
    let dir_name = sanitize_branch_for_dir(trimmed);
    if dir_name.is_empty() {
        return Err(format!("Invalid branch name: {branch}"));
    }

    let cwd = cwd.to_string();
    let dir_name_clone = dir_name.clone();
    let trimmed_clone = trimmed.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(String, String), String> {
            let common_dir = git(
                &cwd,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )?;
            // 对齐 TS getRepoRoot:`realPathOrSelf(dirname(toNativePath(commonDir)))`。
            let repo_root = Path::new(&common_dir)
                .parent()
                .ok_or("Cannot determine repo root")?
                .to_path_buf();
            let repo_root = PathBuf::from(real_path_or_self(&repo_root.to_string_lossy()));

            let base_dir = PathBuf::from(format!("{}-worktrees", repo_root.display()));
            let worktree_path = base_dir.join(&dir_name_clone);
            if worktree_path.exists() {
                return Err(format!(
                    "Directory already exists: {}",
                    worktree_path.display()
                ));
            }
            std::fs::create_dir_all(&base_dir).map_err(|e| e.to_string())?;

            // 检查分支是否已存在
            let branch_exists = git(
                &repo_root.to_string_lossy(),
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{trimmed_clone}"),
                ],
            )
            .is_ok();

            if branch_exists {
                git(
                    &repo_root.to_string_lossy(),
                    &[
                        "worktree",
                        "add",
                        "--",
                        worktree_path.to_str().unwrap(),
                        &trimmed_clone,
                    ],
                )?;
            } else {
                git(
                    &repo_root.to_string_lossy(),
                    &[
                        "worktree",
                        "add",
                        "-b",
                        &trimmed_clone,
                        "--",
                        worktree_path.to_str().unwrap(),
                    ],
                )?;
            }

            allow_file_root(worktree_path.to_str().unwrap());
            invalidate_project_cache();
            Ok((worktree_path.to_string_lossy().to_string(), trimmed_clone))
        })();
        let _ = tx.send(result);
    });
    let result = rx.await.map_err(|_| "thread panicked".to_string())??;
    Ok(result)
}

/// 对齐 `removeWorktree`。
pub async fn remove_worktree(cwd: &str, worktree_path: &str, force: bool) -> Result<(), String> {
    let worktrees = list_worktrees(cwd).await?;
    // 对齐 TS `findWorktreeByPath`:`samePath(worktree.path, candidate)`(大小写/分隔符不敏感)。
    let target = worktrees
        .iter()
        .find(|w| crate::paths::same_path(&w.path, worktree_path))
        .ok_or(format!(
            "Not a worktree of this repository: {worktree_path}"
        ))?;
    if target.is_main {
        return Err("Cannot remove the main worktree".to_string());
    }

    // 对齐 TS:把 git 的规范路径 `target.path` 传给 `git worktree remove`(而非原始输入)。
    let target_path = target.path.clone();
    let cwd = cwd.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&target_path);
        let result = git(&cwd, &args);
        if result.is_ok() {
            invalidate_project_cache();
        }
        let _ = tx.send(result.map(|_| ()));
    });
    rx.await.map_err(|_| "thread panicked".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_branch() {
        assert_eq!(
            sanitize_branch_for_dir("feature/foo bar"),
            "feature-foo-bar"
        );
        assert_eq!(sanitize_branch_for_dir("---leading"), "leading");
        assert_eq!(sanitize_branch_for_dir("clean"), "clean");
    }

    #[test]
    fn infer_removed() {
        // 正常情况不会触发,只验证逻辑不 panic
        let result = infer_removed_worktree("/nonexistent/path");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn resolve_non_git() {
        let info = resolve_project("/tmp/nonexistent-dir-pi-web-rust-test").await;
        assert!(!info.is_worktree);
        assert!(info.branch.is_none());
    }
}
