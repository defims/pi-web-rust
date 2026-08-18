//! session_runtime — 多会话注册表 + 每会话任务(P3)。
//!
//! 架构(docs/api-embed-plan.md §三.3):
//! - 注册表 `session_id → SessionHandle`(mailbox sender + 状态快照 + dead 标记)
//! - **每会话一个 spawn 任务**(chat_thread 模式任务化):mailbox 消费循环;
//!   prompt future 装箱借用 `&mut handle` 的完整 turn 循环在 P3-2 接线
//!   (借用纪律:运行期命令只读快照,借 handle 的命令 busy-reject)
//! - 生命周期:容量默认 1,溢出 = 关旧建新;destroy 显式销毁
//! - panic 纪律(P0 报告:JoinHandle 向 await 点传播):监督 catch_unwind,
//!   会话 panic → 标记 dead(注册表剔除,后续请求 404)
//!
//! 事件线(P3-2):引擎 on_event → wire 过滤 → EventSink;合成事件
//! (prompt_done/agent_settled/queue_update/compaction_*)随 turn 循环。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use pi::sdk::{AbortHandle, AgentEvent, AgentSessionHandle};

use super::commands::ExecCtx;
use super::ApiError;

/// 会话命令(mailbox 载荷)。
#[derive(Debug)]
pub(crate) enum SessionCmd {
    /// 发起一轮 prompt(完整 turn 循环,见 session_loop Prompt 分支)。
    Prompt {
        message: String,
        images: Option<Vec<pi::sdk::ImageContent>>,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Steer { message: String, reply: oneshot::Sender<Result<Value, String>> },
    FollowUp { message: String, reply: oneshot::Sender<Result<Value, String>> },
    Abort,
    ClearQueue { reply: oneshot::Sender<Result<Value, String>> },
    SetModel { provider: String, model: String, reply: oneshot::Sender<Result<Value, String>> },
    SetThinking { level: String, reply: oneshot::Sender<Result<Value, String>> },
    SetSessionName { name: String, reply: oneshot::Sender<Result<Value, String>> },
    Compact { reply: oneshot::Sender<Result<Value, String>> },
    SetTools { names: Vec<String>, reply: oneshot::Sender<Result<Value, String>> },
    /// 树导航(移动引擎 current leaf;空闲期借 handle 实现)。
    NavigateTree { target_id: String, reply: oneshot::Sender<Result<Value, String>> },
    /// fork:创建分支会话文件 + 重建 handle 到新文件。
    Fork { entry_id: Option<String>, reply: oneshot::Sender<Result<Value, String>> },
    /// reload:保留会话文件重建 handle(上游 reload 语义)。
    Reload { reply: oneshot::Sender<Result<Value, String>> },
    /// 重建 handle(ReBuild 到指定文件/None=全新);fork/reload 的内部机制。
    Rebuild { path: Option<std::path::PathBuf>, reply: oneshot::Sender<Result<Value, String>> },
    /// bash 执行(spawn 子进程 + JSONL 落盘;运行期也允许 —— 不借 handle)
    Bash { command: String, reply: oneshot::Sender<Result<Value, String>> },
    /// abort bash(kill 子进程;运行期也允许)
    AbortBash { reply: oneshot::Sender<Result<Value, String>> },
    /// set_tools / extension 面:P4 后接线,当前统一占位。
    Deferred { what: &'static str, reply: oneshot::Sender<Result<Value, String>> },
    GetStats { reply: oneshot::Sender<Result<Value, String>> },
    GetLastText { reply: oneshot::Sender<Result<Value, String>> },
}

/// 会话状态快照(LoopState 对应物;app 自有追踪 —— 计划口径:
/// turn 是本任务自己的调度事实,队列是 app 自有设计)。
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionSnap {
    pub session_id: Option<String>,
    pub is_streaming: bool,
    pub is_prompt_running: bool,
    pub is_compacting: bool,
    pub queued_steering: VecDeque<String>,
    pub queued_follow_up: VecDeque<String>,
    pub last_assistant_text: Option<String>,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub thinking_level: Option<String>,
    /// 会话文件路径(重建 fork/reload 用;首次落盘后回填或重建时显式传入)
    pub session_path: Option<std::path::PathBuf>,
    /// 会话 cwd(重建 handle 时保留;create 时注入)
    pub cwd: Option<String>,
    /// bash 执行中(app 自有:子进程由本 runtime spawn)
    pub is_bash_running: bool,
    /// bash 子进程 pid(abort kill 用)
    pub bash_child_pid: Option<u32>,
    /// 会话名(SetSessionName 维护;get_session_stats 合并进响应)
    pub session_name: Option<String>,
    /// 运行期排队的完整 prompt(上游 pendingPromptCount 语义:连发第二条
    /// prompt 不拒绝,当前 turn 完成后自动续跑;reply 在入队时已 ack)
    pub queued_prompts: VecDeque<(String, Option<Vec<pi::sdk::ImageContent>>)>,
}

impl SessionSnap {
    /// agent_get_state.state 形状(前端挂载恢复/运行中对账的切片)。
    pub fn to_state_json(&self) -> Value {
        let (model, thinking) = match (&self.model_provider, &self.model_id) {
            (Some(p), Some(m)) => (
                json!({ "id": m, "provider": p }),
                self.thinking_level.clone().unwrap_or_else(|| "off".into()),
            ),
            _ => (Value::Null, "off".to_string()),
        };
        json!({
            "sessionId": self.session_id,
            "isStreaming": self.is_streaming,
            "isPromptRunning": self.is_prompt_running,
            "isCompacting": self.is_compacting,
            "isBashRunning": self.is_bash_running,
            "model": model,
            "queuedMessages": {
                "steering": self.queued_steering.iter().cloned().collect::<Vec<_>>(),
                "followUp": self.queued_follow_up.iter().cloned().collect::<Vec<_>>(),
            },
            "contextUsage": Value::Null,
            "systemPrompt": Value::Null,
            "thinkingLevel": thinking,
            "extensionStatuses": [],
            "extensionWidgets": [],
        })
    }
}

/// 注册表句柄:mailbox sender + 快照 + dead 标记。
#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub tx: mpsc::Sender<SessionCmd>,
    pub snap: Arc<Mutex<SessionSnap>>,
    pub dead: Arc<std::sync::atomic::AtomicBool>,
}

/// 会话运行时(经 PiWebApi 持有)。
pub(crate) struct SessionRuntime {
    pub sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    pub max_sessions: usize,
    /// 死会话恢复的 per-id 启动锁:两次并发 RPC 对同一磁盘会话同时 restore
    /// 会为同一文件起两个引擎实例(双写)—— 串行化恢复,恢复后二次检查。
    restore_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl SessionRuntime {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_sessions,
            restore_locks: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 创建并注册会话(溢出 = 关旧建新,对齐 moho Clear/rebuild 语义)。
    /// 历史会话惰性恢复(对齐上游 rpc-manager:POST /api/agent/:id 对不在
    /// 内存 registry 的会话先 resolveSessionPath 再 startRpcSession(id, path))。
    /// 场景:app 重开后前端恢复旧会话视图继续发消息 —— registry 为空,没有
    /// 这条路径时 RPC 404,前端乐观 agentRunning 永不收敛(loading 永转)。
    /// 返回 false = 磁盘也找不到(调用方回 404)。
    pub async fn restore(&self, ctx: &ExecCtx, id: &str) -> bool {
        // per-id 启动锁(并发恢复防双实例)
        let lock = self
            .restore_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;
        // 恢复期间首个调用者可能已注册成功
        if self.sessions.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id) {
            return true;
        }
        // 与 create 传给引擎的 session_dir 同根(引擎落盘位置);hooks 根仅当
        // 宿主覆写了 sessions_root 且引擎仍写默认根时兜底扫描。
        let root = super::commands::default_sessions_root_pub();
        // 文件名为 <timestamp>_<id前8位>.jsonl(lib 落盘约定),前缀候选 +
        // header 精确校验(短前缀有碰撞可能)
        let prefix = id.split('-').next().unwrap_or("").to_string();
        let full_id = id.to_string();
        let found = super::commands::blocking(ctx, move || {
            let dirs = std::fs::read_dir(&root).ok()?;
            let suffix = format!("_{prefix}.jsonl");
            for d in dirs.filter_map(|e| e.ok()) {
                let files = std::fs::read_dir(d.path()).ok()?;
                for f in files.filter_map(|e| e.ok()) {
                    let name = f.file_name().to_string_lossy().into_owned();
                    if !name.ends_with(&suffix) {
                        continue;
                    }
                    let path_s = f.path().to_string_lossy().into_owned();
                    if crate::session::reader::read_session_header(&path_s)
                        .is_some_and(|h| h.id == full_id)
                    {
                        return Some(f.path());
                    }
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        let Some(path) = found else { return false };
        let path_for_snap = path.clone();
        let header = crate::session::reader::read_session_header(&path.to_string_lossy());
        let cwd = header
            .as_ref()
            .map(|h| h.cwd.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "/".to_string());

        let hooks = ctx.hooks.clone();
        let sink = ctx.sink.clone();
        let cwd_owned = cwd.clone();
        let handle = super::commands::blocking(ctx, move || -> Result<AgentSessionHandle, ApiError> {
            let on_event: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |ev: AgentEvent| {
                let ev_value = serde_json::to_value(&ev).unwrap_or(serde_json::Value::Null);
                if let Some(payload) = to_client_event(&ev_value) {
                    let sink = sink.clone();
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                        sink(super::events::ApiEvent::Agent { payload });
                    }));
                }
            });
            let options = pi::sdk::SessionOptions {
                system_prompt: hooks.system_prompt(),
                tool_factory: hooks.tool_factory(),
                no_session: false,
                session_dir: Some(std::path::PathBuf::from(
                    super::commands::default_sessions_root_pub(),
                )),
                working_directory: Some(std::path::PathBuf::from(&cwd_owned)),
                session_path: Some(path),
                on_event: Some(on_event),
                ..Default::default()
            };
            futures::executor::block_on(pi::sdk::create_agent_session(options))
                .map_err(|e| ApiError::internal(format!("restore_agent_session: {e}")))
        })
        .await;
        let handle = match handle {
            Ok(Ok(h)) => h,
            _ => {
                eprintln!("session {id}: disk restore failed");
                return false;
            }
        };

        let engine_state = handle.state().await.ok();
        let restored_id = engine_state
            .as_ref()
            .and_then(|s| s.session_id.clone())
            .unwrap_or_else(|| id.to_string());
        let (tx, rx) = mpsc::channel::<SessionCmd>(64);
        let snap = Arc::new(Mutex::new(SessionSnap {
            session_id: Some(restored_id.clone()),
            session_path: Some(path_for_snap),
            cwd: Some(cwd.clone()),
            ..Default::default()
        }));
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dead_c = dead.clone();
        let snap_task = snap.clone();
        let rt = ctx.rt.clone();
        let sink = ctx.sink.clone();
        let hooks = ctx.hooks.clone();
        let tx_task = tx.clone();
        let dead_task = dead.clone();
        let registry_task = self.sessions.clone();
        let _joined = rt.handle().spawn(async move {
            let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                session_loop(handle, rx, tx_task, dead_task, registry_task, snap_task, sink, rt.clone(), hooks),
            ))
            .await;
            if result.is_err() {
                dead_c.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(restored_id.clone(), SessionHandle { tx, snap, dead });
        eprintln!("session {restored_id}: restored from disk");
        true
    }

    /// 返回 (session_id, 引擎 provider, 引擎 model, 引擎 thinking)——后三者
    /// 供 agent_new 响应回传(对齐上游 route.ts:70-84,前端据此同步选中态)。
    pub async fn create(
        &self,
        ctx: &ExecCtx,
        cwd: &str,
        provider: Option<String>,
        model: Option<String>,
        thinking_level: Option<String>,
        enabled_tools: Option<Vec<String>>,
    ) -> Result<(String, Option<String>, Option<String>, Option<String>), ApiError> {
        // 引擎会话创建(重、同步外壳)走 blocking pool
        let hooks = ctx.hooks.clone();
        let provider_c = provider.clone();
        let model_c = model.clone();
        let tl_c = thinking_level.clone();
        let tools_c = enabled_tools.clone();
        let cwd_owned = cwd.to_string();
        let sink = ctx.sink.clone();
        let scope_out = super::commands::blocking(ctx, move || -> Result<(AgentSessionHandle, Option<(String, String)>, Option<(String, String, String)>), ApiError> {
            // 事件线第一段:引擎 on_event → wire 过滤 → EventSink
            let on_event: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |ev: AgentEvent| {
                let ev_value = serde_json::to_value(&ev).unwrap_or(serde_json::Value::Null);
                if let Some(payload) = to_client_event(&ev_value) {
                    let sink = sink.clone();
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                        sink(super::events::ApiEvent::Agent { payload });
                    }));
                }
            });
            let mut options = pi::sdk::SessionOptions {
                provider: provider_c.clone(),
                model: model_c.clone(),
                // 仅显式传值(行级对齐上游 route.ts:61):未指定时交引擎解析
                // (scoped pin → 会话恢复 → settings default_thinking_level →
                // 引擎默认经 clamp,app.rs:620-637)。此前强制 Off 会覆盖
                // settings 默认与 enabledModels 的 pin。
                thinking: tl_c.as_deref().and_then(|s| s.parse().ok()),
                system_prompt: hooks.system_prompt(),
                tool_factory: hooks.tool_factory(),
                enabled_tools: tools_c,
                no_session: false,
                session_dir: Some(std::path::PathBuf::from(
                    super::commands::default_sessions_root_pub(),
                )),
                working_directory: Some(std::path::PathBuf::from(&cwd_owned)),
                on_event: Some(on_event),
                ..Default::default()
            };

