//! commands — ApiCore 命令处理器(路由表的执行面)。
//!
//! P1:分发框架 + home;P2 起按 docs/api-embed-plan.md §六 填充首批命令
//! (sessions/models/files/git/cwd/file-index),处理器内部调用 lib 模块
//! (session/fs/git/models)与引擎(trait 注入)。
//!
//! 执行纪律:阻塞 IO(git 子进程/session 扫描/大解析)必须经 [`blocking`]
//! 派发到注入运行时的 blocking pool(fs 模块内部已自异步化的调用可直接
//! await);禁止在 async 任务里裸跑同步 IO。

use std::sync::Arc;

use asupersync::runtime::Runtime;
use serde_json::{json, Value};

use super::routes::Dispatch;
use super::{ApiError, HostHooks};

/// 命令执行上下文(由 PiWebApi::handle 注入)。
pub(crate) struct ExecCtx {
    pub rt: Arc<Runtime>,
    pub hooks: Arc<dyn HostHooks>,
}/// 命令执行结果统一为 http::Response(传输方言直通;JSON/字节由命令自定)。
pub(crate) async fn execute(
    ctx: ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    match dispatch.command {
        "home" => home().await,
        "sessions_list" => sessions_list(&ctx).await,
        "sessions_get" => {
            let v = super::sessions::get_command(&ctx, dispatch).await?;
            json_response(v)
        }
        "sessions_context" => {
            let v = super::sessions::context_command(&ctx, dispatch).await?;
            json_response(v)
        }
        "cwd_browse" => cwd_browse(dispatch).await,
        "cwd_validate" => cwd_validate(dispatch).await,
        "default_cwd" => default_cwd(&ctx).await,
        "git_status" => git_status(&ctx, dispatch).await,
        "git_diff" => git_diff(&ctx, dispatch).await,
        #[cfg(test)]
        "test_sleep" => test_sleep(dispatch).await,
        #[cfg(test)]
        "test_panic" => panic!("test_panic: intentional"),
        #[cfg(test)]
        "test_bytes" => test_bytes().await,
        other => Err(ApiError::not_found(format!("unknown command: {other}"))),
    }
}

/// GET /api/home —— 返回用户主目录(对齐上游 app/api/home/route.ts)。
async fn home() -> Result<http::Response<Vec<u8>>, ApiError> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    json_response(json!({ "home": home }))
}

/// GET /api/sessions —— 会话列表(经 lib session::list_all_sessions,
/// 即 pi::sdk::SessionIndex 引擎链路;projectRoot 用真实 git worktree 解析,
/// 上游口径 —— 与 moho-mate 旧胶水的 no-op 解析差异见口径变化清单)。
async fn sessions_list(ctx: &ExecCtx) -> Result<http::Response<Vec<u8>>, ApiError> {
    let root = ctx
        .hooks
        .sessions_root()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(default_sessions_root);
    let infos = blocking(ctx, move || {
        // lib 的 resolve_project 是 async(内部 thread+oneshot+60s 缓存,纯
        // futures 无运行时依赖),而 list_all_sessions 注入同步闭包 —— 在
        // blocking pool 线程里用 futures executor 驱动(blocking 线程正为此设)。
        let resolve = |cwd: &str| {
            futures::executor::block_on(crate::git::worktree::resolve_project(cwd))
        };
        crate::session::list_all_sessions(&root, resolve)
    })
    .await?;
    let sessions: Vec<Value> = infos
        .iter()
        .map(|i| serde_json::to_value(i).unwrap_or(Value::Null))
        .collect();
    json_response(json!({ "sessions": sessions }))
}

/// 会话根目录默认值(对齐上游 getAgentDir):PI_CODING_AGENT_DIR/sessions
/// 或 ~/.pi/agent/sessions。宿主覆盖走 HostHooks::sessions_root。
pub(crate) fn default_sessions_root_pub() -> String {
    default_sessions_root()
}

fn default_sessions_root() -> String {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        let dir = dir.trim_end_matches('/');
        return format!("{dir}/sessions");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    format!("{home}/.pi/agent/sessions")
}

// ── cwd 三件 + git 两件(P2 批次:lib 直供) ──────────────────────────────

/// GET /api/cwd/browse?path= —— 目录浏览器(对齐上游 lib/directory-browser:
/// 仅列目录、软链解析、大小写不敏感排序、隐藏目录不过滤)。
/// lib 内部已 thread+oneshot 自异步化,直接 await。
async fn cwd_browse(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    use crate::fs::directory_browser as db;
    let raw = str_arg(&dispatch, "path");
    let base = if raw.is_empty() {
        db::get_browse_start_directory(None)
    } else {
        raw
    };
    let norm = db::normalize_directory(&base);
    if !norm.exists() {
        return Err(ApiError::not_found(format!("path does not exist: {}", norm.display())));
    }
    if !norm.is_dir() {
        return Err(ApiError::new(400, format!("not a directory: {}", norm.display())));
    }
    let resolved =
        db::resolve_directory(&base).await.map_err(|e| ApiError::internal(format!("canonicalize: {e}")))?;
    let dirs = db::list_directories(&resolved)
        .await
        .map_err(|e| ApiError::internal(format!("read_dir: {e}")))?;
    let directories: Vec<Value> =
        dirs.iter().map(|d| serde_json::to_value(d).unwrap_or(Value::Null)).collect();
    json_response(json!({
        "path": resolved,
        "parentPath": db::get_parent_directory(&resolved),
        "directories": directories,
    }))
}

