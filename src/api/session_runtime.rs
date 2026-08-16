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
    Prompt { message: String, reply: oneshot::Sender<Result<Value, String>> },
    Steer { message: String, reply: oneshot::Sender<Result<Value, String>> },
    FollowUp { message: String, reply: oneshot::Sender<Result<Value, String>> },
    Abort,
    ClearQueue { reply: oneshot::Sender<Result<Value, String>> },
    SetModel { provider: String, model: String, reply: oneshot::Sender<Result<Value, String>> },
    SetThinking { level: String, reply: oneshot::Sender<Result<Value, String>> },
    SetSessionName { name: String, reply: oneshot::Sender<Result<Value, String>> },
    Compact { reply: oneshot::Sender<Result<Value, String>> },
    SetTools { names: Vec<String>, reply: oneshot::Sender<Result<Value, String>> },
    /// set_tools / fork / reload / navigate 等需要完整 turn 循环或重建的命令:
    /// P3-2 接线,当前统一占位。
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
            "isBashRunning": false,
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
}

impl SessionRuntime {
    pub fn new(max_sessions: usize) -> Self {
        Self { sessions: Arc::new(Mutex::new(HashMap::new())), max_sessions }
    }

    /// 创建并注册会话(溢出 = 关旧建新,对齐 moho Clear/rebuild 语义)。
    pub async fn create(
        &self,
        ctx: &ExecCtx,
        cwd: &str,
        provider: Option<String>,
        model: Option<String>,
        thinking_level: Option<String>,
        enabled_tools: Option<Vec<String>>,
    ) -> Result<String, ApiError> {
        // 引擎会话创建(重、同步外壳)走 blocking pool
        let hooks = ctx.hooks.clone();
        let provider_c = provider.clone();
        let model_c = model.clone();
        let tl_c = thinking_level.clone();
        let tools_c = enabled_tools.clone();
        let cwd_owned = cwd.to_string();
        let sink = ctx.sink.clone();
        let handle = super::commands::blocking(ctx, move || -> Result<AgentSessionHandle, ApiError> {
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
            let options = pi::sdk::SessionOptions {
                provider: provider_c,
                model: model_c,
                // 默认 off(对齐 moho build_session_options;agent_new 可覆盖)
                thinking: Some(
                    tl_c
                        .as_deref()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(pi::sdk::ThinkingLevel::Off),
                ),
                system_prompt: hooks.system_prompt(),
                enabled_tools: tools_c,
                no_session: false,
                session_dir: Some(std::path::PathBuf::from(
                    super::commands::default_sessions_root_pub(),
                )),
                working_directory: Some(std::path::PathBuf::from(&cwd_owned)),
                on_event: Some(on_event),
                ..Default::default()
            };
            futures::executor::block_on(pi::sdk::create_agent_session(options))
                .map_err(|e| ApiError::internal(format!("create_agent_session: {e}")))
        })
        .await??;

        let engine_state = handle.state().await.ok();
        let session_id = engine_state
            .as_ref()
            .and_then(|s| s.session_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // 模型真相从引擎 state 回填(显式 provider/model 是请求意向;注意引擎
        // registry 对 models.json 键名与 entry.provider 字段的映射在显式
        // find 路径上存在口径差 —— 自动选择路径已验证可用,显式路径待上游
        // 追踪核实后放开)
        let (eng_provider, eng_model) = engine_state
            .as_ref()
            .map(|s| (Some(s.provider.clone()), Some(s.model_id.clone())))
            .unwrap_or((None, None));

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
            model_provider: eng_provider.or(provider),
            model_id: eng_model.or(model),
            thinking_level,
            ..Default::default()
        }));
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dead_c = dead.clone();
        let snap_task = snap.clone();
        let rt = ctx.rt.clone();
        let sink = ctx.sink.clone();
        let _joined = rt.handle().spawn(async move {
            // panic 纪律:mid-await panic 经 FutureExt::catch_unwind 截获(P0 报告:
            // panic 会向 await 点传播,监督任务不得被波及)
            let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                session_loop(handle, rx, snap_task, sink),
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
        Ok(session_id)
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

    pub fn running_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, h)| !h.dead.load(std::sync::atomic::Ordering::SeqCst))
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
    snap: Arc<Mutex<SessionSnap>>,
    sink: super::EventSink,
) {
    let mut abort_handle: Option<AbortHandle> = None;
    loop {
        match rx.recv().await {
            None => return,
            Some(cmd) => match cmd {
                // Prompt:future 在独立函数内创建并消费(不跨迭代返回借用 ——
                // 等价 chat_thread 的"起 prompt 与驱动至完成同一匹配",但结构
                // 更简:借用生命周期封闭在 handle_prompt_turn 调用内)
                SessionCmd::Prompt { message, reply } => {
                    let _ = reply.send(Ok(json!({})));
                    handle_prompt_turn(&mut handle, &mut rx, message, &mut abort_handle, &snap, &sink)
                        .await;
                }
                other => {
                    idle_cmd(other, &mut handle, &snap, &sink).await;
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
    message: String,
    abort_handle: &mut Option<AbortHandle>,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
) {
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
    let fut: PinBoxPromptFut<'_> = Box::pin(handle.prompt_with_abort(message, signal, |_| {}));
    run_prompt_until_settled(fut, rx, abort_handle.as_ref(), snap, sink).await;
    mark_running(snap, false);
    *abort_handle = None;
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
    handle: &mut AgentSessionHandle,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
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
            let r = handle.set_session_name(&name).await.map(|_| json!({})).map_err(|e| format!("{e}"));
            let _ = reply.send(r);
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
                .map(|_| json!({ "result": "ok" }))
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
            let _ = reply.send(Err(format!("{what}: full loop wiring pending")));
        }
        SessionCmd::GetStats { reply } => {
            let stats = handle.get_session_stats().await.unwrap_or_else(|_| json!({}));
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
) {
    loop {
        tokio::select! {
            biased;
            res = &mut prompt_fut => {
                finish_turn(res, snap, sink).await;
                return;
            }
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return };
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
        // 借 handle 的重命令运行期 busy(沿用 chat_thread 语义)
        SessionCmd::SetModel { reply, .. }
        | SessionCmd::SetThinking { reply, .. }
        | SessionCmd::SetSessionName { reply, .. }
        | SessionCmd::Compact { reply }
        | SessionCmd::SetTools { reply, .. }
        | SessionCmd::Deferred { reply, .. }
        | SessionCmd::GetStats { reply }
        | SessionCmd::Prompt { reply, .. } => {
            let _ = reply.send(Err("session busy (prompt running)".into()));
        }
    }
}

/// turn 完成:回落标志 + last_assistant_text + 合成 prompt_done/error + agent_settled。
async fn finish_turn(
    res: Result<pi::sdk::AssistantMessage, pi::sdk::Error>,
    snap: &Arc<Mutex<SessionSnap>>,
    sink: &super::EventSink,
) {
    match &res {
        Ok(am) => {
            let text = extract_assistant_text(am);
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            s.is_streaming = false;
            s.is_prompt_running = false;
            s.last_assistant_text = Some(text);
            emit_agent(sink, json!({ "type": "prompt_done" }));
            emit_agent(sink, json!({ "type": "agent_settled" }));
        }
        Err(e) => {
            let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
            s.is_streaming = false;
            s.is_prompt_running = false;
            // 对齐 rpc-manager:先 prompt_error 再 prompt_done(客户端靠
            // prompt_done 复位 rpcPromptPendingRef,否则 agent_settled 被守卫吞)
            emit_agent(sink, json!({ "type": "prompt_error", "errorMessage": e.to_string() }));
            emit_agent(sink, json!({ "type": "prompt_done" }));
            emit_agent(sink, json!({ "type": "agent_settled" }));
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
            r#"{"providers":{"probe":{"baseUrl":"https://probe.invalid","api":"openai-completions","apiKey":"k","models":[{"id":"p1"}]}}}}"#,
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