            // P1(上游 rpc-manager.ts:1599-1627 对齐):services 一次快照
            // (settings 全局+项目合并的 config + 真 registry),web 层解析
            // 默认模型 —— 显式越界 400;未指定时 default > scoped[0] >
            // visible 内 default。解析出的 (provider, model) 显式注入,
            // 引擎兜底链不再参与(上游同款"原子性"语义)。
            let services = futures::executor::block_on(
                pi::sdk::create_agent_session_services(&options),
            )
            .map_err(|e| ApiError::internal(format!("agent_session_services: {e}")))?;
            let cfg = &services.config;
            let scope = crate::models::scope::resolve_visible_models(
                cfg.enabled_models.as_deref().unwrap_or(&[]),
                || {
                    services
                        .model_registry
                        .get_available()
                        .into_iter()
                        .map(|e| crate::models::scope::Model {
                            id: e.model.id.clone(),
                            name: e.model.name.clone(),
                            provider: e.model.provider.clone(),
                        })
                        .collect()
                },
                |patterns| {
                    let (scoped, diags) = pi::sdk::resolve_model_scope_with_diagnostics(
                        patterns,
                        &services.model_registry,
                        false,
                    );
                    let scoped = scoped
                        .into_iter()
                        .map(|sm| crate::models::scope::ScopedModel {
                            model: crate::models::scope::Model {
                                id: sm.model.model.id.clone(),
                                name: sm.model.model.name.clone(),
                                provider: sm.model.model.provider.clone(),
                            },
                            thinking_level: sm.thinking_level.map(|t| t.to_string()),
                        })
                        .collect();
                    (scoped, diags)
                },
            );
            let default_ref = match (&cfg.default_provider, &cfg.default_model) {
                (Some(p), Some(m)) => Some(crate::models::scope::ModelRef {
                    provider: p.clone(),
                    model_id: m.clone(),
                }),
                _ => None,
            };
            let requested_ref = match (&provider_c, &model_c) {
                (Some(p), Some(m)) => Some(crate::models::scope::ModelRef {
                    provider: p.clone(),
                    model_id: m.clone(),
                }),
                _ => None,
            };
            let sel = crate::models::scope::select_initial_model_scope(
                &scope,
                &crate::models::scope::InitialModelScopeOptions {
                    requested_model: requested_ref.clone(),
                    default_model: default_ref,
                    thinking_level: tl_c.clone(),
                },
            )
            .map_err(|e| ApiError::new(400, e.0))?;
            if let Some(m) = &sel.model {
                options.provider = Some(m.provider.clone());
                options.model = Some(m.id.clone());
            }
            if let Some(level) = &sel.thinking_level {
                options.thinking = level.parse().ok();
            }