/// POST /api/cwd/validate {cwd} —— 校验 + 规范化 + 加入 lib allowed roots
/// (替代 moho-mate 旧 ipc_security::add_root 的注入面)。
async fn cwd_validate(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    use crate::fs::directory_browser as db;
    let raw = str_arg(&dispatch, "cwd");
    if raw.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    let norm = db::normalize_directory(&raw);
    if !norm.exists() {
        return Err(ApiError::not_found(format!("path does not exist: {raw}")));
    }
    if !norm.is_dir() {
        return Err(ApiError::new(400, format!("not a directory: {raw}")));
    }
    let canon =
        db::resolve_directory(&raw).await.map_err(|e| ApiError::internal(format!("canonicalize: {e}")))?;
    crate::fs::allowed_roots::allow_file_root(&canon);
    json_response(json!({ "success": true, "cwd": canon }))
}

/// POST /api/default-cwd —— ~/pi-cwd-YYYY-MM-DD/(不存在则建),加入 roots。
async fn default_cwd(ctx: &ExecCtx) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = blocking(ctx, move || -> Result<String, ApiError> {
        let home = crate::paths::home_dir()
            .ok_or_else(|| ApiError::internal("cannot resolve home directory"))?;
        let stamp = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let dir = home.join(format!("pi-cwd-{stamp}"));
        std::fs::create_dir_all(&dir)
            .map_err(|e| ApiError::internal(format!("create default cwd: {e}")))?;
        let canon = dir
            .canonicalize()
            .map_err(|e| ApiError::internal(format!("canonicalize: {e}")))?;
        let canon_str = canon.to_string_lossy().into_owned();
        crate::fs::allowed_roots::allow_file_root(&canon_str);
        Ok(canon_str)
    })
    .await??;
    json_response(json!({ "cwd": cwd }))
}

/// roots 门禁(git/files 类命令),对齐上游 getAllowedFileRoots:
/// 全部会话的 cwd+projectRoot + ~/pi-cwd-* + 动态 additional。
/// 会话扫描经 blocking(文件 IO);roots 合成有 lib 侧 TTL 缓存(上游同款 5s)。
async fn gate_roots(ctx: &ExecCtx, cwd: &str) -> Result<(), ApiError> {
    let root = ctx
        .hooks
        .sessions_root()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(default_sessions_root);
    let session_roots = blocking(ctx, move || {
        let resolve = |cwd: &str| {
            futures::executor::block_on(crate::git::worktree::resolve_project(cwd))
        };
        crate::session::list_all_sessions(&root, resolve)
            .iter()
            .flat_map(|s| [Some(s.cwd.clone()), s.project_root.clone()])
            .flatten()
            .collect::<std::collections::HashSet<String>>()
    })
    .await?;
    let roots = crate::fs::file_access::get_allowed_file_roots_async(session_roots).await;
    if !crate::fs::path_security::is_path_within_roots(cwd, &roots) {
        return Err(ApiError::new(403, format!("access denied: {cwd}")));
    }
    Ok(())
}

/// GET /api/git/status?cwd= —— 经 lib git::changes(lib 内部自异步化)。
async fn git_status(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = str_arg(&dispatch, "cwd");
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    gate_roots(ctx, &cwd).await?;
    let resp = crate::git::changes::get_git_status(&cwd).await;
    json_response(
        serde_json::to_value(&resp).map_err(|e| ApiError::internal(format!("serialize: {e}")))?,
    )
}

/// GET /api/git/diff?cwd=&path= —— lib 真实现(旧 moho 实现为
/// {"supported": false} 未实现 —— 属口径变化清单项:切换后前端可见真 diff)。
async fn git_diff(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = str_arg(&dispatch, "cwd");
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    let path = str_arg(&dispatch, "path");
    if path.is_empty() {
        return Err(ApiError::new(400, "path is required"));
    }
    gate_roots(ctx, &cwd).await?;
    let resp = crate::git::changes::get_git_file_diff(&cwd, &path).await;
    json_response(
        serde_json::to_value(&resp).map_err(|e| ApiError::internal(format!("serialize: {e}")))?,
    )
}

fn str_arg(dispatch: &Dispatch, key: &str) -> String {
    dispatch
        .args
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

// ── helpers ─────────────────────────────────────────────────────────────

/// 阻塞派发:把闭包丢到注入运行时的 blocking pool,异步侧 await 结果。
/// asupersync 的 spawn_blocking 闭包无返回值(FnOnce()),经 futures oneshot 回收。
pub(crate) async fn blocking<T: Send + 'static>(
    ctx: &ExecCtx,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let mut tx = Some(tx);
    let spawned = ctx.rt.spawn_blocking(move || {
        if let Some(tx) = tx.take() {
            let _ = tx.send(f());
        }
    });
    if spawned.is_none() {
        return Err(ApiError::internal("blocking pool unavailable"));
    }
    rx.await.map_err(|_| ApiError::internal("blocking worker dropped"))
}

pub(crate) fn json_response(body: Value) -> Result<http::Response<Vec<u8>>, ApiError> {
    let body = serde_json::to_vec(&body)
        .map_err(|e| ApiError::internal(format!("serialize response: {e}")))?;
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("static response builder"))
}

// ── 测试专用命令(exactly-once / 超时 / panic 路径的测试靶) ─────────────

#[cfg(test)]
async fn test_sleep(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let ms = dispatch
        .args
        .get("ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000);
    asupersync::time::sleep(asupersync::time::wall_now(), std::time::Duration::from_millis(ms)).await;
    json_response(json!({ "slept": ms }))
}

#[cfg(test)]
async fn test_bytes() -> Result<http::Response<Vec<u8>>, ApiError> {
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, "application/octet-stream")
        .body(vec![0u8, 1, 2, 250, 255])
        .expect("static response builder"))
}
