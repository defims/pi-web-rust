//! worktrees — GET/POST/DELETE /api/worktrees(上游 app/api/worktrees/route.ts 移植)。
//!
//! lib `git::worktree`(resolve_project/list/add/remove)全量承接;本模块只做
//! 门禁(与 /api/files 同款:路径在 roots 内 + 实路径也在 roots 内)、
//! 参数校验与响应组装。GET 会把列出的 worktree 路径加入文件 roots(上游
//! 同款:文件浏览器可在会话建立前浏览它们)。

use serde_json::json;

use super::ApiError;
use super::commands::ExecCtx;
use super::routes::Dispatch;

/// 上游 checkCwdAllowed:isFilePathAllowed && isExistingFilePathAllowed。
/// 我们的 gate_roots 只做前者(路径在 roots 内);此处补实路径检查
/// (canonicalize 后仍在 roots 内)。
async fn check_cwd_allowed(ctx: &ExecCtx, cwd: &str) -> Result<(), ApiError> {
    super::commands::gate_roots(ctx, cwd).await?;
    let roots = crate::fs::file_access::get_allowed_file_roots_async(collect_session_roots(ctx).await).await;
    if !crate::fs::path_security::is_existing_path_within_roots(cwd, &roots).await {
        return Err(ApiError::new(403, "Access denied"));
    }
    Ok(())
}

/// 与 commands::gate_roots 同源的 roots 合成(会话 cwd/projectRoot 全集)。
async fn collect_session_roots(ctx: &ExecCtx) -> std::collections::HashSet<String> {
    let root = ctx
        .hooks
        .sessions_root()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(super::commands::default_sessions_root_pub);
    super::commands::blocking(ctx, move || {
        let resolve = |cwd: &str| futures::executor::block_on(crate::git::worktree::resolve_project(cwd));
        crate::session::list_all_sessions(&root, resolve)
            .iter()
            .flat_map(|s| [Some(s.cwd.clone()), s.project_root.clone()])
            .flatten()
            .collect::<std::collections::HashSet<String>>()
    })
    .await
    .unwrap_or_default()
}

/// GET /api/worktrees?cwd= → {projectRoot, isGit, isTopLevel, currentWorktreePath, worktrees}
pub(crate) async fn get(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    check_cwd_allowed(ctx, &cwd).await?;

    let project = crate::git::worktree::resolve_project(&cwd).await;
    // 已删除 worktree 的 cwd → 回退到推导的项目根(切换器仍能展示项目)
    let list_target = if std::path::Path::new(&cwd).exists() { cwd.clone() } else { project.project_root.clone() };
    let (worktrees, is_git) = match crate::git::worktree::list_worktrees(&list_target).await {
        Ok(w) => (w, true),
        Err(_) => (Vec::new(), false),
    };
    let current = find_current_worktree_path(&worktrees, &cwd);
    // 上游同款:每个列出路径都经 git 验证属于本项目 → 加入 roots
    for w in &worktrees {
        crate::fs::allowed_roots::allow_file_root(&w.path);
    }
    super::commands::json_response(json!({
        "projectRoot": project.project_root,
        "isGit": is_git,
        "isTopLevel": project.is_top_level,
        "currentWorktreePath": current,
        "worktrees": worktrees,
    }))
}

/// 上游 findCurrentWorktreePath:realpath(cwd) 后与 worktree.path 同路径比对。
fn find_current_worktree_path(worktrees: &[crate::git::worktree::WorktreeInfo], cwd: &str) -> Option<String> {
    let real = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| cwd.to_string());
    let real_meta = std::path::Path::new(&real);
    worktrees.iter().find_map(|w| {
        let w_real = std::fs::canonicalize(&w.path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| w.path.clone());
        if std::path::Path::new(&w_real) == real_meta {
            Some(w.path.clone())
        } else {
            None
        }
    })
}

/// POST /api/worktrees body {cwd, branch} → {path, branch};失败 400
pub(crate) async fn post(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let branch = dispatch.args.get("branch").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    if branch.is_empty() {
        return Err(ApiError::new(400, "branch is required"));
    }
    check_cwd_allowed(ctx, &cwd).await?;
    if !std::path::Path::new(&cwd).exists() {
        return Err(ApiError::new(400, format!("Directory does not exist: {cwd}")));
    }
    match crate::git::worktree::add_worktree(&cwd, &branch).await {
        Ok((path, branch)) => super::commands::json_response(json!({ "path": path, "branch": branch })),
        Err(message) => Err(ApiError::new(400, message)),
    }
}