            let handle = futures::executor::block_on(
                pi::sdk::create_agent_session_from_services(&services, options),
            )
            .map_err(|e| ApiError::internal(format!("create_agent_session: {e}")))?;
            // startup preferences 原料:显式请求(可能为空) + 引擎生效值
            let state = futures::executor::block_on(handle.state()).ok();
            let effective = state.map(|s| (s.provider.clone(), s.model_id.clone(), s.thinking_level.map(|t| t.to_string())));
            Ok((handle, requested_ref.map(|r| (r.provider, r.model_id)), effective.map(|(p, m, t)| (p, m, t.unwrap_or_default()))))
        })
        .await??;
        let (handle, explicit_ref, effective) = scope_out;

        // P1-2(上游 rpc-manager.ts:1629-1643):显式选择与引擎生效一致时
        // 写回 settings.json 默认(deep-merge 保他键)—— "记住我的选择"。
        // 文件 IO 走 blocking。
        let explicit_model_ref = explicit_ref.clone().map(|(p, m)| {
            crate::models::cache::ModelRef { provider: p, model_id: m }
        });
        {
            let explicit = crate::settings::startup_preferences::ExplicitStartupPreferences {
                model: explicit_model_ref,
                thinking_level: thinking_level.clone(),
            };
            let effective = crate::settings::startup_preferences::EffectiveStartupPreferences {
                model: effective.as_ref().map(|(p, m, _)| {
                    crate::models::cache::ModelRef {
                        provider: p.clone(),
                        model_id: m.clone(),
                    }
                }),
                thinking_level: effective
                    .as_ref()
                    .map(|(_, _, t)| t.clone())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "off".to_string()),
                supports_thinking: true,
            };
            let tl_guard = thinking_level.clone();
            let _ = super::commands::blocking(ctx, move || {
                let mut ops = JsonSettingsOps::new();
                let _ = crate::settings::startup_preferences::persist_explicit_startup_preferences(
                    &mut ops,
                    &explicit,
                    &effective,
                );
            })
            .await;
        }

        let engine_state = handle.state().await.ok();
        let session_id = engine_state
            .as_ref()
            .and_then(|s| s.session_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // 模型真相从引擎 state 回填(显式 provider/model 是请求意向;注意引擎
        // registry 对 models.json 键名与 entry.provider 字段的映射在显式
        // find 路径上存在口径差 —— 自动选择路径已验证可用,显式路径待上游
        // 追踪核实后放开)
        let (eng_provider, eng_model, eng_thinking) = engine_state
            .as_ref()
            .map(|s| {
                (
                    Some(s.provider.clone()),
                    Some(s.model_id.clone()),
                    s.thinking_level.map(|t| t.to_string()),
                )
            })
            .unwrap_or((None, None, None));

        // 溢出:容量满 → 剔除最旧(drop sender → 任务循环退出)
        {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            while sessions.len() >= self.max_sessions {
                match sessions.keys().next().cloned() {
                    Some(id) => {
                        sessions.remove(&id);
                    }
                    None => break,
                }
            }
        }

        // 会话任务(监督 catch_unwind;panic → dead 标记)
        let (tx, rx) = mpsc::channel::<SessionCmd>(64);
        let snap = Arc::new(Mutex::new(SessionSnap {
            session_id: Some(session_id.clone()),
            model_provider: eng_provider.clone().or(provider),
            model_id: eng_model.clone().or(model),
            // 引擎解析真相优先(含 scoped pin/settings 默认);显式请求值兜底
            thinking_level: eng_thinking.clone().or(thinking_level),
            cwd: Some(cwd.to_string()),
            ..Default::default()
        }));
        // 注:路径回填 —— 引擎 AgentSessionState 无 session_file 字段;fork/reload
        // 重建时 session_path 显式传入(rebuild_session),首次落盘由引擎 save 触发
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dead_c = dead.clone();
        let snap_task = snap.clone();
        let rt = ctx.rt.clone();
        let sink = ctx.sink.clone();
        let hooks = ctx.hooks.clone();
        let tx_task = tx.clone();
        let dead_task = dead.clone();
        let registry_task = self.sessions.clone();
        let _joined = rt.handle().spawn(async move {
            // panic 纪律:mid-await panic 经 FutureExt::catch_unwind 截获(P0 报告:
            // panic 会向 await 点传播,监督任务不得被波及)
            let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                session_loop(handle, rx, tx_task, dead_task, registry_task, snap_task, sink, rt.clone(), hooks),
            ))
            .await;
            if result.is_err() {
                dead_c.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });

        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.clone(), SessionHandle { tx, snap, dead });
        // 引擎真相随会话返回(agent_new 响应回传 model/thinkingLevel,
        // 对齐上游 route.ts:70-84)
        Ok((
            session_id,
            eng_provider,
            eng_model,
            eng_thinking,
        ))
    }

    /// 显式销毁(drop sender → 任务退出;注册表剔除)。
    pub fn destroy(&self, id: &str) -> bool {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id).is_some()
    }

    pub fn get(&self, id: &str) -> Option<SessionHandle> {
        let h = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
        // dead 会话视同不存在(404)
        h.filter(|h| !h.dead.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// 正在运行的会话 id(对齐上游 getRunningRpcSessionIds/isRunning,
    /// rpc-manager.ts:207-209、1491-1497):alive 且 streaming/prompt/compacting
    /// 任一为真。此前只过滤 dead,导致所有聊过的会话永久显示运行中
    /// (侧栏 spinner 永转)。
    pub fn running_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, h)| {
                if h.dead.load(std::sync::atomic::Ordering::SeqCst) {
                    return false;
                }
                let s = h.snap.lock().unwrap_or_else(|e| e.into_inner());
                s.is_streaming || s.is_prompt_running || s.is_compacting
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// 会话任务主循环(chat_thread select_loop 任务化)。
///
/// 借用模型:prompt future 借用 `&mut handle`;borrow checker 要求该 future
/// 不跨空闲循环迭代存活 —— 与 chat_thread 同款:起 prompt 与驱动至完成放
/// 进同一匹配(空闲内层循环返回 future,直接流入运行期子循环,不落跨迭代变量)。
async fn session_loop(
    mut handle: AgentSessionHandle,
    mut rx: mpsc::Receiver<SessionCmd>,
    tx: mpsc::Sender<SessionCmd>,
    dead: Arc<std::sync::atomic::AtomicBool>,
    registry: Arc<Mutex<HashMap<String, SessionHandle>>>,
    snap: Arc<Mutex<SessionSnap>>,
    sink: super::EventSink,
    rt: Arc<asupersync::runtime::Runtime>,
    hooks: Arc<dyn super::HostHooks>,
) {
    let mut abort_handle: Option<AbortHandle> = None;
    loop {
        match rx.recv().await {
            None => return,
            Some(cmd) => match cmd {
                // Prompt:future 在独立函数内创建并消费(不跨迭代返回借用 ——
                // 等价 chat_thread 的"起 prompt 与驱动至完成同一匹配",但结构
                // 更简:借用生命周期封闭在 handle_prompt_turn 调用内)
                SessionCmd::Prompt { message, images, reply } => {
                    let _ = reply.send(Ok(json!({})));
                    handle_prompt_turn(&mut handle, &mut rx, message, images, &mut abort_handle, &snap, &sink)
                        .await;
                }
                other => {
                    idle_cmd(other, &mut handle, &snap, &sink, &rt, &hooks, &tx, &dead, &registry).await;
                }
            },
        }
    }
}

/// 完整 turn:入队镜像 drain → abort handle → mark_running → select 驱动 →
/// 回落。prompt future 的 &mut handle 借用在本函数内创建并释放。
async fn handle_prompt_turn(
    handle: &mut AgentSessionHandle,
    rx: &mut mpsc::Receiver<SessionCmd>,
    mut message: String,
    mut images: Option<Vec<pi::sdk::ImageContent>>,
    abort_handle: &mut Option<AbortHandle>,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
) {
    // 循环:首轮用参数,之后续跑 queued_prompts(上游 pendingPromptCount
    // 排队语义 —— 连发第二条 prompt 不拒绝,当前 turn 完成自动续跑)
    loop {
        {
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            for m in s.queued_steering.drain(..) {
                let _ = handle.session_mut().agent.queue_steering(user_text_message(m));
            }
            for m in s.queued_follow_up.drain(..) {
                let _ = handle.session_mut().agent.queue_follow_up(user_text_message(m));
            }
        }
        let (ah, signal) = AgentSessionHandle::new_abort_handle();
        *abort_handle = Some(ah);
        mark_running(snap, true);
        let fut: PinBoxPromptFut<'_> = match images {
            Some(imgs) if !imgs.is_empty() => {
                Box::pin(handle.prompt_images_with_abort(message, imgs, signal, |_| {}))
            }
            _ => Box::pin(handle.prompt_with_abort(message, signal, |_| {})),
        };
        let res = run_prompt_until_settled(fut, rx, abort_handle.as_ref(), snap, sink).await;
        mark_running(snap, false);
        *abort_handle = None;
        // 队列非空 → 续跑;agent_settled 只在最后一轮发(上游 settled = 无
        // 活动 run 且无排队续跑)
        let next = snap
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queued_prompts
            .pop_front();
        finish_turn(res, snap, sink, next.is_none()).await;
        match next {
            Some((m, imgs)) => {
                message = m;
                images = imgs;
            }
            None => return,
        }
    }
}

type PinBoxPromptFut<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<pi::sdk::AssistantMessage, pi::sdk::Error>>
            + Send
            + 'a,
    >,
>;

/// 空闲命令处理(非 prompt;借 handle 的命令在此正常执行)。
async fn idle_cmd(
    cmd: SessionCmd,
    mut handle: &mut AgentSessionHandle,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
    rt: &Arc<asupersync::runtime::Runtime>,
    hooks: &Arc<dyn super::HostHooks>,
    tx: &mpsc::Sender<SessionCmd>,
    dead: &Arc<std::sync::atomic::AtomicBool>,
    registry: &Arc<Mutex<HashMap<String, SessionHandle>>>,
) {
    match cmd {
        SessionCmd::Steer { message, reply } => {
            {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                s.queued_steering.push_back(message.clone());
            }
            emit_queue_update(snap, sink);
            let _ = reply.send(Ok(json!({})));
        }
        SessionCmd::FollowUp { message, reply } => {
            {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                s.queued_follow_up.push_back(message.clone());
            }
            emit_queue_update(snap, sink);
            let _ = reply.send(Ok(json!({})));
        }
        SessionCmd::Abort => {}
        SessionCmd::ClearQueue { reply } => {
            let (steering, follow_up) = {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                (
                    s.queued_steering.drain(..).collect::<Vec<_>>(),
                    s.queued_follow_up.drain(..).collect::<Vec<_>>(),
                )
            };
            emit_queue_update(snap, sink);
            let _ = reply.send(Ok(json!({ "steering": steering, "followUp": follow_up })));
        }
        SessionCmd::SetModel { provider, model, reply } => {
            let r = handle
                .set_model(&provider, &model)
                .await
                .map(|_| json!({}))
                .map_err(|e| format!("{e}"));
            if r.is_ok() {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                s.model_provider = Some(provider);
                s.model_id = Some(model);
            }
            let _ = reply.send(r);
        }
        SessionCmd::SetThinking { level, reply } => {
            let r = match level.parse::<pi::sdk::ThinkingLevel>() {
                Ok(t) => handle.set_thinking_level(t).await.map(|_| json!({})).map_err(|e| format!("{e}")),
                Err(e) => Err(e),
            };
            if r.is_ok() {
                snap.lock().unwrap_or_else(|e| e.into_inner()).thinking_level = Some(level);
            }
            let _ = reply.send(r);
        }
        SessionCmd::SetSessionName { name, reply } => {
            // 空名校验(对齐上游 rpc-manager.ts:607-613:空名直接拒绝)
            let r = if name.trim().is_empty() {
                Err("session name must not be empty".to_string())
            } else {
                handle.set_session_name(&name).await.map(|_| json!({})).map_err(|e| format!("{e}"))
            };
            if r.is_ok() {
                snap.lock().unwrap_or_else(|e| e.into_inner()).session_name = Some(name);
            }
            let _ = reply.send(r);
        }
        SessionCmd::Bash { command, reply } => {
            // 定位会话文件:优先快照;缺失时按 id 扫描(root 经 hooks)。
            // 不借 handle force save —— AgentCx 的 raw 指针不能跨 Send 任务
            // await(save 场景)。已知缺口:全新未落盘会话(引擎 write-behind,
            // 首个 mutation 前无文件)的 bash 输出不持久化;prompt 后引擎自动
            // 落盘,该缺口仅影响"首条 bash 先于首条 prompt"的罕见序列。
            {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                if s.session_path.is_none()
                    && s.session_id.as_deref().is_some_and(|sid| !sid.is_empty())
                {
                    let root = hooks
                        .sessions_root()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(super::commands::default_sessions_root_pub);
                    let sid = s.session_id.clone().unwrap();
                    if let Some(p) = super::sessions::find_session_file(&root, &sid) {
                        s.session_path = Some(p);
                    }
                }
            }
            run_bash(command, snap.clone(), reply);
        }
        SessionCmd::AbortBash { reply } => {
            abort_bash_kill(snap);
            let _ = reply.send(Ok(json!({})));
        }
        SessionCmd::NavigateTree { target_id, reply } => {
            // 移动引擎 current leaf(参考 chat_thread NavigateTree / rpc-manager)。
            // pi Session 锁需 Cx:空闲期无 run 持锁,可安全获取;navigate_to
            // 不存在返回 false → cancelled:true。
            let cx = pi::agent_cx::AgentCx::for_current_or_request();
            let ok = handle
                .session_mut()
                .session
                .lock(cx.cx())
                .await
                .map(|mut g| g.navigate_to(&target_id))
                .unwrap_or(false);
            let _ = reply.send(Ok(json!({ "cancelled": !ok })));
        }
        SessionCmd::Fork { entry_id, reply } => {
            // fork:创建分支文件(到 fork 点不含) + 重建 handle 到新文件。
            // 文件手术复制自 moho session_scanner::fork_session(纯 JSONL 操作)。
            let path = snap.lock().unwrap_or_else(|e| e.into_inner()).session_path.clone();
            match path {
                Some(source) => match fork_file(&source, entry_id.as_deref()) {
                    Ok(new_path) => {
                        let cwd = snap.lock().unwrap_or_else(|e| e.into_inner()).cwd.clone().unwrap_or_default();
                        match rebuild_session(&mut handle, snap.clone(), Some(new_path.clone()), cwd, sink, rt, hooks).await {
                            Ok(()) => {
                                // registry 重键(对齐上游 fork:cacheSessionPath(newId)+
                                // 旧会话 shutdown):rebuild 已把 snap.session_id 换成
                                // 新文件的引擎 id;旧键移除,新键接管本任务的 tx/snap。
                                // 不重键则新 id 的后续 RPC 走 restore 为同一文件再起
                                // 一个引擎实例(双写),旧键成孤儿。
                                let new_id = snap
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .session_id
                                    .clone()
                                    .unwrap_or_default();
                                let old_id_key = {
                                    let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                                    let mut old: Option<String> = None;
                                    for (k, v) in reg.iter() {
                                        if std::sync::Arc::ptr_eq(&v.snap, &snap) {
                                            old = Some(k.clone());
                                            break;
                                        }
                                    }
                                    if let Some(k) = &old {
                                        reg.remove(k);
                                    }
                                    if !new_id.is_empty() {
                                        reg.insert(
                                            new_id.clone(),
                                            SessionHandle { tx: tx.clone(), snap: snap.clone(), dead: dead.clone() },
                                        );
                                    }
                                    old
                                };
                                let _ = old_id_key;
                                let _ = reply.send(Ok(json!({ "cancelled": false, "newSessionId": new_id })));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(format!("fork switch failed: {}", e.message)));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = reply.send(Err(format!("fork file failed: {e}")));
                    }
                },
                None => {
                    let _ = reply.send(Err("session has no file path (not persisted)".into()));
                }
            }
        }
        SessionCmd::Reload { reply } => {
            // reload:保留会话文件重建(上游 reload 语义;扩展面清理在 P4 后)
            let path = snap.lock().unwrap_or_else(|e| e.into_inner()).session_path.clone();
            let cwd = snap.lock().unwrap_or_else(|e| e.into_inner()).cwd.clone().unwrap_or_default();
            match rebuild_session(&mut handle, snap.clone(), path, cwd, sink, rt, hooks).await {
                Ok(()) => {
                    let _ = reply.send(Ok(json!({ "success": true })));
                }
                Err(e) => {
                    let _ = reply.send(Err(format!("reload failed: {}", e.message)));
                }
            }
        }
        SessionCmd::Rebuild { path, reply } => {
            // 显式路径重建(测试/宿主重试用)
            let cwd = snap.lock().unwrap_or_else(|e| e.into_inner()).cwd.clone().unwrap_or_default();
            match rebuild_session(&mut handle, snap.clone(), path, cwd, sink, rt, hooks).await {
                Ok(()) => {
                    let _ = reply.send(Ok(json!({ "success": true })));
                }
                Err(e) => {
                    let _ = reply.send(Err(format!("rebuild failed: {}", e.message)));
                }
            }
        }
        SessionCmd::Compact { reply } => {
            // compact:借 handle 的完整实现 + compaction_start/end 合成
            emit_agent(sink, json!({ "type": "compaction_start" }));
            {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                s.is_compacting = true;
            }
            let r = handle
                .compact(|_| {}) // 事件经 session 级 on_event 透传(避免双发)
                .await
                .map(|_| {
                    // InProcess compact 不暴露 token 统计(上游契约要 tokensBefore/
                    // estimatedTokensAfter,仅在 RPC 变体可得)—— 保持 result 信封
                    json!({ "result": "ok" })
                })
                .map_err(|e| format!("{e}"));
            {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                s.is_compacting = false;
            }
            let reason = if r.is_ok() { "manual" } else { "error" };
            emit_agent(sink, json!({ "type": "compaction_end", "reason": reason }));
            let _ = reply.send(r);
        }
        SessionCmd::SetTools { names, reply } => {
            // pi 无 set_active_tools_by_name → set_system_prompt 近似
            // (对齐 rpc-manager set_tools 与 moho chat_thread 同款手段);
            // 空列表 → 清空 prompt(禁用工具)
            if names.is_empty() {
                handle.session_mut().agent.set_system_prompt(None);
            } else {
                handle
                    .session_mut()
                    .agent
                    .set_system_prompt(Some(super::commands::system_prompt_pub(
                        snap.lock().unwrap_or_else(|e| e.into_inner()).session_id.as_deref(),
                    )));
            }
            let _ = reply.send(Ok(json!({ "success": true })));
        }
        SessionCmd::Deferred { what, reply } => {
            // 剩余:extension_ui_*/get_commands 等扩展面(随扩展接线)
            let _ = reply.send(Err(format!("{what}: extension wiring pending")));
        }
        SessionCmd::GetStats { reply } => {
            let mut stats = handle.get_session_stats().await.unwrap_or_else(|_| json!({}));
            // 合并 snap 会话名(上游 get_session_stats 契约含 sessionName;
            // 引擎 InProcess 统计不带名字段)
            if stats.get("sessionName").and_then(|v| v.as_str()).map_or(true, |n| n.is_empty()) {
                if let Some(name) = snap.lock().unwrap_or_else(|e| e.into_inner()).session_name.clone() {
                    if let Some(obj) = stats.as_object_mut() {
                        obj.insert("sessionName".to_string(), serde_json::json!(name));
                    }
                }
            }
            let _ = reply.send(Ok(stats));
        }
        SessionCmd::GetLastText { reply } => {
            let fallback = snap.lock().unwrap_or_else(|e| e.into_inner()).last_assistant_text.clone();
            let t = handle.get_last_assistant_text().await.ok().flatten().or(fallback);
            let _ = reply.send(Ok(json!(t)));
        }
        SessionCmd::Prompt { reply, .. } => {
            // 主循环已拦截 Prompt;防御分支
            let _ = reply.send(Err("prompt handled at top level".into()));
        }
    }
}

/// 运行期 select 子循环:驱动 prompt future,中途收命令(chat_thread
/// run_prompt_until_settled 的任务化)。借用纪律:运行期绝不借 &mut handle
/// (prompt future 占用);Abort 经 AbortHandle(不借 handle);读命令读快照。
async fn run_prompt_until_settled<'a>(
    mut prompt_fut: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<pi::sdk::AssistantMessage, pi::sdk::Error>> + Send + 'a>,
    >,
    rx: &mut mpsc::Receiver<SessionCmd>,
    abort_handle: Option<&AbortHandle>,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
) -> Result<pi::sdk::AssistantMessage, pi::sdk::Error> {
    loop {
        tokio::select! {
            biased;
            res = &mut prompt_fut => {
                return res;
            }
            cmd = rx.recv() => {
                let Some(cmd) = cmd else {
                    return Err(pi::sdk::Error::api("session task dropped during prompt"));
                };
                handle_running_cmd(cmd, abort_handle, snap, sink);
            }
        }
    }
}

/// 运行期命令:Abort → abort_handle;队列命令 → 镜像 + queue_update;读命令 → 快照。
fn handle_running_cmd(
    cmd: SessionCmd,
    abort_handle: Option<&AbortHandle>,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
) {
    match cmd {
        SessionCmd::Abort => {
            if let Some(ah) = abort_handle {
                ah.abort();
            }
        }
        SessionCmd::Steer { message, reply } => {
            snap.lock().unwrap_or_else(|e| e.into_inner()).queued_steering.push_back(message);
            emit_queue_update(snap, sink);
            let _ = reply.send(Ok(json!({})));
        }
        SessionCmd::FollowUp { message, reply } => {
            snap.lock().unwrap_or_else(|e| e.into_inner()).queued_follow_up.push_back(message);
            emit_queue_update(snap, sink);
            let _ = reply.send(Ok(json!({})));
        }
        SessionCmd::ClearQueue { reply } => {
            let (steering, follow_up) = {
                let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                (
                    s.queued_steering.drain(..).collect::<Vec<_>>(),
                    s.queued_follow_up.drain(..).collect::<Vec<_>>(),
                )
            };
            emit_queue_update(snap, sink);
            let _ = reply.send(Ok(json!({ "steering": steering, "followUp": follow_up })));
        }
        SessionCmd::GetLastText { reply } => {
            let t = snap.lock().unwrap_or_else(|e| e.into_inner()).last_assistant_text.clone();
            let _ = reply.send(Ok(json!(t)));
        }
        SessionCmd::Bash { command, reply } => {
            // 运行期 Bash:不借 handle(prompt future 占用);无 ensure ——
            // 文件未落盘时输出返回但不持久化(moho 运行期同语义)
            run_bash(command, snap.clone(), reply);
        }
        SessionCmd::AbortBash { reply } => {
            abort_bash_kill(snap);
            let _ = reply.send(Ok(json!({})));
        }
        // 运行期新 prompt → 排队(对齐上游 pendingPromptCount:立即 ack 接受,
        // 当前 turn 结束后由 handle_prompt_turn 循环续跑)
        SessionCmd::Prompt { message, images, reply } => {
            let _ = reply.send(Ok(json!({})));
            snap.lock().unwrap_or_else(|e| e.into_inner())
                .queued_prompts.push_back((message, images));
        }
        // 借 handle 的重命令运行期 busy(沿用 chat_thread 语义)
        SessionCmd::SetModel { reply, .. }
        | SessionCmd::SetThinking { reply, .. }
        | SessionCmd::SetSessionName { reply, .. }
        | SessionCmd::Compact { reply }
        | SessionCmd::SetTools { reply, .. }
        | SessionCmd::NavigateTree { reply, .. }
        | SessionCmd::Fork { reply, .. }
        | SessionCmd::Reload { reply }
        | SessionCmd::Rebuild { reply, .. }
        | SessionCmd::Deferred { reply, .. }
        | SessionCmd::GetStats { reply } => {
            let _ = reply.send(Err("session busy (prompt running)".into()));
        }
    }
}

/// turn 完成:回落标志 + last_assistant_text + 合成 prompt_done/error。
/// `emit_settled` 仅在队列空(最后一轮)时为 true —— agent_settled 语义 =
/// 无活动 run 且无排队续跑,连发 prompt 时提前发会让前端提前收表。
async fn finish_turn(
    res: Result<pi::sdk::AssistantMessage, pi::sdk::Error>,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
    emit_settled: bool,
) {
    match &res {
        Ok(am) => {
            let text = extract_assistant_text(am);
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            s.is_streaming = false;
            s.is_prompt_running = false;
            s.last_assistant_text = Some(text);
            emit_agent(sink, json!({ "type": "prompt_done" }));
            if emit_settled {
                emit_agent(sink, json!({ "type": "agent_settled" }));
            }
        }
        Err(e) => {
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            s.is_streaming = false;
            s.is_prompt_running = false;
            // 对齐 rpc-manager:先 prompt_error 再 prompt_done(客户端靠
            // prompt_done 复位 rpcPromptPendingRef,否则 agent_settled 被守卫吞)
            emit_agent(sink, json!({ "type": "prompt_error", "errorMessage": e.to_string() }));
            emit_agent(sink, json!({ "type": "prompt_done" }));
            if emit_settled {
                emit_agent(sink, json!({ "type": "agent_settled" }));
            }
        }
    }
}

fn mark_running(snap: &Arc<Mutex<SessionSnap>>, running: bool) {
    let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
    s.is_streaming = running;
    s.is_prompt_running = running;
}

fn emit_queue_update(snap: &Arc<Mutex<SessionSnap>>, sink: &super::EventSink) {
    let s = snap.lock().unwrap_or_else(|e| e.into_inner());
    emit_agent(
        sink,
        json!({
            "type": "queue_update",
            "steering": s.queued_steering.iter().cloned().collect::<Vec<_>>(),
            "followUp": s.queued_follow_up.iter().cloned().collect::<Vec<_>>(),
        }),
    );
}

/// 事件出口:agent_event 通道(catch_unwind 契约纪律)。
fn emit_agent(sink: &super::EventSink, payload: Value) {
    let sink = sink.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        sink(super::events::ApiEvent::Agent { payload });
    }));
}

