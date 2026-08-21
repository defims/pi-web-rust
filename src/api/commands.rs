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
    pub sessions: Arc<super::session_runtime::SessionRuntime>,
    pub sink: super::EventSink,
}/// 命令执行结果统一为 http::Response(传输方言直通;JSON/字节由命令自定)。
pub(crate) async fn execute(
    ctx: ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    match dispatch.command {
        "gated_skills_get" => super::models::skills_get(&ctx, dispatch).await,
        "gated_skills_patch" => super::models::skills_patch(&ctx, dispatch).await,
        "gated_skills_search" => super::models::skills_search(&ctx, dispatch).await,
        "gated_skills_check" => super::models::skills_check(&ctx, dispatch).await,
        "gated_plugins_get" => super::models::plugins_get(&ctx, dispatch).await,
        "gated_plugins_post" => super::models::plugins_post(&ctx, dispatch).await,
        "gated_auth_providers" => json_response(json!({ "providers": [] })),
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
        "sessions_rename" => {
            let v = super::sessions::rename_command(&ctx, dispatch).await?;
            json_response(v)
        }
        "sessions_delete" => {
            let v = super::sessions::delete_command(&ctx, dispatch).await?;
            json_response(v)
        }
        "sessions_auto_name" => {
            let v = super::sessions::auto_name_command(&ctx, dispatch).await?;
            json_response(v)
        }
        "sessions_thinking" => {
            let v = super::sessions::thinking_command(&ctx, dispatch).await?;
            json_response(v)
        }
        "cwd_browse" => cwd_browse(dispatch).await,
        "cwd_validate" => cwd_validate(dispatch).await,
        "default_cwd" => default_cwd(&ctx).await,
        "git_status" => git_status(&ctx, dispatch).await,
        "git_diff" => git_diff(&ctx, dispatch).await,
        "files" => super::files::files_command(&ctx, dispatch).await,
        "file_index" => super::file_index::file_index_command(&ctx, dispatch).await,
        "models_list" => super::models::models_list(&ctx, dispatch).await,
        "models_config_get" => super::models::models_config_get(&ctx).await,
        "models_config_discover" => super::models::models_config_discover(&ctx, dispatch).await,
        "models_config_test" => super::models::models_config_test(&ctx, dispatch).await,
        "models_config_put" => super::models::models_config_put(&ctx, dispatch).await,
        "agent_new" => agent_new(&ctx, dispatch).await,
        "agent_running" => agent_running(&ctx),
        "agent_get_state" => agent_get_state(&ctx, dispatch),
        "agent_bash_output" => agent_bash_output(dispatch).await,
        "project_trust_get" => project_trust_get(dispatch).await,
        "project_trust_set" => project_trust_set(dispatch).await,
        "models_config_catalog" => models_config_catalog(&ctx, dispatch).await,
        "agent_rpc" => agent_rpc(&ctx, dispatch).await,
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
    // runningSessionIds:前端 SessionSidebar 消费(列表 + running 轮询双处);
    // P2 首版遗漏,calibration golden 对拍抓出 —— 与 agent_running 同源
    json_response(json!({
        "sessions": sessions,
        "runningSessionIds": ctx.sessions.running_ids(),
    }))
}

/// system prompt 构造(SetTools 非空列表时重建带工具描述的 prompt;
/// 宿主经 HostHooks::system_prompt 定制,会话参数留位)。
pub(crate) fn system_prompt_pub(_session_id: Option<&str>) -> String {
    // 默认空 —— 宿主(P4)moho 经 HostHooks::system_prompt 注入 skills prompt
    String::new()
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
pub(crate) async fn gate_roots(ctx: &ExecCtx, cwd: &str) -> Result<(), ApiError> {
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

// ── agent 面(P3:会话 runtime 接线) ─────────────────────────────────────

/// POST /api/agent/new —— 建会话(溢出 = 关旧建新)。
/// body: {cwd, provider?, modelId?, thinkingLevel?, toolNames?}
async fn agent_new(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = str_arg(&dispatch, "cwd");
    let cwd = if cwd.is_empty() {
        crate::paths::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string())
    } else {
        cwd
    };
    let provider = dispatch.args.get("provider").and_then(|v| v.as_str()).map(String::from);
    let model = dispatch.args.get("modelId").and_then(|v| v.as_str()).map(String::from);
    // 成对校验(对齐上游 route.ts:49-51):半配对属契约错误,400。
    match (provider.as_deref(), model.as_deref()) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(ApiError::new(400, "provider and modelId must be provided together"));
        }
        _ => {}
    }
    let thinking = dispatch.args.get("thinkingLevel").and_then(|v| v.as_str()).map(String::from);
    let tools = dispatch.args.get("toolNames").and_then(|v| v.as_array()).map(|a| {
        a.iter().filter_map(|t| t.as_str().map(String::from)).collect::<Vec<_>>()
    });
    let (session_id, eng_provider, eng_model, eng_thinking) = ctx
        .sessions
        .create(ctx, &cwd, provider, model, thinking, tools)
        .await?;
    // 响应回传引擎实际选中(对齐上游 route.ts:70-84:前端 setNewSessionDefaultModel
    // /setThinkingLevel 消费,消灭"UI 显示 A、引擎用 B")
    json_response(json!({
        "sessionId": session_id,
        "model": match (eng_provider, eng_model) {
            (Some(p), Some(m)) => json!({ "provider": p, "modelId": m }),
            _ => Value::Null,
        },
        "thinkingLevel": eng_thinking,
    }))
}

