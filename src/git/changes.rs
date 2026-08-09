//! 对齐 `lib/git-changes.ts`。
//!
//! git status/diff 命令执行 + 结果聚合。async + std::thread(运行时无关)。
//! 调用 git CLI 子进程(std::process::Command)。

use std::path::{Path, PathBuf};
use std::process::Command;

use super::status::{parse_git_porcelain_v1, classify_git_status, GitPorcelainEntry};
use super::types::{
    GitFileStatus, GitFileStatusKind, GitStatusCode, GitStatusResponse, GitFileDiffResponse,
};
use crate::file::types::TEXT_PREVIEW_MAX_BYTES;

const GIT_TIMEOUT_SECS: u64 = 10;
const GIT_STATUS_MAX_BUFFER: usize = 8 * 1024 * 1024;

/// 对齐 `git(cwd, args)`。跑 `git -C <cwd> <args>`,LC_ALL=C,10s 超时。
/// 返回 stdout;失败返回 Err。
fn git_exec(cwd: &str, args: &[&str], max_buffer: usize) -> std::io::Result<String> {
    let full_args: Vec<&str> = std::iter::once("-C")
        .chain(std::iter::once(cwd))
        .chain(args.iter().copied())
        .collect();
    let output = Command::new("git")
        .args(&full_args)
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("git {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&output.stderr)),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.len() > max_buffer {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "git output exceeds max buffer",
        ));
    }
    Ok(stdout)
}

/// 对齐 `findRepositoryRoot`。
pub async fn find_repository_root(cwd: &str) -> Option<String> {
    let cwd = cwd.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = git_exec(&cwd, &["rev-parse", "--show-toplevel"], 1024)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let _ = tx.send(result);
    });
    rx.await.ok().flatten()
}

/// 对齐 `isWithinPath`。
fn is_within_path(parent: &str, target: &str) -> bool {
    let parent = Path::new(parent).canonicalize().unwrap_or_else(|_| PathBuf::from(parent));
    let target = Path::new(target).canonicalize().unwrap_or_else(|_| PathBuf::from(target));
    target == parent || target.starts_with(&parent)
}

/// 对齐 `toGitPath`。
fn to_git_path(file_path: &str) -> String {
    file_path.replace('\\', "/")
}

/// 对齐 `readStatusEntries`。
async fn read_status_entries(repository_root: &str) -> Vec<GitPorcelainEntry> {
    let root = repository_root.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = git_exec(
            &root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            GIT_STATUS_MAX_BUFFER,
        )
        .map(|output| parse_git_porcelain_v1(&output))
        .unwrap_or_default();
        let _ = tx.send(result);
    });
    rx.await.unwrap_or_default()
}

/// 对齐 `readTrackedLineStats`。
async fn read_tracked_line_stats(
    repository_root: &str,
    cwd: &str,
) -> (u64, u64) {
    let root = repository_root.to_string();
    let relative_cwd = Path::new(cwd)
        .strip_prefix(repository_root)
        .map(|p| to_git_path(&p.to_string_lossy()))
        .unwrap_or_default();
    let pathspec = if relative_cwd.is_empty() { "." } else { &relative_cwd };
    let pathspec = pathspec.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = git_exec(
            &root,
            &["diff", "--no-color", "--no-ext-diff", "--numstat", "HEAD", "--", &pathspec],
            GIT_STATUS_MAX_BUFFER,
        )
        .map(|output| {
            let mut additions = 0u64;
            let mut deletions = 0u64;
            for line in output.lines() {
                let parts: Vec<&str> = line.splitn(2, '\t').collect();
                if parts.len() < 2 { continue; }
                if let Ok(n) = parts[0].parse::<u64>() { additions += n; }
                if let Some(rest) = parts.get(1).and_then(|s| s.split('\t').next()) {
                    if let Ok(n) = rest.parse::<u64>() { deletions += n; }
                }
            }
            (additions, deletions)
        })
        .unwrap_or((0, 0));
        let _ = tx.send(result);
    });
    rx.await.unwrap_or((0, 0))
}

/// 对齐 `countUntrackedTextLines`。
fn count_untracked_text_lines(file_path: &str) -> u64 {
    let path = Path::new(file_path);
    let Ok(meta) = std::fs::metadata(path) else { return 0; };
    if !meta.is_file() || meta.len() as usize > TEXT_PREVIEW_MAX_BYTES {
        return 0;
    }
    let Ok(content) = std::fs::read(path) else { return 0; };
    if content.contains(&0u8) || content.is_empty() { return 0; }
    let text = String::from_utf8_lossy(&content);
    if text.ends_with('\n') {
        text.lines().count() as u64
    } else {
        text.lines().count() as u64
    }
}

/// 对齐 `getGitStatus`。完整 git status 响应。
pub async fn get_git_status(cwd: &str) -> GitStatusResponse {
    let repository_root = match find_repository_root(cwd).await {
        Some(root) => root,
        None => {
            return GitStatusResponse {
                is_git_repository: false,
                repository_root: None,
                files: vec![],
                additions: 0,
                deletions: 0,
            };
        }
    };

    let entries = read_status_entries(&repository_root).await;
    let tracked_stats = read_tracked_line_stats(&repository_root, cwd).await;

    let files: Vec<GitFileStatus> = entries
        .iter()
        .filter_map(|entry| {
            let file_path = Path::new(&repository_root)
                .join(&entry.path)
                .to_string_lossy()
                .to_string();
            if !is_within_path(cwd, &file_path) {
                return None;
            }
            let (status, code) = classify_git_status(entry);
            Some(GitFileStatus {
                file_path,
                status,
                code,
                index_status: entry.index_status.clone(),
                worktree_status: entry.worktree_status.clone(),
            })
        })
        .collect();

    let untracked_additions: u64 = files
        .iter()
        .filter(|f| f.status == GitFileStatusKind::Untracked)
        .map(|f| count_untracked_text_lines(&f.file_path))
        .sum();

    GitStatusResponse {
        is_git_repository: true,
        repository_root: Some(repository_root),
        files,
        additions: tracked_stats.0 + untracked_additions,
        deletions: tracked_stats.1,
    }
}