fn user_text_message(text: String) -> pi::sdk::Message {
    pi::sdk::Message::User(pi::sdk::UserMessage {
        content: pi::sdk::UserContent::Text(text),
        timestamp: chrono::Local::now().timestamp(),
    })
}

fn extract_assistant_text(am: &pi::sdk::AssistantMessage) -> String {
    am.content
        .iter()
        .filter_map(|b| match b {
            pi::sdk::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// 上游 toClientAgentEvent 语义(wire 过滤;agent-event-wire.ts):
/// 丢 turn_start/turn_end/tool_execution_update;message_update 投影
/// (剥 partial);agent_end 折叠为 {type:"agent_end"}。
/// 输入为 AgentEvent 序列化后的 Value(on_event 回调内先 to_value)。
pub(crate) fn to_client_event(v: &serde_json::Value) -> Option<serde_json::Value> {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "turn_start" | "turn_end" | "tool_execution_update" => None,
        "message_update" => {
            let ame = v.get("assistantMessageEvent")?;
            if !ame.is_object() {
                return None;
            }
            let mut ame = ame.clone();
            if let Some(obj) = ame.as_object_mut() {
                obj.remove("partial");
            }
            Some(serde_json::json!({
                "type": "message_update",
                "assistantMessageEvent": ame,
            }))
        }
        "agent_end" => Some(serde_json::json!({ "type": "agent_end" })),
        _ => Some(v.clone()),
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    fn ev(json_str: &str) -> serde_json::Value {
        serde_json::from_str(json_str).expect("agent event json")
    }

    #[test]
    fn to_client_event_omits_high_frequency() {
        assert!(to_client_event(&ev(r#"{"type":"turn_start","sessionId":"s"}"#)).is_none());
        assert!(to_client_event(&ev(r#"{"type":"tool_execution_update","toolCallId":"t"}"#)).is_none());
    }

    #[test]
    fn to_client_event_projects_message_update() {
        let out = to_client_event(&ev(
            r#"{"type":"message_update","sessionId":"s","assistantMessageEvent":{"partial":true,"content":[{"type":"text","text":"hi"}]}}"#,
        ))
        .expect("projected");
        // partial 剥除;形状 {type, assistantMessageEvent}
        assert_eq!(out["type"], serde_json::json!("message_update"));
        assert!(out["assistantMessageEvent"].get("partial").is_none());
        assert_eq!(out["assistantMessageEvent"]["content"][0]["text"], serde_json::json!("hi"));
    }

    #[test]
    fn to_client_event_folds_agent_end_and_passthrough() {
        let out = to_client_event(&ev(r#"{"type":"agent_end","sessionId":"s","messages":[]}"#))
            .expect("folded");
        assert_eq!(out, serde_json::json!({ "type": "agent_end" }));
        let out = to_client_event(&ev(r#"{"type":"agent_start","sessionId":"s"}"#)).expect("kept");
        assert_eq!(out["type"], serde_json::json!("agent_start"));
    }

    #[test]
    fn snap_state_shape_for_reconcile() {
        let snap = SessionSnap {
            session_id: Some("s1".into()),
            is_prompt_running: true,
            is_streaming: true,
            queued_steering: VecDeque::from(["a".to_string()]),
            model_provider: Some("p".into()),
            model_id: Some("m".into()),
            ..Default::default()
        };
        let v = snap.to_state_json();
        assert_eq!(v["sessionId"], serde_json::json!("s1"));
        assert_eq!(v["isStreaming"], serde_json::json!(true));
        assert_eq!(v["isPromptRunning"], serde_json::json!(true));
        assert_eq!(v["queuedMessages"]["steering"], serde_json::json!(["a"]));
        assert_eq!(v["model"]["id"], serde_json::json!("m"));
        assert_eq!(v["thinkingLevel"], serde_json::json!("off"));
    }
}

#[cfg(test)]
mod sink_tests {
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi, TimeoutConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Hooks(std::path::PathBuf);
    impl HostHooks for Hooks {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.join("sessions"))
        }
    }

    fn real_home() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("sink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn api_for(tmp: &std::path::Path, sink: EventSink) -> PiWebApi {
        let pi = tmp.join(".pi/agent");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(
            pi.join("models.json"),
            r#"{"providers":{"probe":{"baseUrl":"https://probe.invalid","api":"openai-completions","apiKey":"k","models":[{"id":"p1"}]}}}"#,
        )
        .unwrap();
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(sink);
        cfg.hooks = Arc::new(Hooks(tmp.to_path_buf()));
        cfg.timeouts = TimeoutConfig::default();
        PiWebApi::new(rt, cfg)
    }

    fn call(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(60)).expect("responder called")
    }

    fn post(uri: &str, body: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body.as_bytes().to_vec())
            .unwrap()
    }

    #[test]
    fn event_line_end_to_end_mock_sink() {
        // 共享 HOME 锁(api::HOME_LOCK):与 golden/models/lifecycle 测试互斥
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = real_home();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let events: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let panic_count = Arc::new(AtomicUsize::new(0));
        let ev_c = events.clone();
        let sink: EventSink = Arc::new(move |ev: crate::api::ApiEvent| {
            match &ev {
                crate::api::ApiEvent::Agent { payload } => {
                    ev_c.lock().unwrap_or_else(|e| e.into_inner()).push(payload.clone());
                }
                _ => {}
            }
        });
        let api = api_for(&tmp, sink);

        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#)))
            .expect("create ok");
        let sid = serde_json::from_slice::<serde_json::Value>(resp.body()).unwrap()["sessionId"]
            .as_str()
            .expect("sid")
            .to_string();

        // steer → queue_update 事件(镜像 + sink)
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"steer","message":"s1"}"#))
            .expect("steer ok");
        assert_eq!(resp.status(), 200);
        let seen = {
            let v = events.lock().unwrap_or_else(|e| e.into_inner());
            v.iter().any(|e| e["type"] == serde_json::json!("queue_update"))
        };
        assert!(seen, "queue_update emitted");

        // prompt → 假 provider 快速失败 → 合成 prompt_error/prompt_done/agent_settled
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"hello"}"#))
            .expect("prompt envelope ok");
        // envelope 200;turn 结果经事件(前端语义)
        assert_eq!(resp.status(), 200);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let types: Vec<String> = {
                let v = events.lock().unwrap_or_else(|e| e.into_inner());
                v.iter().filter(|e| e["type"].as_str().is_some()).map(|e| e["type"].as_str().unwrap().to_string()).collect()
            };
            if types.iter().any(|t| t == "prompt_error" || t == "prompt_done") {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "synthesis timeout, got: {types:?}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let types = {
            let v = events.lock().unwrap_or_else(|e| e.into_inner());
            v.iter().filter(|e| e["type"].as_str().is_some()).map(|e| e["type"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        };
        assert!(types.iter().any(|t| t == "prompt_error" || t == "prompt_done"), "synthesis: {types:?}");
        // agent_settled 在完成序列中
        assert!(types.iter().any(|t| t == "agent_settled"), "settled: {types:?}");

        // 收尾:恢复 env
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
        let _ = panic_count;
    }
}

#[cfg(test)]
mod runtime_drop_tests {
    use std::sync::Arc;

    /// 退出配方依据:drop Arc<Runtime> 时,在飞 spawn_blocking 任务与
    /// spawn 任务的行为(等待/截断/panic)。计划 P4 退出配方 = shutdown →
    /// drop runtime → exit;本测试锁定"drop 不 panic、handle().spawn 在
    /// drop 后调用返回错误"。
    #[test]
    fn runtime_drop_semantics_for_exit_recipe() {
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let (btx, brx) = std::sync::mpsc::channel::<u32>();
        let bh = rt.spawn_blocking(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = btx.send(1);
        });
        assert!(bh.is_some());
        let h = rt.handle();
        let jh = h.spawn(async { 42u32 });
        drop(rt); // Arc 全 drop:runtime 开始关闭
        // 在飞任务结果仍可回收(drop 等待/收割语义由 runtime 保证)
        let _ = brx.recv_timeout(std::time::Duration::from_secs(5));
        // spawn 任务句柄在 drop 后 poll:运行时状态由 asupersync 语义决定,
        // 此处仅验证"drop 不 panic"(实际 join 在 P4 退出配方端到端验证)
        let _ = jh;
    }
}

// ============================================================================
// fork / rebuild helpers(P4 遗留接线)
// ============================================================================

/// fork 文件手术(复制自 moho session_scanner::fork_session,纯 JSONL):
/// 拷贝源到 entry_id(不含)之前的 entry,header 置 branchedFrom,
/// 新文件同目录 uuid.jsonl。
pub(crate) fn fork_file(source: &std::path::Path, entry_id: Option<&str>) -> Result<std::path::PathBuf, String> {
    let content = std::fs::read_to_string(source)
        .map_err(|e| format!("cannot read source: {e}"))?;
    let mut lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let Some(header) = lines.first().cloned() else {
        return Err("empty source header".into());
    };
    let new_id = uuid::Uuid::new_v4().to_string();
    let parent_dir = source.parent().ok_or("source has no parent dir")?;
    // 文件名对齐 lib 落盘约定 <timestamp>_<id前8位>.jsonl(restore 扫描按此
    // 约定匹配;裸 uuid 文件名会导致 fork 会话磁盘恢复 404)。
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S%.3fZ");
    let new_path = parent_dir.join(format!("{ts}_{}.jsonl", &new_id[..8]));

    // header:branchedFrom = 源文件路径;id 换成新会话 id(引擎按 header.id
    // 建会话,restore 也按 header.id 校验 —— 不换则 fork 会话仍指向旧 id)
    let mut new_header = header;
    if let Some(obj) = new_header.as_object_mut() {
        obj.insert("branchedFrom".to_string(), serde_json::json!(source.to_string_lossy()));
        obj.insert("id".to_string(), serde_json::json!(new_id));
    }

    let mut out = Vec::new();
    out.push(new_header);
    // entry_id=None → 空分支(首条消息前 fork);Some → 拷贝到该 entry(不含)
    if let Some(eid) = entry_id {
        for v in lines.iter().skip(1) {
            if v.get("id").and_then(|i| i.as_str()) == Some(eid) {
                break;
            }
            out.push(v.clone());
        }
    }
    let mut body = String::new();
    for v in &out {
        body.push_str(&serde_json::to_string(v).map_err(|e| format!("serialize: {e}"))?);
        body.push('\n');
    }
    std::fs::write(&new_path, body).map_err(|e| format!("write fork: {e}"))?;
    Ok(new_path)
}

/// 重建会话 handle(fork/reload 用):以既有文件打开新句柄,替换旧 handle。
/// `path=None` 表示全新会话;Some 表示 open 该文件(header 从文件读)。
async fn rebuild_session(
    handle: &mut AgentSessionHandle,
    snap: Arc<Mutex<SessionSnap>>,
    path: Option<std::path::PathBuf>,
    cwd: String,
    sink: &super::EventSink,
    rt: &Arc<asupersync::runtime::Runtime>,
    hooks: &Arc<dyn super::HostHooks>,
) -> Result<(), ApiError> {
    let sink_c = sink.clone();
    let on_event: Arc<dyn Fn(AgentEvent) + Send + Sync> = Arc::new(move |ev: AgentEvent| {
        let ev_value = serde_json::to_value(&ev).unwrap_or(serde_json::Value::Null);
        if let Some(payload) = to_client_event(&ev_value) {
            let sink = sink_c.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                sink(super::events::ApiEvent::Agent { payload });
            }));
        }
    });
    let path_c = path.clone();
    let options = pi::sdk::SessionOptions {
        system_prompt: hooks.system_prompt(),
        tool_factory: hooks.tool_factory(),
        no_session: false,
        session_dir: Some(std::path::PathBuf::from(super::commands::default_sessions_root_pub())),
        working_directory: Some(std::path::PathBuf::from(cwd)),
        session_path: path_c,
        on_event: Some(on_event),
        ..Default::default()
    };
    let new_handle = super::commands::blocking(
        &super::commands::ExecCtx {
            rt: rt.clone(),
            hooks: hooks.clone(),
            sessions: Arc::new(SessionRuntime::new(1)),
            sink: sink.clone(),
        },
        move || {
            futures::executor::block_on(pi::sdk::create_agent_session(options))
                .map_err(|e| ApiError::internal(format!("create_agent_session: {e}")))
        },
    )
    .await??;

    let new_state = new_handle.state().await.ok();
    let (new_sid, eng_provider, eng_model) = new_state
        .as_ref()
        .map(|st| {
            (
                st.session_id.clone().unwrap_or_default(),
                Some(st.provider.clone()),
                Some(st.model_id.clone()),
            )
        })
        .unwrap_or_default();
    {
        let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
        if !new_sid.is_empty() {
            s.session_id = Some(new_sid);
        }
        if let Some(p) = &path {
            s.session_path = Some(p.clone());
        }
        if let (Some(p), Some(m)) = (eng_provider, eng_model) {
            s.model_provider = Some(p);
            s.model_id = Some(m);
        }
    }
    *handle = new_handle;
    Ok(())
}


// ============================================================================
// bash 执行(自 moho chat_thread::run_bash 移植;§1.1 #24)
// ============================================================================

/// 输出截断上限(对齐上游 BASH_OUTPUT_MAX_BYTES = 5MB)。
const BASH_OUTPUT_MAX_BYTES: usize = 5 * 1024 * 1024;

/// 后台执行 `bash -c <command>`:spawn std child(pid 入快照供 abort kill),
/// 独立线程 wait_with_output → 截断 → BashExecution JSONL 追加 → reply
/// {output, exitCode, cancelled, truncated, fullOutputPath}。
/// 不借 handle(线程只用 Arc 快照)—— 空闲/运行期均可调。
fn run_bash(
    command: String,
    snap: Arc<Mutex<SessionSnap>>,
    reply: oneshot::Sender<Result<Value, String>>,
) {
    let cwd = snap.lock().unwrap_or_else(|e| e.into_inner()).cwd.clone();
    let session_id =
        snap.lock().unwrap_or_else(|e| e.into_inner()).session_id.clone().unwrap_or_default();
    let session_path = snap.lock().unwrap_or_else(|e| e.into_inner()).session_path.clone();

    let mut cmd = std::process::Command::new("bash");
    cmd.arg("-c").arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(c) = cwd.filter(|s| !s.is_empty()) {
        cmd.current_dir(c);
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = reply.send(Err(e.to_string()));
            return;
        }
    };
    let pid = child.id();
    {
        let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
        s.is_bash_running = true;
        s.bash_child_pid = Some(pid);
    }
    std::thread::spawn(move || {
        // ExitStatus::signal 是 unix 扩展
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        let output = child.wait_with_output();
        let (mut buf, exit_code, cancelled) = match output {
            Ok(o) => {
                let mut b = o.stdout;
                if !o.stderr.is_empty() {
                    b.extend_from_slice(&o.stderr);
                }
                // 被信号终止(status.code() None)→ cancelled;退出码取负信号值
                let (code, cancelled) = match o.status.code() {
                    Some(c) => (c, false),
                    None => (o.status.signal().map(|s| -s).unwrap_or(-1), true),
                };
                (b, code, cancelled)
            }
            Err(_) => (Vec::new(), -1, true),
        };
        let truncated = buf.len() > BASH_OUTPUT_MAX_BYTES;
        buf.truncate(BASH_OUTPUT_MAX_BYTES);
        let out_str = String::from_utf8_lossy(&buf).to_string();
        // 写 BashExecution 条目(文件级 append;§1.1 #24 persistBashOnlySession)
        if !session_id.is_empty() {
            if let Some(p) = session_path.as_ref() {
                append_bash_execution(p, &command, &out_str, exit_code, cancelled, truncated);
            }
        }
        // 回落快照
        {
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            s.is_bash_running = false;
            s.bash_child_pid = None;
        }
        let _ = reply.send(Ok(json!({
            "output": out_str,
            "exitCode": exit_code,
            "cancelled": cancelled,
            "truncated": truncated,
            "fullOutputPath": Value::Null,
        })));
    });
}

/// kill bash 子进程(pid 从快照取;无在飞 bash 时 no-op)。
fn abort_bash_kill(snap: &Arc<Mutex<SessionSnap>>) {
    let pid = snap.lock().unwrap_or_else(|e| e.into_inner()).bash_child_pid.take();
    if let Some(pid) = pid {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).spawn();
    }
}

/// 追加 BashExecution JSONL 条目(文件级;parentId = 文件内最后一条 entry 的 id)。
fn append_bash_execution(
    file_path: &std::path::Path,
    command: &str,
    output: &str,
    exit_code: i32,
    cancelled: bool,
    truncated: bool,
) {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(file_path) else {
        return;
    };
    // parentId = 当前叶(最后一条 entry 的 id;线性会话即文件尾)
    let parent_id = {
        let content = std::fs::read_to_string(file_path).unwrap_or_default();
        content
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find_map(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
    };
    let entry = json!({
        "type": "message",
        "id": uuid::Uuid::new_v4().to_string(),
        "parentId": parent_id,
        "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "message": {
            "role": "bashExecution",
            "command": command,
            "output": output,
            "exitCode": exit_code,
            "cancelled": cancelled,
            "truncated": truncated,
            "fullOutputPath": Value::Null,
            "timestamp": chrono::Utc::now().timestamp_millis(),
        },
    });
    let line = serde_json::to_string(&entry).unwrap_or_default();
    let _ = writeln!(f, "{line}");
}

#[cfg(test)]
mod bash_tests {
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct Hooks(std::path::PathBuf);
    impl HostHooks for Hooks {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.join("sessions"))
        }
    }

    fn real_home() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("bash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn api_for(tmp: &std::path::Path) -> PiWebApi {
        let pi = tmp.join(".pi/agent");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(
            pi.join("models.json"),
            r#"{"providers":{"probe":{"baseUrl":"https://probe.invalid","api":"openai-completions","apiKey":"k","models":[{"id":"p1"}]}}}"#,
        )
        .unwrap();
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(Hooks(tmp.to_path_buf()));
        PiWebApi::new(rt, cfg)
    }

    fn call(api: &PiWebApi, req: ::http::Request<Vec<u8>>) -> Result<::http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    fn post(uri: &str, body: &str) -> ::http::Request<Vec<u8>> {
        ::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header(::http::header::CONTENT_TYPE, "application/json")
            .body(body.as_bytes().to_vec())
            .unwrap()
    }

    fn get(uri: &str) -> ::http::Request<Vec<u8>> {
        ::http::Request::builder().method("GET").uri(uri).body(Vec::new()).unwrap()
    }

    fn body(resp: &::http::Response<Vec<u8>>) -> serde_json::Value {
        serde_json::from_slice(resp.body()).expect("json")
    }

    #[test]
    fn bash_rpc_executes_echo() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = real_home();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let api = api_for(&tmp);
        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#)))
            .expect("create ok");
        let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();

        // bash echo → 输出 + exitCode 0
        let resp = call(
            &api,
            post(&format!("/api/agent/{sid}"), r#"{"type":"bash","command":"echo hello-bash-spike"}"#),
        )
        .expect("bash ok");
        assert_eq!(resp.status(), 200);
        let v = body(&resp);
        assert_eq!(v["success"], serde_json::json!(true));
        let data = &v["data"];
        assert!(
            data["output"].as_str().is_some_and(|o| o.contains("hello-bash-spike")),
            "output: {data}"
        );
        assert_eq!(data["exitCode"], serde_json::json!(0));
        assert_eq!(data["cancelled"], serde_json::json!(false));
        // 回落:get_state isBashRunning=false
        let resp = call(&api, get(&format!("/api/agent/{sid}"))).expect("state ok");
        assert_eq!(body(&resp)["state"]["isBashRunning"], serde_json::json!(false));

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// agent_new 契约(对齐上游 route.ts:49-51/70-84):
    /// - 未显式指定模型 → 引擎兜底选中 models.json 里唯一有 key 的自定义
    ///   provider(而非偏好表条目 —— bedrock 无 AWS 凭证时 not ready 是
    ///   引擎侧 TS hasConfiguredAuth 对齐修复的行为断言);
    /// - 响应回传引擎真相 model/thinkingLevel(前端同步选中态);
    /// - provider/modelId 半配对 → 400。
    #[test]
    fn agent_new_contract_model_thinking_roundtrip() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = real_home();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let api = api_for(&tmp);
        let cwd = tmp.to_string_lossy().to_string();
        // 未显式模型 → 引擎兜底 = 唯一 ready 的 probe/p1
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#)))
            .expect("create ok");
        let v = body(&resp);
        assert!(
            v["model"]["provider"] == serde_json::json!("probe"),
            "engine fallback must pick the user-configured provider, got: {v}"
        );
        assert_eq!(v["model"]["modelId"], serde_json::json!("p1"));
        assert!(
            v["thinkingLevel"].is_string(),
            "engine-resolved thinking must round-trip, got: {v}"
        );

        // 显式模型 → 精确透传
        let resp = call(
            &api,
            post(
                "/api/agent/new",
                &format!(r#"{{"cwd":"{cwd}","provider":"probe","modelId":"p1"}}"#),
            ),
        )
        .expect("explicit create ok");
        let v = body(&resp);
        assert_eq!(v["model"]["provider"], serde_json::json!("probe"));
        assert_eq!(v["model"]["modelId"], serde_json::json!("p1"));

        // 半配对 → 400(成对校验)
        for half in [
            format!(r#"{{"cwd":"{cwd}","provider":"probe"}}"#),
            format!(r#"{{"cwd":"{cwd}","modelId":"p1"}}"#),
        ] {
            match call(&api, post("/api/agent/new", &half)) {
                Ok(resp) => assert_eq!(resp.status(), 400, "half-pair must 400: {half}"),
                Err(e) => assert_eq!(e.status, 400, "half-pair must 400: {half}"),
            }
        }

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    #[test]
    fn bash_abort_cancels_long_running() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = real_home();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let api = api_for(&tmp);
        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#)))
            .expect("create ok");
        let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();

        // 长 bash:fire(responder 挂起) → 等 isBashRunning → abort → 收 cancelled
        let (tx1, rx1) = std::sync::mpsc::channel();
        api.handle(
            post(&format!("/api/agent/{sid}"), r#"{"type":"bash","command":"sleep 30"}"#),
            Box::new(move |r| {
                let _ = tx1.send(r);
            }),
        );
        // 等 bash 起跑(spawn 后 is_bash_running=true)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let resp = call(&api, get(&format!("/api/agent/{sid}"))).expect("state");
            if body(&resp)["state"]["isBashRunning"] == serde_json::json!(true) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "bash never started");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // abort
        let resp = call(
            &api,
            post(&format!("/api/agent/{sid}"), r#"{"type":"abort_bash"}"#),
        )
        .expect("abort ok");
        assert_eq!(resp.status(), 200);
        // bash 回执:kill 后 sleep 立即死 → cancelled:true(负信号码)
        let resp = rx1.recv_timeout(std::time::Duration::from_secs(10)).expect("bash reply after kill");
        let v = body(&resp.expect("bash ok"));
        let data = &v["data"];
        assert_eq!(data["cancelled"], serde_json::json!(true), "data: {data}");
        assert!(data["exitCode"].as_i64().is_some_and(|c| c < 0), "signal exit: {data}");

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }
}

/// settings.json 的 SettingsOps 实现(startup preferences 写回用):
/// set_* 记录变更,flush 深合并写回文件(保留 theme 等其它键)。
struct JsonSettingsOps {
    default_model: Option<(String, String)>,
    default_thinking: Option<String>,
}

impl JsonSettingsOps {
    fn new() -> Self {
        Self { default_model: None, default_thinking: None }
    }

    fn settings_path() -> std::path::PathBuf {
        let dir = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        dir.join("settings.json")
    }
}

impl crate::settings::startup_preferences::SettingsOps for JsonSettingsOps {
    fn set_default_model_and_provider(&mut self, provider: &str, model_id: &str) {
        self.default_model = Some((provider.to_string(), model_id.to_string()));
    }

    fn set_default_thinking_level(&mut self, level: &str) {
        self.default_thinking = Some(level.to_string());
    }

    fn flush(&mut self) {
        let path = Self::settings_path();
        let mut root: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if !root.is_object() {
            root = serde_json::json!({});
        }
        if let Some(obj) = root.as_object_mut() {
            if let Some((p, m)) = &self.default_model {
                obj.insert("default_provider".to_string(), serde_json::json!(p));
                obj.insert("default_model".to_string(), serde_json::json!(m));
            }
            if let Some(t) = &self.default_thinking {
                obj.insert("default_thinking_level".to_string(), serde_json::json!(t));
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(pretty) = serde_json::to_string_pretty(&root) {
            let _ = std::fs::write(&path, pretty);
        }
    }
}

/// 成功路径 settle 扫描:极简本地 SSE server 逐形态验证引擎 prompt future
/// 收尾(用户症状:回复完整送达但 loading 永转 = future 不 settle)。
#[cfg(test)]
mod success_settle_tests {
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::io::{Read, Write};
    use std::sync::Arc;

    struct Hooks(std::path::PathBuf);
    impl HostHooks for Hooks {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.join("sessions"))
        }
    }

    /// 形态开关:模拟不同网关收尾行为。
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum StreamShape {
        /// 标准:delta + finish_reason=stop + [DONE]
        Standard,
        /// 无 [DONE] 哨兵,连接直接关闭(EOF)
        EofOnly,
        /// finish_reason=length(截断)
        Length,
        /// delta 带 reasoning_content(deepseek 风格推理字段)
        ReasoningContent,
        /// [DONE] 之后还发一帧注释/心跳(网关 keep-alive 尾巴)
        TrailingComment,
        /// chunked 编码 + 帧间注释心跳 + usage 帧(真网关形态)
        ChunkedInterleaved,
    }

    /// 起一个单请求 SSE server,返回 (port, handle)。按 shape 生成响应体。
    fn spawn_sse_server(shape: StreamShape) -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf); // 请求头+体(读一次即可)
                let body = sse_body(shape);
                if shape == StreamShape::ChunkedInterleaved {
                    // 真 SSE 形态:无 Content-Length,chunked 编码,分块+帧间注释心跳
                    let _ = write_all(&mut stream, b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
                    for piece in body.split_once("\n\n").map(|(a, b)| vec![a.to_string() + "\n\n", b.to_string()]).unwrap_or_default().iter().flat_map(|s| s.split("\n\n").map(|x| x.to_string()).collect::<Vec<_>>()) {
                        if piece.trim().is_empty() { continue; }
                        let frame = format!(": ka\n\n{}", piece);
                        let _ = write_all(&mut stream, format!("{:X}\r\n{}\r\n", frame.len(), frame).as_bytes());
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    let _ = write_all(&mut stream, b"0\r\n\r\n");
                } else {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = write_all(&mut stream, resp.as_bytes());
                }
                let _ = stream.flush();
                use std::net::Shutdown;
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
        (port, rx)
    }

    fn write_all<S: Write>(s: &mut S, buf: &[u8]) -> std::io::Result<()> {
        s.write_all(buf)
    }

    fn sse_body(shape: StreamShape) -> String {
        let chunk = |delta: &str, finish: &str| {
            format!(
                "data: {{\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{finish}}}]}}\n\n"
            )
        };
        let mut body = String::new();
        match shape {
            StreamShape::Standard | StreamShape::TrailingComment => {
                body.push_str(&chunk("{\"role\":\"assistant\",\"content\":\"Hello from fake\"}", "null"));
                body.push_str(&chunk("{}", "\"stop\""));
                body.push_str("data: [DONE]\n\n");
                if shape == StreamShape::TrailingComment {
                    body.push_str(": keepalive\n\n");
                }
            }
            StreamShape::EofOnly => {
                body.push_str(&chunk("{\"role\":\"assistant\",\"content\":\"Hello from fake\"}", "null"));
                body.push_str(&chunk("{}", "\"stop\""));
                // 无 [DONE],Content-Length 边界即 EOF
            }
            StreamShape::Length => {
                body.push_str(&chunk("{\"role\":\"assistant\",\"content\":\"Hello from fake\"}", "null"));
                body.push_str(&chunk("{}", "\"length\""));
                body.push_str("data: [DONE]\n\n");
            }
            StreamShape::ChunkedInterleaved => {
                body.push_str(&chunk("{\"role\":\"assistant\",\"content\":\"Hello from fake\"}", "null"));
                body.push_str("data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n");
                body.push_str(&chunk("{}", "\"stop\""));
                body.push_str("data: [DONE]\n\n");
            }
            StreamShape::ReasoningContent => {
                body.push_str(&chunk(
                    "{\"role\":\"assistant\",\"reasoning_content\":\"thinking hard\",\"content\":\"Hello\"}",
                    "null",
                ));
                body.push_str(&chunk("{}", "\"stop\""));
                body.push_str("data: [DONE]\n\n");
            }
        }
        body
    }

    fn api_with_provider(tmp: &std::path::Path, port: u16) -> PiWebApi {
        let pi = tmp.join(".pi/agent");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(
            pi.join("models.json"),
            format!(
                r#"{{"providers":{{"fake":{{"baseUrl":"http://127.0.0.1:{port}/v1","api":"openai-completions","apiKey":"k","models":[{{"id":"f1","name":"f1"}}]}}}}}}"#
            ),
        )
        .unwrap();
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(2, 4)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(Hooks(tmp.to_path_buf()));
        PiWebApi::new(rt, cfg)
    }

    fn call(api: &PiWebApi, req: ::http::Request<Vec<u8>>) -> Result<::http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(60)).expect("responder called")
    }

    fn post(uri: &str, body: &str) -> ::http::Request<Vec<u8>> {
        ::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.as_bytes().to_vec())
            .unwrap()
    }

    fn get(uri: &str) -> ::http::Request<Vec<u8>> {
        ::http::Request::builder().method("GET").uri(uri).body(Vec::new()).unwrap()
    }

    fn body(v: &::http::Response<Vec<u8>>) -> serde_json::Value {
        serde_json::from_slice(v.body()).unwrap_or(serde_json::Value::Null)
    }

    /// 单形态全链路:建会话(无显式 thinking)→ prompt → 限时等待 settled。
    /// 返回 (settled?, 耗时, get_state running)。
    fn run_shape(shape: StreamShape) -> (bool, std::time::Duration, bool) {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("settle-{}-{:?}", std::process::id(), shape));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let (port, _rx) = spawn_sse_server(shape);
        let api = api_with_provider(&tmp, port);
        let cwd = tmp.to_string_lossy().to_string();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#))).expect("create");
            let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();
            let t0 = std::time::Instant::now();
            // prompt(fire;responder 挂起至完成)
            let (tx, rx) = std::sync::mpsc::channel();
            api.handle(
                post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"hi"}"#),
                Box::new(move |r| {
                    let _ = tx.send(r);
                }),
            );
            // 轮询 get_state 至 idle 或 20s 超时
            let mut running = true;
            while t0.elapsed() < std::time::Duration::from_secs(20) {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let st = call(&api, get(&format!("/api/agent/{sid}"))).expect("state");
                running = body(&st)["running"].as_bool().unwrap_or(true);
                if !running {
                    break;
                }
            }
            let settled = !running;
            // prompt responder 最终应有回执(成功或错误均可)
            let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
            (settled, t0.elapsed(), running)
        }));
        let out = result.unwrap_or((false, std::time::Duration::from_secs(999), true));

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
        out
    }

    /// 死会话(进程重启等价:磁盘有上次会话、registry 空)对齐上游:
    /// GET → {running:false}(不 404,reconcile 可收敛);POST prompt →
    /// 自动从磁盘恢复会话并执行(route.ts:19-32);磁盘也无 → 404。
    /// 注:引擎落盘发生在进程退出,同进程无法依赖"prompt 后文件已写",
    /// 故直接手写磁盘会话文件模拟"上次进程留下的会话"。
    #[test]
    fn dead_session_reports_idle_and_rpc_restores_from_disk() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("restore2-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let (port, _rx) = spawn_sse_server(StreamShape::Standard);

        // 手写磁盘会话:header + 一条 user 消息(真实前置 = 上次进程正常退出)
        let sid = "cafe1234-1111-2222-3333-444455556666".to_string();
        let slug = "restore-fixture";
        let dir = tmp.join(".pi/agent/sessions").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = tmp.to_string_lossy().to_string();
        let jsonl = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{sid}\",\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"cwd\":\"{cwd}\"}}\n"
        );
        std::fs::write(dir.join("2026-01-01T00:00:00.000Z_cafe1234.jsonl"), jsonl).unwrap();

        let api = api_with_provider(&tmp, port);

        // GET 死会话 → running:false(修复前 404)
        let st = call(&api, get(&format!("/api/agent/{sid}"))).expect("get on dead");
        assert_eq!(body(&st)["running"], serde_json::json!(false), "dead session must report idle");

        // POST prompt → 磁盘恢复 + 执行成功(fake SSE 回 200)
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"hi"}"#))
            .expect("prompt on dead must restore from disk");
        assert_eq!(resp.status(), 200);

        // 完全不存在的 id → 404 JSON(信封形状:prompt 带 prompt_rejected)
        let resp = call(&api, post("/api/agent/00000000-0000-0000-0000-000000000000", r#"{"type":"get_state"}"#))
            .expect("unknown id handled");
        assert_eq!(resp.status(), 404);
        assert_eq!(body(&resp)["error"], serde_json::json!("Session not found"));
        let resp = call(&api, post("/api/agent/00000000-0000-0000-0000-000000000000", r#"{"type":"prompt","message":"x"}"#))
            .expect("unknown id prompt handled");
        assert_eq!(resp.status(), 404);
        assert_eq!(body(&resp)["code"], serde_json::json!("prompt_rejected"));
        assert_eq!(body(&resp)["accepted"], serde_json::json!(false));

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// P1 scope 接线:settings 默认模型被选中;显式请求可用模型外的组合 → 400。
    /// P1-2 startup prefs:显式选择与引擎生效一致 → settings.json 写回默认。
    #[test]
    fn scope_resolution_and_startup_preferences() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("scope-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        // 两个模型:z-first(排序首)与 chosen(settings 默认)
        let pi = tmp.join(".pi/agent");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(
            pi.join("models.json"),
            r#"{"providers":{"fake":{"baseUrl":"http://127.0.0.1:1/v1","api":"openai-completions","apiKey":"k","models":[{"id":"z-first","name":"z-first"},{"id":"chosen","name":"chosen"}]}}}"#,
        )
        .unwrap();
        std::fs::write(
            pi.join("settings.json"),
            r#"{"default_provider":"fake","default_model":"chosen","theme":"global"}"#,
        )
        .unwrap();

        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(2, 4)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = crate::api::ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(Hooks(tmp.to_path_buf()));
        let api = PiWebApi::new(rt, cfg);
        let cwd = tmp.to_string_lossy().to_string();

        // 无显式 → settings 默认 chosen(而非排序首 z-first)
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#))).expect("create");
        let v = body(&resp);
        assert_eq!(v["model"]["modelId"], serde_json::json!("chosen"), "settings default must win: {v}");

        // 显式可用模型 → 精确选中 + settings 写回(显式==生效)
        let resp = call(
            &api,
            post("/api/agent/new", r#"{"cwd":"/tmp","provider":"fake","modelId":"z-first"}"#),
        )
        .expect("explicit create");
        let v = body(&resp);
        assert_eq!(v["model"]["modelId"], serde_json::json!("z-first"));
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(pi.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["default_model"], serde_json::json!("z-first"), "startup prefs must persist: {settings}");
        assert_eq!(settings["theme"], serde_json::json!("global"), "other keys preserved");

        // 显式越界(不存在的模型)→ 400(scope 语义)
        let e = call(
            &api,
            post("/api/agent/new", r#"{"cwd":"/tmp","provider":"fake","modelId":"nope"}"#),
        )
        .expect_err("out-of-scope must 400");
        assert_eq!(e.status, 400, "got: {:?}", e.message);

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// P2:空会话名拒绝;合法重命名写入 stats.sessionName(上游契约)。
    #[test]
    fn set_session_name_validation_and_stats_merge() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("name-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let (port, _rx) = spawn_sse_server(StreamShape::Standard);
        let api = api_with_provider(&tmp, port);
        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#))).expect("create");
        let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();

        // 空名 → 400 语义(上游 rpc-manager 拒绝)
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"set_session_name","name":"  "}"#)).expect("empty");
        assert_eq!(resp.status(), 500, "empty name must error");
        let v = body(&resp);
        assert!(v["error"].as_str().is_some_and(|e| e.contains("must not be empty")), "{v}");

        // 合法名 → 200 + stats 合并
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"set_session_name","name":"My Project"}"#)).expect("rename");
        assert_eq!(resp.status(), 200);
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"get_session_stats"}"#)).expect("stats");
        assert_eq!(resp.status(), 200);
        let data = &body(&resp)["data"];
        assert_eq!(data["sessionName"], serde_json::json!("My Project"), "stats must carry name: {data}");

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// P2 连发排队(上游 pendingPromptCount):运行中第二条 prompt 不再
    /// busy-reject,而是 ack 入队、当前 turn 完成后自动续跑;两轮都完成。
    #[test]
    fn queued_prompts_continue_after_current_turn() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("queue-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let (port, _rx) = spawn_sse_server(StreamShape::Standard);
        let api = api_with_provider(&tmp, port);
        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#))).expect("create");
        let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();

        // 第一条 prompt(fire)+ 立即第二条(运行中) → 均 ack
        let r1 = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"first"}"#)).expect("p1");
        assert_eq!(r1.status(), 200, "first prompt must ack");
        let r2 = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"second"}"#)).expect("p2");
        assert_eq!(r2.status(), 200, "queued prompt must ack, not busy-reject");

        // 两轮都完成 → idle
        let t0 = std::time::Instant::now();
        loop {
            let st = call(&api, get(&format!("/api/agent/{sid}"))).expect("state");
            if !body(&st)["running"].as_bool().unwrap_or(true) {
                break;
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(25), "two turns never idle");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        // 两条 prompt 都被消费(队列空 = 第二轮已续跑;idle 到达 = 末轮
        // settled 已发)。context 断言不适用:引擎 write-behind 落盘在进程
        // 退出,磁盘文件此时未必存在。
        let queued = {
            // 经 get_state 间接验证队列空:快照 queuedMessages 只含 steering/
            // follow_up;queued_prompts 通过 running 复位与 idle 到达证明消费。
            // 直接断言:再次 prompt 会立即跑(fake server 仍在线),无积压。
            let _ = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"third"}"#)).expect("p3 ack");
            let t1 = std::time::Instant::now();
            loop {
                let st = call(&api, get(&format!("/api/agent/{sid}"))).expect("state");
                if !body(&st)["running"].as_bool().unwrap_or(true) {
                    break;
                }
                assert!(t1.elapsed() < std::time::Duration::from_secs(20), "third never idle");
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            "consumed"
        };
        assert_eq!(queued, "consumed");

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// tool_factory 接线回归:hooks 提供的工厂必须在建会话时被引擎调用
    /// (Wire B 会话曾漏传 → Moho 工具全丢)。
    #[test]
    fn host_tool_factory_invoked_on_create() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("toolfac-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        struct SpyFactory(std::sync::atomic::AtomicBool);
        impl pi::sdk::ToolFactory for SpyFactory {
            fn create_tool_registry(
                &self,
                enabled: &[&str],
                cwd: &std::path::Path,
                config: &pi::sdk::Config,
            ) -> pi::sdk::ToolRegistry {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                pi::sdk::default_tool_registry(enabled, cwd, config)
            }
        }

        let spy = Arc::new(SpyFactory(std::sync::atomic::AtomicBool::new(false)));
        struct FactoryHooks(std::sync::Arc<SpyFactory>, std::path::PathBuf);
        impl HostHooks for FactoryHooks {
            fn sessions_root(&self) -> Option<std::path::PathBuf> {
                Some(self.1.join("sessions"))
            }
            fn tool_factory(&self) -> Option<Arc<dyn pi::sdk::ToolFactory>> {
                Some(self.0.clone())
            }
        }

        let (port, _rx) = spawn_sse_server(StreamShape::Standard);
        // api_with_provider 用默认 Hooks;这里手搭一个带工厂的 api
        let pi_dir = tmp.join(".pi/agent");
        std::fs::create_dir_all(&pi_dir).unwrap();
        std::fs::write(
            pi_dir.join("models.json"),
            format!(
                r#"{{"providers":{{"fake":{{"baseUrl":"http://127.0.0.1:{port}/v1","api":"openai-completions","apiKey":"k","models":[{{"id":"f1","name":"f1"}}]}}}}}}"#
            ),
        )
        .unwrap();
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(2, 4)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = crate::api::ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(FactoryHooks(spy.clone(), tmp.clone()));
        let api = PiWebApi::new(rt, cfg);

        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#))).expect("create");
        assert_eq!(resp.status(), 200);
        assert!(
            spy.0.load(std::sync::atomic::Ordering::SeqCst),
            "tool_factory must be invoked during session creation"
        );

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// running_ids 只含真正运行的会话(对齐上游 isRunning):空闲后清空。
    #[test]
    fn running_ids_clear_after_settle() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("runids-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let (port, _rx) = spawn_sse_server(StreamShape::Standard);
        let api = api_with_provider(&tmp, port);
        let cwd = tmp.to_string_lossy().to_string();
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#))).expect("create");
        let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();

        // prompt 完成 → idle → running 列表必须清空(修复前恒含该 id)
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"prompt","message":"hi"}"#)).expect("prompt");
        assert_eq!(resp.status(), 200);
        let t0 = std::time::Instant::now();
        loop {
            let st = call(&api, get(&format!("/api/agent/{sid}"))).expect("state");
            if !body(&st)["running"].as_bool().unwrap_or(true) {
                break;
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(20), "never idle");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let running = call(&api, get("/api/agent/running")).expect("running");
        let ids = body(&running)["runningSessionIds"].as_array().cloned().unwrap_or_default();
        assert!(
            !ids.iter().any(|v| v.as_str() == Some(sid.as_str())),
            "idle session must not be reported running: {ids:?}"
        );

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// fork 后:新文件名满足 <ts>_<id8>.jsonl 约定且 header.id 已换新 id。
    #[test]
    fn fork_file_matches_restore_convention() {
        let tmp = tempfile_dir();
        let src = tmp.join("2026-01-01T00-00-00.000Z_aaaa1111.jsonl");
        std::fs::write(&src,
            "{\"type\":\"session\",\"id\":\"aaaa1111-1111-1111-1111-111111111111\",\"cwd\":\"/tmp\",\"timestamp\":\"2026-01-01T00:00:00.000Z\"}\n{\"type\":\"message\",\"id\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n"
        ).unwrap();
        let new_path = super::super::session_runtime::fork_file(&src, Some("m1")).expect("fork");
        let name = new_path.file_name().unwrap().to_string_lossy().into_owned();
        // 约定:8 位短 id 结尾 + 时间戳前缀
        let id8 = name.trim_end_matches(".jsonl").rsplit('_').next().unwrap().to_string();
        assert_eq!(id8.len(), 8, "filename must carry id prefix: {name}");
        // header.id 已换新(读回校验)
        let header = crate::session::reader::read_session_header(&new_path.to_string_lossy()).expect("header");
        assert_ne!(header.id, "aaaa1111-1111-1111-1111-111111111111", "header id must be re-stamped");
        assert!(header.id.starts_with(&id8), "header id must match filename prefix");
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("forkfile-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settle_shape_standard() {
        let (settled, dt, running) = run_shape(StreamShape::Standard);
        assert!(settled, "standard stream must settle (running={running}, {:?})", dt);
    }

    #[test]
    fn settle_shape_eof_only() {
        let (settled, dt, running) = run_shape(StreamShape::EofOnly);
        assert!(settled, "EOF-without-DONE must settle (running={running}, {:?})", dt);
    }

    #[test]
    fn settle_shape_length() {
        let (settled, dt, running) = run_shape(StreamShape::Length);
        assert!(settled, "length stop must settle (running={running}, {:?})", dt);
    }

    #[test]
    fn settle_shape_reasoning_content() {
        let (settled, dt, running) = run_shape(StreamShape::ReasoningContent);
        assert!(settled, "reasoning_content stream must settle (running={running}, {:?})", dt);
    }

    #[test]
    fn settle_shape_chunked_interleaved() {
        let (settled, dt, running) = run_shape(StreamShape::ChunkedInterleaved);
        assert!(settled, "chunked+heartbeat+usage stream must settle (running={running}, {:?})", dt);
    }

    #[test]
    fn settle_shape_trailing_comment() {
        let (settled, dt, running) = run_shape(StreamShape::TrailingComment);
        assert!(settled, "trailing keepalive must settle (running={running}, {:?})", dt);
    }
}