/// GET /api/agent/running —— {runningSessionIds}(侧栏轮询面)。
fn agent_running(ctx: &ExecCtx) -> Result<http::Response<Vec<u8>>, ApiError> {
    json_response(json!({ "runningSessionIds": ctx.sessions.running_ids() }))
}

/// GET /api/agent/:id —— {running, state}(挂载恢复/对账切片)。
/// 死会话回 {running:false}(对齐上游 route.ts:64-67:wrapper 不存在/已死
/// 直接 running:false,不 404 —— 前端 15s 对账靠它收敛,否则 loading 永转)。
fn agent_get_state(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let id = str_arg(&dispatch, "id");
    let Some(h) = ctx.sessions.get(&id) else {
        return json_response(json!({ "running": false }));
    };
    let snap = h.snap.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let running = snap.is_streaming || snap.is_prompt_running || snap.is_compacting;

    // contextUsage(上游 getContextUsage 等价):snap 缓存的 usage tokens
    // vs 模型 context_window 的百分比。tokens 由 finish_turn 每轮更新。
    let tokens = snap.last_usage_total;
    let provider = snap.model_provider.clone().unwrap_or_default();
    let model_id = snap.model_id.clone().unwrap_or_default();
    let context_usage = compute_context_usage(tokens, &provider, &model_id);

    let mut state = snap.to_state_json();
    if let Some(obj) = state.as_object_mut() {
        obj.insert("contextUsage".to_string(), context_usage);
    }
    json_response(json!({ "running": running, "state": state }))
}

/// tokens + models.json 的 contextWindow → {tokens, contextWindow, percent};
/// 任一缺失 → null(上游压缩后 percent 可 null 同款语义)。
fn compute_context_usage(tokens: u64, provider: &str, model_id: &str) -> Value {
    if tokens == 0 || provider.is_empty() || model_id.is_empty() {
        return Value::Null;
    }
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent")))
        .unwrap_or_default();
    let cfg = crate::fs::models_config_store::read_models_config(&agent_dir.join("models.json"));
    let cw = cfg.get("providers").and_then(|p| p.get(provider))
        .and_then(|p| p.get("models")).and_then(|m| m.as_array())
        .and_then(|arr| arr.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(model_id)))
        .and_then(|m| m.get("contextWindow")).and_then(|v| v.as_u64())
        .unwrap_or(0);
    if cw == 0 {
        return Value::Null;
    }
    json!({
        "tokens": tokens,
        "contextWindow": cw,
        "percent": (tokens as f64 / cw as f64) * 100.0,
    })
}


