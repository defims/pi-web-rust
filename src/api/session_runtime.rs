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

use pi::sdk::AgentSessionHandle;

use super::commands::ExecCtx;
use super::ApiError;

/// 会话命令(mailbox 载荷)。
#[derive(Debug)]
pub(crate) enum SessionCmd {
    /// 发起一轮 prompt(P3-2 接完整 turn 循环;当前阶段占位)。
    Prompt { message: Value, reply: oneshot::Sender<Result<Value, String>> },
    Steer { message: String, reply: oneshot::Sender<Result<Value, String>> },
    FollowUp { message: String, reply: oneshot::Sender<Result<Value, String>> },
    Abort,
    ClearQueue { reply: oneshot::Sender<Result<Value, String>> },
    SetModel { provider: String, model: String, reply: oneshot::Sender<Result<Value, String>> },
    SetThinking { level: String, reply: oneshot::Sender<Result<Value, String>> },
    SetSessionName { name: String, reply: oneshot::Sender<Result<Value, String>> },
    /// compact / set_tools / fork / reload / navigate 等需要完整 turn 循环或
    /// 重建的命令:P3-2 接线,当前统一占位。
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
        let handle = super::commands::blocking(ctx, move || -> Result<AgentSessionHandle, ApiError> {
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
        let _joined = ctx.rt.handle().spawn(async move {
            // panic 纪律:mid-await panic 经 FutureExt::catch_unwind 截获(P0 报告:
            // panic 会向 await 点传播,监督任务不得被波及)
            let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                session_loop(handle, rx, snap_task),
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

/// 会话任务主循环(mailbox 消费;P3-2 扩展完整 turn select 循环)。
async fn session_loop(
    mut handle: AgentSessionHandle,
    mut rx: mpsc::Receiver<SessionCmd>,
    snap: Arc<Mutex<SessionSnap>>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            SessionCmd::Prompt { message: _, reply } => {
                // P3-2:完整 turn(prompt_with_abort + select + 合成事件)
                let _ = reply.send(Err("prompt: wired in P3-2".into()));
            }
            SessionCmd::Steer { message, reply } => {
                snap.lock().unwrap_or_else(|e| e.into_inner()).queued_steering.push_back(message);
                let _ = reply.send(Ok(json!({})));
            }
            SessionCmd::FollowUp { message, reply } => {
                snap.lock().unwrap_or_else(|e| e.into_inner()).queued_follow_up.push_back(message);
                let _ = reply.send(Ok(json!({})));
            }
            SessionCmd::Abort => {
                // 无在飞 prompt 时 no-op(P3-2 接 AbortHandle 存储)
            }
            SessionCmd::ClearQueue { reply } => {
                let (steering, follow_up) = {
                    let mut s = snap.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        s.queued_steering.drain(..).collect::<Vec<_>>(),
                        s.queued_follow_up.drain(..).collect::<Vec<_>>(),
                    )
                };
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
                    Ok(t) => handle
                        .set_thinking_level(t)
                        .await
                        .map(|_| json!({}))
                        .map_err(|e| format!("{e}")),
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
            SessionCmd::Deferred { what, reply } => {
                let _ = reply.send(Err(format!("{what}: wired in P3-2")));
            }
            SessionCmd::GetStats { reply } => {
                let stats = handle.get_session_stats().await.unwrap_or_else(|_| json!({}));
                let _ = reply.send(Ok(stats));
            }
            SessionCmd::GetLastText { reply } => {
                let fallback =
                    snap.lock().unwrap_or_else(|e| e.into_inner()).last_assistant_text.clone();
                let t = handle.get_last_assistant_text().await.ok().flatten().or(fallback);
                let _ = reply.send(Ok(json!(t)));
            }
        }
    }
}

// ============================================================================
// 测试:生命周期 + RPC 面板(HOME 隔离 + 假 provider 的 models.json)
// ============================================================================

#[cfg(test)]
mod tests {
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct HomeGuard {
        _g: std::sync::MutexGuard<'static, ()>,
        old_home: Option<std::ffi::OsString>,
        old_agent_dir: Option<std::ffi::OsString>,
    }
    impl HomeGuard {
        fn new(tmp: &std::path::Path) -> Self {
            let g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let old_home = std::env::var_os("HOME");
            let old_agent_dir = std::env::var_os("PI_CODING_AGENT_DIR");
            // 引擎 agent 目录用 PI_CODING_AGENT_DIR 显式指路(比 HOME 更强的杠杆,
            // 绕开 dirs::home_dir 的解析差异);auth 同样走此目录(O_NOFOLLOW 需真实路径)
            std::env::set_var("HOME", tmp);
            std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));
            Self { _g: g, old_home, old_agent_dir }
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(old) = self.old_home.take() {
                std::env::set_var("HOME", old);
            }
            match self.old_agent_dir.take() {
                Some(old) => std::env::set_var("PI_CODING_AGENT_DIR", old),
                None => std::env::remove_var("PI_CODING_AGENT_DIR"),
            }
        }
    }

    struct Hooks(std::path::PathBuf);
    impl HostHooks for Hooks {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.join("sessions"))
        }
    }

    fn api_for(tmp: &std::path::Path) -> PiWebApi {
        // models.json:假 provider(无 credential;create 走 registry,prompt 才碰网络)
        let pi = tmp.join(".pi/agent");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(
            pi.join("models.json"),
            r#"{"providers":{"probe":{"baseUrl":"https://probe.invalid","api":"openai-completions","apiKey":"test-key","models":[{"id":"p1","name":"Probe One"}]}}}}"#,
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

    fn call(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    fn post(uri: &str, body: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body.as_bytes().to_vec())
            .unwrap()
    }

    fn get(uri: &str) -> http::Request<Vec<u8>> {
        http::Request::builder().method("GET").uri(uri).body(Vec::new()).unwrap()
    }

    fn body(resp: &http::Response<Vec<u8>>) -> serde_json::Value {
        serde_json::from_slice(resp.body()).expect("json")
    }

    /// 真实路径 HOME(引擎 auth 加载用 O_NOFOLLOW 组件链,tempdir 的
    /// /var/folders 符号链接会 ENOTDIR —— 放 target/ 下避开)。
    fn real_home() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("sr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn session_lifecycle_and_rpc_surface() {
        let tmp = real_home();
        let _guard = HomeGuard::new(&tmp);
        let api = api_for(&tmp);
        let cwd = tmp.to_string_lossy().to_string();

        // new → 200 + sessionId
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#)))
            .expect("create ok");
        assert_eq!(resp.status(), 200, "body(no explicit model): {}", String::from_utf8_lossy(resp.body()));
        let sid = body(&resp)["sessionId"].as_str().expect("sid").to_string();
        assert!(!sid.is_empty());

        // running 列表含新会话
        let resp = call(&api, get("/api/agent/running")).expect("ok");
        assert!(body(&resp)["runningSessionIds"].as_array().unwrap().iter().any(|v| v.as_str() == Some(sid.as_str())));

        // get_state 形状(挂载切片)
        let resp = call(&api, get(&format!("/api/agent/{sid}"))).expect("ok");
        let v = body(&resp);
        assert_eq!(v["running"], serde_json::json!(false));
        // 模型真相由引擎 state 回填(测试进程的 registry 可能解析到内置模型 +
        // ambient 凭据 —— env 解析存在进程级缓存;断言回填机制而非具体值)
        assert!(v["state"]["model"].is_object(), "model backfilled from engine state");
        assert!(v["state"]["model"]["id"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(v["state"]["thinkingLevel"], serde_json::json!("off"));

        // RPC:steer 入镜像 → get_state 可见;clear_queue 回被清内容
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"steer","message":"hi"}"#))
            .expect("ok");
        assert_eq!(resp.status(), 200);
        assert_eq!(body(&resp)["success"], serde_json::json!(true));
        let resp = call(&api, get(&format!("/api/agent/{sid}"))).expect("ok");
        assert_eq!(body(&resp)["state"]["queuedMessages"]["steering"], serde_json::json!(["hi"]));
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"clear_queue"}"#))
            .expect("ok");
        assert_eq!(body(&resp)["data"]["steering"], serde_json::json!(["hi"]));

        // RPC:无效 thinking → 500 {error} 信封
        let resp = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"set_thinking_level","level":"bogus"}"#))
            .expect("ok");
        assert_eq!(resp.status(), 500);
        assert!(body(&resp)["error"].as_str().unwrap().contains("Invalid thinking level"));

        // RPC:未知 type → 400;未知会话 → 404
        let e = call(&api, post(&format!("/api/agent/{sid}"), r#"{"type":"nope"}"#)).unwrap_err();
        assert_eq!(e.status, 400);
        let e = call(&api, post("/api/agent/00000000-0000-0000-0000-000000000000", r#"{"type":"steer","message":"x"}"#))
            .unwrap_err();
        assert_eq!(e.status, 404);
        let e = call(&api, get("/api/agent/00000000-0000-0000-0000-000000000000")).unwrap_err();
        assert_eq!(e.status, 404);

        // 溢出(容量 1):第二个 new 替换第一个 → 旧 id 404
        let resp = call(&api, post("/api/agent/new", &format!(r#"{{"cwd":"{cwd}"}}"#)))
            .expect("create ok");
        let sid2 = body(&resp)["sessionId"].as_str().expect("sid2").to_string();
        assert_ne!(sid, sid2);
        let e = call(&api, get(&format!("/api/agent/{sid}"))).unwrap_err();
        assert_eq!(e.status, 404, "evicted old session");
    }
}