/// DELETE /api/worktrees body {cwd, path, force?} → {success};脏 worktree 409 带 dirty
pub(crate) async fn delete(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let path = dispatch.args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let force = dispatch.args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    if path.is_empty() {
        return Err(ApiError::new(400, "path is required"));
    }
    check_cwd_allowed(ctx, &cwd).await?;
    match crate::git::worktree::remove_worktree(&cwd, &path, force).await {
        Ok(()) => super::commands::json_response(json!({ "success": true })),
        Err(message) => {
            // git 拒绝无 --force 删脏 worktree → 409 + dirty(UI 据此弹强制删除确认)
            let dirty = message.contains("contains modified or untracked files") || message.contains("is dirty");
            if dirty {
                let body = serde_json::json!({ "error": message, "dirty": true });
                let body = serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?;
                return Ok(http::Response::builder()
                    .status(409)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .expect("static builder"));
            }
            Err(ApiError::new(400, message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct SessionsRoot(std::path::PathBuf);
    impl HostHooks for SessionsRoot {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
    }

    fn api_with_root(root: &std::path::Path) -> PiWebApi {
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(SessionsRoot(root.to_path_buf()));
        PiWebApi::new(rt, cfg)
    }

    fn call(
        api: &PiWebApi,
        req: http::Request<Vec<u8>>,
    ) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn body(v: &http::Response<Vec<u8>>) -> serde_json::Value {
        serde_json::from_slice(v.body()).unwrap_or(serde_json::Value::Null)
    }

    #[test]
    fn worktrees_get_post_delete_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        // 真 git 仓(一个初始提交,branch -M main 保证分支名确定)
        git(tmp.path(), &["init", "-q"]);
        std::fs::write(tmp.path().join("README.md"), "x").unwrap();
        git(tmp.path(), &["add", "."]);
        git(tmp.path(), &["commit", "-q", "-m", "init"]);
        git(tmp.path(), &["branch", "-M", "main"]);

        let sessions = tempfile::tempdir().unwrap();
        let api = api_with_root(sessions.path());
        crate::fs::allowed_roots::allow_file_root(&tmp.path().to_string_lossy());
        let cwd = tmp.path().to_string_lossy().to_string();

        // GET:单 worktree(主仓),current 命中
        let resp = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri(format!("/api/worktrees?cwd={}", cwd))
                .body(Vec::new())
                .unwrap(),
        )
        .expect("get");
        assert_eq!(resp.status(), 200);
        let v = body(&resp);
        assert_eq!(v["isGit"], serde_json::json!(true));
        let worktrees = v["worktrees"].as_array().unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0]["isMain"], serde_json::json!(true));
        let cwd_real = std::fs::canonicalize(&cwd).unwrap().to_string_lossy().into_owned();
        assert_eq!(v["currentWorktreePath"].as_str().unwrap().trim_end_matches('/'), cwd_real.trim_end_matches('/'));

        // POST:建 worktree → {path, branch};再 GET 出现两条,current 不变
        let resp = call(
            &api,
            http::Request::builder()
                .method("POST")
                .uri("/api/worktrees")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(format!(r#"{{"cwd":"{cwd}","branch":"wt-one"}}"#).into_bytes())
                .unwrap(),
        )
        .expect("post");
        assert_eq!(resp.status(), 200, "body: {}", body(&resp));
        let added = body(&resp)["path"].as_str().expect("path").to_string();
        assert!(added.contains("wt-one"), "worktree dir from branch: {added}");

        let resp = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri(format!("/api/worktrees?cwd={}", cwd))
                .body(Vec::new())
                .unwrap(),
        )
        .expect("get2");
        let v = body(&resp);
        assert_eq!(v["worktrees"].as_array().unwrap().len(), 2);

        // DELETE:body 传参({cwd, path})→ success;再 GET 回一条
        let resp = call(
            &api,
            http::Request::builder()
                .method("DELETE")
                .uri("/api/worktrees")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(format!(r#"{{"cwd":"{cwd}","path":"{added}"}}"#).into_bytes())
                .unwrap(),
        )
        .expect("delete");
        assert_eq!(resp.status(), 200, "body: {}", body(&resp));
        let resp = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri(format!("/api/worktrees?cwd={}", cwd))
                .body(Vec::new())
                .unwrap(),
        )
        .expect("get3");
        assert_eq!(body(&resp)["worktrees"].as_array().unwrap().len(), 1);

        // 门禁:roots 外 → 403
        let e = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri("/api/worktrees?cwd=/etc")
                .body(Vec::new())
                .unwrap(),
        )
        .unwrap_err();
        assert_eq!(e.status, 403);
    }
}