/// POST /api/agent/:id —— 25-case RPC(body {type: ...})。
/// 信封:{success:true,data} / 500 {error}(对齐上游 route.ts)。
async fn agent_rpc(ctx: &ExecCtx, dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let id = str_arg(&dispatch, "id");
    let ty = dispatch
        .args
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // 死会话惰性恢复(对齐上游 route.ts:19-32:resolveSessionPath →
    // startRpcSession(id, path) 后再执行命令;磁盘也找不到才 404)
    let mut h = match ctx.sessions.get(&id) {
        Some(h) => h,
        None => {
            if !ctx.sessions.restore(ctx, &id).await {
                // 对齐上游 route.ts:28-33:prompt 被拒带 prompt_rejected 信封
                return reject_session_response(&ty);
            }
            ctx.sessions
                .get(&id)
                .ok_or_else(|| ApiError::not_found("Session not found"))?
        }
    };
    let message = dispatch.args.get("message").and_then(|v| v.as_str()).map(String::from);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    use super::session_runtime::SessionCmd as C;
    let cmd = match ty.as_str() {
        // streamingBehavior:"steer" → 等价 steer 队列语义(对齐 rpc-manager;
        // 运行期 steer 入镜像,turn 结束被消费)
        "prompt" if dispatch.args.get("streamingBehavior").and_then(|v| v.as_str()) == Some("steer") => {
            C::Steer { message: message.unwrap_or_default(), reply: reply_tx }
        }
        "prompt" => {
            // images: [{data: base64, mimeType}] → 引擎 ImageContent
            // (上游 rpc-manager.ts:397-400 校验后透传;此处 serde 直转)
            let images = dispatch
                .args
                .get("images")
                .and_then(|v| serde_json::from_value::<Vec<pi::sdk::ImageContent>>(v.clone()).ok());
            C::Prompt {
                message: message.unwrap_or_default(),
                images,
                reply: reply_tx,
            }
        }
        "steer" => C::Steer { message: message.unwrap_or_default(), reply: reply_tx },
        "follow_up" => C::FollowUp { message: message.unwrap_or_default(), reply: reply_tx },
        "abort" => {
            // abort 无回执语义(fire-and-forget;对齐上游 immediate {})
            let _ = h.tx.try_send(C::Abort);
            return json_response(json!({}));
        }
        "clear_queue" => C::ClearQueue { reply: reply_tx },
        "set_model" => C::SetModel {
            provider: str_arg(&dispatch, "provider"),
            model: str_arg(&dispatch, "modelId"),
            reply: reply_tx,
        },
        "set_thinking_level" => C::SetThinking {
            level: str_arg(&dispatch, "level"),
            reply: reply_tx,
        },
        "set_session_name" => C::SetSessionName {
            name: str_arg(&dispatch, "name"),
            reply: reply_tx,
        },
        "compact" => C::Compact { reply: reply_tx },
        "abort_compaction" => {
            // 空闲无压缩可 abort;运行期 compact 阻塞在 idle(引擎 compact 同步
            // 完成) —— 无 in-flight 压缩,no-op 对齐上游
            let _ = reply_tx.send(Ok(json!({})));
            return json_response(json!({ "success": true, "data": {} }));
        }
        "set_tools" => {
            let names = dispatch
                .args
                .get("toolNames")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();
            C::SetTools { names, reply: reply_tx }
        }
        "navigate_tree" => C::NavigateTree {
            target_id: str_arg(&dispatch, "targetId"),
            reply: reply_tx,
        },
        "fork" => C::Fork {
            entry_id: dispatch.args.get("entryId").and_then(|v| v.as_str()).map(String::from),
            reply: reply_tx,
        },
        "reload" => C::Reload { reply: reply_tx },
        "bash" => C::Bash { command: str_arg(&dispatch, "command"), reply: reply_tx },
        "abort_bash" => C::AbortBash { reply: reply_tx },
        "get_tools" => C::GetTools { reply: reply_tx },
        "get_commands" => C::GetCommands { reply: reply_tx },
        "extension_ui_response" => C::ExtensionUiResponse {
            id: str_arg(&dispatch, "id"),
            body: dispatch.args.clone(),
            reply: reply_tx,
        },
        "extension_ui_input" => C::ExtensionUiInput {
            id: str_arg(&dispatch, "id"),
            data: str_arg(&dispatch, "data"),
            reply: reply_tx,
        },
        "get_session_stats" => C::GetStats { reply: reply_tx },
        "get_last_assistant_text" => C::GetLastText { reply: reply_tx },
        other => {
            return Err(ApiError::new(400, format!("unknown rpc type: {other}")));
        }
    };
    if h.tx.send(cmd).await.is_err() {
        return reject_session_response(&ty);
    }
    match reply_rx.await {
        Ok(Ok(data)) => json_response(json!({ "success": true, "data": data })),
        Ok(Err(e)) => {
            // 上游 route.ts:41-47:prompt 失败(未 accepted)带 prompt_rejected
            let mut body = json!({ "error": e });
            if ty == "prompt" {
                body["code"] = json!("prompt_rejected");
                body["accepted"] = json!(false);
            }
            let body = serde_json::to_vec(&body).map_err(|e2| ApiError::internal(e2.to_string()))?;
            Ok(http::Response::builder()
                .status(500)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("static builder"))
        }
        Err(_) => reject_session_response(&ty),
    }
}

