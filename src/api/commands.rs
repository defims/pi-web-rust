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
}

/// 命令执行结果统一为 http::Response(传输方言直通;JSON/字节由命令自定)。
pub(crate) async fn execute(
    ctx: ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    match dispatch.command {
        "home" => home().await,
        "sessions_list" => sessions_list(&ctx).await,
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
fn default_sessions_root() -> String {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        let dir = dir.trim_end_matches('/');
        return format!("{dir}/sessions");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    format!("{home}/.pi/agent/sessions")
}

// ── helpers ─────────────────────────────────────────────────────────────

/// 阻塞派发:把闭包丢到注入运行时的 blocking pool,异步侧 await 结果。
/// asupersync 的 spawn_blocking 闭包无返回值(FnOnce()),经 futures oneshot 回收。
async fn blocking<T: Send + 'static>(
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