/// 对齐 `createAddedFilePatch`。
fn create_added_file_patch(git_path: &str, content: &str) -> String {
    let has_trailing = content.ends_with('\n');
    let mut lines: Vec<&str> = content.lines().collect();
    if has_trailing && content.ends_with('\n') && !content.is_empty() {
        // lines() 已经不包含最后的空行
    }
    let body: String = lines.iter().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n");
    let no_newline = if !has_trailing && !lines.is_empty() {
        "\n\\ No newline at end of file"
    } else {
        ""
    };
    format!(
        "diff --git a/{git_path} b/{git_path}\nnew file mode 100644\n--- /dev/null\n+++ b/{git_path}\n@@ -0,0 +1,{} @@\n{body}{no_newline}",
        lines.len()
    )
}

/// 对齐 `createTrackedFilePatch`。
async fn create_tracked_file_patch(
    repository_root: &str,
    relative_path: &str,
    original_path: Option<&str>,
) -> Option<String> {
    let root = repository_root.to_string();
    let paths: Vec<String> = match original_path {
        Some(orig) if orig != relative_path => vec![orig.to_string(), relative_path.to_string()],
        _ => vec![relative_path.to_string()],
    };
    let paths_arg: Vec<String> = paths.iter().map(|s| s.as_str().to_string()).collect();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let args: Vec<&str> = ["diff", "--no-color", "--no-ext-diff", "--unified=3", "HEAD", "--"]
            .into_iter()
            .chain(paths_arg.iter().map(|s| s.as_str()))
            .collect();
        let result = git_exec(&root, &args, TEXT_PREVIEW_MAX_BYTES * 4).ok();
        let _ = tx.send(result);
    });
    rx.await.ok().flatten()
}

/// 对齐 `getGitFileDiff`。
pub async fn get_git_file_diff(cwd: &str, file_path: &str) -> GitFileDiffResponse {
    let repository_root = match find_repository_root(cwd).await {
        Some(root) => root,
        None => return GitFileDiffResponse { supported: false, status: None, patch: None },
    };
    if !is_within_path(&repository_root, file_path) {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    }

    let resolved = PathBuf::from(file_path);
    let relative_path = resolved
        .strip_prefix(&repository_root)
        .map(|p| to_git_path(&p.to_string_lossy()))
        .unwrap_or_default();

    let entries = read_status_entries(&repository_root).await;
    let entry = entries.iter().find(|e| e.path == relative_path);
    let Some(entry) = entry else {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    };

    let (status, _code) = classify_git_status(entry);

    if status == GitFileStatusKind::Deleted {
        let patch = create_tracked_file_patch(&repository_root, &relative_path, entry.original_path.as_deref()).await;
        match patch {
            Some(p) if p.contains("\n@@ ") => {
                return GitFileDiffResponse { supported: true, status: Some(status), patch: Some(p) };
            }
            _ => return GitFileDiffResponse { supported: false, status: None, patch: None },
        }
    }

    // 读当前文件内容
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    };
    if !meta.is_file() || meta.len() as usize > TEXT_PREVIEW_MAX_BYTES {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    }
    let Ok(content_buf) = std::fs::read(&resolved) else {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    };
    if content_buf.contains(&0u8) {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    }
    let new_content = String::from_utf8_lossy(&content_buf).to_string();

    let patch = if status == GitFileStatusKind::Untracked {
        create_added_file_patch(&relative_path, &new_content)
    } else {
        match create_tracked_file_patch(&repository_root, &relative_path, entry.original_path.as_deref()).await {
            Some(tracked) => tracked,
            None => {
                if status != GitFileStatusKind::Added {
                    return GitFileDiffResponse { supported: false, status: None, patch: None };
                }
                create_added_file_patch(&relative_path, &new_content)
            }
        }
    };

    if !patch.contains("\n@@ ") {
        return GitFileDiffResponse { supported: false, status: None, patch: None };
    }
    GitFileDiffResponse { supported: true, status: Some(status), patch: Some(patch) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_file_patch() {
        let patch = create_added_file_patch("test.rs", "line1\nline2\n");
        assert!(patch.contains("diff --git a/test.rs b/test.rs"));
        assert!(patch.contains("new file mode 100644"));
        assert!(patch.contains("+line1"));
        assert!(patch.contains("+line2"));
    }

    #[test]
    fn added_file_patch_no_trailing_newline() {
        let patch = create_added_file_patch("test.rs", "line1");
        assert!(patch.contains("\\ No newline at end of file"));
    }

    #[tokio::test]
    async fn status_not_a_repo() {
        // /tmp 在大多系统上不是 git repo(可能是,但通常是)
        let resp = get_git_status("/tmp/nonexistent-dir-for-pi-web-rust-test").await;
        assert!(!resp.is_git_repository);
    }
}