/// 会话拒绝响应:404 JSON {error, code?, accepted?}(对齐上游 route.ts:28-33
/// 的 prompt_rejected 信封;非 prompt 命令仅 {error})。
fn reject_session_response(rpc_type: &str) -> Result<http::Response<Vec<u8>>, ApiError> {
    let mut body = json!({ "error": "Session not found" });
    if rpc_type == "prompt" {
        body["code"] = json!("prompt_rejected");
        body["accepted"] = json!(false);
    }
    let body = serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(http::Response::builder()
        .status(404)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("static builder"))
}

/// GET /api/agent/:id/bash-output?path= —— bash 全量输出读取
/// (lib fs::bash_output:O_NOFOLLOW + tempRoot 下 pi-bash-*.log 校验 +
/// 限字节)。本 runtime 的 run_bash 内联返回输出(fullOutputPath:null,
/// moho 同款)不写全量文件 —— 此端点为截断场景的防御性完备:
/// 前端对截断输出请求此端点,无文件 → 404 优雅降级。
async fn agent_bash_output(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let path = dispatch
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if path.is_empty() {
        return Err(ApiError::new(400, "path is required"));
    }
    // temp root 校验(防目录穿越;lib resolve_bash_output_path)
    let temp_root = std::env::temp_dir().to_string_lossy().into_owned();
    let Some(resolved) = crate::fs::bash_output::resolve_bash_output_path(&path, &temp_root) else {
        return Err(ApiError::new(400, "invalid bash output path"));
    };
    match crate::fs::bash_output::read_utf8_file_within_limit(&resolved, None).await {
        Ok(crate::fs::bash_output::ReadResult::Content { content, .. }) => {
            json_response(json!({ "output": content }))
        }
        Ok(crate::fs::bash_output::ReadResult::TooLarge { size }) => Err(ApiError::new(
            413,
            format!("bash output too large ({size} bytes); use download"),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ApiError::not_found("bash output file not found"))
        }
        Err(e) => Err(ApiError::internal(format!("read bash output: {e}"))),
    }
}

// ── project_trust + models catalog(P4 遗留面补全) ──────────────────────

/// GET /api/project-trust?cwd= —— 恒信任 stub(lib security,引擎无扩展系统)。
async fn project_trust_get(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch
        .args
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    let st = crate::security::project_trust::get_project_trust_status(&cwd, "");
    json_response(json!({ "requiresTrust": st.requires_trust, "trusted": st.trusted }))
}

/// POST /api/project-trust?cwd= —— trust(stub 同上,无副作用)。
async fn project_trust_set(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch
        .args
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd is required"));
    }
    let st = crate::security::project_trust::trust_project(&cwd, "");
    json_response(json!({ "requiresTrust": st.requires_trust, "trusted": st.trusted }))
}

/// catalog 缓存(1h TTL,文件级:temp/moho-mate-models-dev-catalog.json ——
/// 与旧 moho handler 同路径同 TTL,两实现共享缓存)。
const CATALOG_URL: &str = "https://models.dev/api.json";
const CATALOG_TTL_SECS: u64 = 3600;

fn catalog_cache_path() -> std::path::PathBuf {
    std::env::temp_dir().join("moho-mate-models-dev-catalog.json")
}

/// 磁盘缓存命中(TTL 内)返回 payload;未命中返回 None。纯本地 IO,
/// 可安全跑在共享 blocking 池线程上。
fn catalog_from_cache() -> Option<Value> {
    let cache_path = catalog_cache_path();
    let meta = std::fs::metadata(&cache_path).ok()?;
    let modified = meta.modified().ok()?;
    if modified
        .elapsed()
        .unwrap_or(std::time::Duration::from_secs(CATALOG_TTL_SECS))
        >= std::time::Duration::from_secs(CATALOG_TTL_SECS)
    {
        return None;
    }
    let content = std::fs::read_to_string(&cache_path).ok()?;
    serde_json::from_str::<Value>(&content).ok()
}

/// catalog 获取:缓存快路径 + 未命中时**专用线程**网络抓取。
/// 网络经 HostHooks::fetch(策略在此:15s;机制在宿主)—— 不得占用共享
/// blocking 池:池线程被数秒级网络等待独占时,sessions_list 等本地 IO
/// 命令会在池上排队饿死(宿主过渡代理 2s 收包超时 → 侧栏 HTTP 500,
/// 网络不可达机型上 100% 复现)。等待在 async 上下文(oneshot),池零占用。
async fn fetch_catalog(hooks: Arc<dyn HostHooks>) -> Result<Value, String> {
    if let Some(v) = catalog_from_cache() {
        return Ok(v);
    }
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let fetched = (|| {
            let resp = hooks.fetch(&super::FetchSpec::get_json(
                CATALOG_URL,
                std::time::Duration::from_secs(15), // 对齐旧 handler 的 catalog 超时
            ))?;
            if !(200..300).contains(&resp.status) {
                return Err(format!("fetch {CATALOG_URL}: HTTP {}", resp.status));
            }
            let text = resp.text();
            let v: Value =
                serde_json::from_str(&text).map_err(|e| format!("catalog parse: {e}"))?;
            let _ = std::fs::write(catalog_cache_path(), &text);
            Ok(v)
        })();
        let _ = tx.send(fetched);
    });
    rx.await.map_err(|_| "catalog fetch thread crashed".to_string())?
}

/// ModelCatalogEntry → camelCase Value(lib 无 Serialize derive,手工转;
/// 字段对齐旧 moho handler 的展平形状)。
fn catalog_entry_value(e: &crate::models::catalog::ModelCatalogEntry) -> Value {
    json!({
        "key": e.key,
        "providerId": e.provider_id,
        "providerName": e.provider_name,
        "providerBaseUrl": e.provider_base_url,
        "id": e.id,
        "name": e.name,
        "reasoning": e.reasoning,
        "input": e.input,
        "contextWindow": e.context_window,
        "maxTokens": e.max_tokens,
        "cost": serde_json::to_value(&e.cost).unwrap_or(Value::Null),
    })
}

/// GET /api/models-config/catalog?q=&provider=&limit=&baseUrl=
/// (网络经 HostHooks::fetch_text;展平/搜索/推荐用 lib 纯函数)。
async fn models_config_catalog(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let q: String = dispatch
        .args
        .get("q")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    let provider: String = dispatch
        .args
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    let base_url: String = dispatch
        .args
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(500)
        .collect();
    let limit = dispatch
        .args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .clamp(1, 100) as usize;

    let hooks = ctx.hooks.clone();
    let payload = fetch_catalog(hooks).await.map_err(|e| ApiError::new(502, e))?;
    let entries = crate::models::catalog::flatten_models_dev_catalog(&payload);
    if entries.is_empty() {
        return Err(ApiError::new(502, "models.dev returned an empty catalog"));
    }
    let found = crate::models::catalog::search_model_catalog(&entries, &q, &provider, limit);
    let recommendation =
        crate::models::catalog::recommend_model_catalog_preset(&entries, &q, &provider, &base_url);
    json_response(json!({
        "models": found.iter().map(catalog_entry_value).collect::<Vec<_>>(),
        "recommendation": serde_json::to_value(&recommendation).unwrap_or(Value::Null),
        "source": CATALOG_URL,
    }))
}
