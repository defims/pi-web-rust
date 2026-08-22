//! api — 嵌入契约层(feature = "api")。
//!
//! 计划与决策记录见宿主仓库 `docs/api-embed-plan.md`;P0 spike 结论见
//! `docs/api-embed-p0-report.md`。要点:
//!
//! - 唯一请求入口 [`PiWebApi::handle`],传输方言 = `http` crate 原生类型
//! - **responder exactly-once**:所有路径(未命中 404 / 处理错误 /
//!   catch_unwind 命中 / 超时 504 / 关闭后 503)必达恰一次 —— 本模块以
//!   单一调用点结构化保证
//! - 事件出口 [`EventSink`](crate::api::EventSink) 回调;crate 侧每次调用包
//!   catch_unwind(宿主 panic 不炸会话任务),宿主实现必须非阻塞
//! - 慢命令禁入同步上下文:handle 仅做查表 + 派发,执行全部经注入的
//!   asupersync 运行时(`rt.handle().spawn`)
//! - 已验证平台 = macOS(P0 报告;异步协议在 webkitgtk 未验证)

/// HOME 环境切换的全局互斥锁(所有改 HOME 的测试必须先拿;读 HOME 的
/// 测试同样拿锁避免并行漂移 —— 跨模块共享)。
pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 路由级测试支架(export/worktrees/skills 等命令测试共用):注入
/// sessions_root 的 PiWebApi + 同步 call + JSON POST 构造。持锁 guard 交给
/// 调用方持有(HOME 环境隔离,HOME_LOCK 同模式)。
#[cfg(test)]
pub(crate) mod export_test_support {
    use super::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    pub(crate) struct SessionsRoot(std::path::PathBuf);
    impl HostHooks for SessionsRoot {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
    }

    pub(crate) fn api_with_sessions_root(root: &std::path::Path) -> PiWebApi {
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

    /// HOME 锁 + api(测试里凡会读 HOME/PI_CODING_AGENT_DIR 派生路径的命令都用这个)
    pub(crate) fn api_with_sessions_root_locked(
        root: &std::path::Path,
    ) -> (PiWebApi, std::sync::MutexGuard<'static, ()>) {
        let guard = super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        (api_with_sessions_root(root), guard)
    }

    pub(crate) fn call(
        api: &PiWebApi,
        req: http::Request<Vec<u8>>,
    ) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(
            req,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    pub(crate) fn post_json(uri: &str, body: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body.as_bytes().to_vec())
            .unwrap()
    }
}

mod commands;
pub mod events;
mod export;
mod file_index;
mod files;
mod models;
mod session_runtime;
pub mod routes;
mod sessions;
mod worktrees;

pub use events::ApiEvent;
pub use routes::{TimeoutClass, TimeoutConfig};

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use asupersync::runtime::Runtime;

/// 请求回执:恰好调用一次;`Err` 携带真 HTTP 语义状态码(渲染为对应响应)。
pub type Responder = Box<dyn FnOnce(Result<http::Response<Vec<u8>>, ApiError>) + Send>;

/// 事件出口(宿主实现;非阻塞纪律,crate 侧包 catch_unwind)。
pub type EventSink = Arc<dyn Fn(ApiEvent) + Send + Sync>;

/// 宿主钩子(默认空实现;cwd 解析钩子待 P2 盘点定夺)。
pub trait HostHooks: Send + Sync {
    /// 会话 system prompt 定制(moho-mate 的 skills prompt 注入面)。
    fn system_prompt(&self) -> Option<String> {
        None
    }

    /// 宿主原生工具工厂(嵌入者缝:引擎 SessionOptions::tool_factory 的注入面,
    /// 上游 SDK 文档明示给"下游嵌入者叠加原生工具"用 —— moho_tools 五工具)。
    /// None = 引擎默认工具集。
    fn tool_factory(&self) -> Option<std::sync::Arc<dyn pi::sdk::ToolFactory>> {
        None
    }

    /// 会话根目录覆盖(moho-mate 的 AppConfig.chat.session_dir 注入面;
    /// None = crate 默认:PI_CODING_AGENT_DIR/sessions 或 ~/.pi/agent/sessions,
    /// 对齐上游 getAgentDir)。
    fn sessions_root(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// 网络传输(单一传输钩子:机制在宿主,策略在调用方)。picrab-web 保持
    /// 零 HTTP 客户端;fetch_text 的窄签名(无头/宿主藏超时)已退役 ——
    /// discover 的按家族鉴权头、各路由超时常量(对齐上游)都由 FetchSpec 携带。
    /// 默认不支持 → 依赖它的命令按各自语义返回 502/504。
    fn fetch(&self, spec: &FetchSpec) -> Result<FetchResponse, String> {
        Err(format!("fetch not supported by this host: {}", spec.url))
    }
}

/// 传输请求描述(方法固定 GET;POST 需要时再扩)。
#[derive(Debug, Clone)]
pub struct FetchSpec {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout: std::time::Duration,
}

impl FetchSpec {
    pub fn get_json(url: impl Into<String>, timeout: std::time::Duration) -> Self {
        Self {
            url: url.into(),
            headers: vec![("accept".to_string(), "application/json".to_string())],
            timeout,
        }
    }
}

/// 传输响应(状态码 + 字节体)。
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl FetchResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// 空实现。
pub struct NoopHooks;

impl HostHooks for NoopHooks {}

/// 契约错误:一处定义,一处渲染(`to_response`)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }

    pub fn timeout() -> Self {
        Self::new(504, "request timed out")
    }

    pub fn unavailable() -> Self {
        Self::new(503, "api is shutting down")
    }

    /// 渲染为 http 响应(JSON 错误体 {error},对齐上游 NextResponse.json({error})
    /// 系列 —— 前端消费方读 body.error;agent RPC 信封已是同形。此前 text/plain
    /// 让原生协议路径(Wire B 直连)的错误信息在前端降级为 "HTTP xxx")。
    pub fn to_response(&self) -> http::Response<Vec<u8>> {
        let body = serde_json::json!({ "error": self.message });
        http::Response::builder()
            .status(self.status)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body).unwrap_or_else(|_| self.message.clone().into_bytes()))
            .expect("static response builder")
    }
}

/// 嵌入配置。
pub struct ApiConfig {
    pub sink: EventSink,
    pub hooks: Arc<dyn HostHooks>,
    pub timeouts: TimeoutConfig,
    /// 会话注册表容量(默认 1;溢出 = 关旧建新)。
    pub max_sessions: usize,
    /// idle 清扫阈值(None = 禁用;Some(d) = 无命令且不忙超过 d 的会话
    /// 被优雅关停:引擎 flush 落盘后任务退出,后续 RPC 走磁盘恢复)。
    pub idle_shutdown_after: Option<std::time::Duration>,
}

impl ApiConfig {
    pub fn new(sink: EventSink) -> Self {
        Self {
            sink,
            hooks: Arc::new(NoopHooks),
            timeouts: TimeoutConfig::default(),
            max_sessions: 1,
            idle_shutdown_after: None,
        }
    }
}

/// 嵌入入口:路由 + 命令 + 事件源的统一门面。
///
/// 生命周期:宿主持有 `Arc<Runtime>` 与本实例;退出配方见计划文档
/// (关窗/停接收 → `shutdown()` → drop runtime → exit)。
pub struct PiWebApi(Arc<Inner>);

struct Inner {
    rt: Arc<Runtime>,
    cfg: ApiConfig,
    shutdown: AtomicBool,
    sessions: Arc<session_runtime::SessionRuntime>,
}

impl PiWebApi {
    pub fn new(rt: Arc<Runtime>, cfg: ApiConfig) -> Self {
        let sessions = Arc::new(session_runtime::SessionRuntime::new(cfg.max_sessions));
        // idle 清扫任务(上游 rpc-manager 闲置驱逐):每 60s 扫一次,
        // 超过阈值且不忙的会话优雅关停(flush 后任务自退)。
        // 任务随运行时生命周期(进程退出即止),无泄漏面。
        if let Some(idle_after) = cfg.idle_shutdown_after {
            let sweeper = sessions.clone();
            rt.handle().spawn(async move {
                loop {
                    let now = asupersync::time::wall_now();
                    asupersync::time::sleep(now, std::time::Duration::from_secs(60)).await;
                    sweeper.sweep_idle(idle_after);
                }
            });
        }
        Self(Arc::new(Inner { rt, cfg, shutdown: AtomicBool::new(false), sessions }))
    }

    /// 唯一请求入口。查表 + 派发到注入运行时;responder 恰好调用一次。
    ///
    /// 线律:本函数可从任意线程调用(协议回调在主线程也不阻塞 —— 仅查表)。
    pub fn handle(&self, req: http::Request<Vec<u8>>, responder: Responder) {
        if self.0.shutdown.load(Ordering::SeqCst) {
            responder(Err(ApiError::unavailable()));
            return;
        }
        let dispatch = match routes::resolve(&req) {
            Some(d) => d,
            None => {
                responder(Err(ApiError::not_found(format!(
                    "no route: {} {}",
                    req.method(),
                    req.uri().path()
                ))));
                return;
            }
        };
        // 上传体积第一层(对齐上游 bounded-form-data 的声明长度预检;仅 files
        // 上传路由,其余 POST 不设全局帽 —— agent RPC 带图等合法大体不拦):
        // Wire B 已将 body 缓冲为 Vec<u8>,按声明值提前拒绝,超大体不进命令层。
        if req.method() == "POST" && dispatch.command == "files" {
            let declared = req
                .headers()
                .get(http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok());
            if crate::http::bounded_form_data::check_declared_content_length(
                declared,
                files::MAX_UPLOAD_TOTAL_BYTES as u64,
            )
            .is_err()
            {
                responder(Err(ApiError::new(413, "Request body exceeds the allowed size")));
                return;
            }
        }
        let timeout = self.0.cfg.timeouts.for_class(dispatch.timeout_class);
        let ctx = commands::ExecCtx {
            rt: self.0.rt.clone(),
            hooks: self.0.cfg.hooks.clone(),
            sessions: self.0.sessions.clone(),
            sink: self.0.cfg.sink.clone(),
        };

        // 派发到运行时;panic 传播在 spawn 边界截获(P0 报告:panic 任务的
        // JoinHandle 会向 await 点传播 —— 派发任务自身不允许被命令 panic 波及)。
        let now = asupersync::time::wall_now();
        let fut = async move {
            match asupersync::time::timeout(now, timeout, commands::execute(ctx, dispatch)).await {
                Ok(result) => result,
                Err(_) => Err(ApiError::timeout()),
            }
        };
        let mut responder = Some(responder);
        let spawn_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let responder = responder.take().expect("responder consumed once");
            self.0.rt.handle().spawn(async move {
                let result = match futures::FutureExt::catch_unwind(AssertUnwindSafe(fut)).await {
                    Ok(r) => r,
                    Err(_) => Err(ApiError::internal("handler panicked")),
                };
                responder(result);
            });
        }));
        if spawn_result.is_err() {
            // runtime 已不可用(极端路径):仍需回执,保持 exactly-once
            if let Some(responder) = responder.take() {
                responder(Err(ApiError::unavailable()));
            }
        }
    }

    /// 关闭:P1 置停机位(后续请求 503);P3 扩为
    /// abort 会话 → join → 引擎落盘(见计划退出配方)。
    pub fn shutdown(&self) {
        self.0.shutdown.store(true, Ordering::SeqCst);
    }

    /// 优雅关停(退出配方):向所有会话发强制 Shutdown(idle_after=ZERO
    /// 跳过 idle 二次确认;busy 会话在 turn 内 busy-拒绝),有界等待
    /// flush 落盘回执,再置停机位。此后 drop runtime 不丢未落盘消息
    /// (write-behind 队列已在会话任务内 drain)。
    pub fn shutdown_graceful(&self, grace: std::time::Duration) {
        self.0.rt.block_on(async {
            let handles: Vec<_> = self
                .0
                .sessions
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .cloned()
                .collect();
            let mut waits = Vec::new();
            for h in handles {
                if h.dead.load(Ordering::SeqCst) {
                    continue;
                }
                let (tx, rx) = tokio::sync::oneshot::channel();
                if h.tx
                    .send(session_runtime::SessionCmd::Shutdown {
                        idle_after: std::time::Duration::ZERO,
                        reply: tx,
                    })
                    .await
                    .is_ok()
                {
                    waits.push(rx);
                }
            }
            let now = asupersync::time::wall_now();
            let _ = asupersync::time::timeout(now, grace, futures::future::join_all(waits)).await;
        });
        self.0.shutdown.store(true, Ordering::SeqCst);
    }

    /// 宿主事件注入(legacy 链路的归宿);sink panic 被 catch_unwind 吞掉
    /// 并忽略(尽力而为语义,见契约三纪律)。
    pub fn emit(&self, event: ApiEvent) {
        let sink = self.0.cfg.sink.clone();
        let _ = std::panic::catch_unwind(AssertUnwindSafe(move || sink(event)));
    }

    /// 手动触发 idle 清扫(自动清扫之外的宿主/测试入口)。
    pub fn sweep_idle_sessions(&self, idle_after: std::time::Duration) {
        self.0.sessions.sweep_idle(idle_after);
    }

    /// 注册表会话数(观测/测试用)。
    pub fn session_count(&self) -> usize {
        self.0.sessions.sessions.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

// ============================================================================
// P1 契约测试(docs/api-embed-plan.md 阶段表 P1 验收)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Duration;

    fn rt() -> Arc<Runtime> {
        let reactor = asupersync::runtime::reactor::create_reactor().expect("reactor");
        let runtime = asupersync::runtime::RuntimeBuilder::multi_thread()
            .blocking_threads(1, 2)
            .with_reactor(reactor)
            .build()
            .expect("runtime");
        Arc::new(runtime)
    }

    fn api_with(sink: EventSink, timeouts: TimeoutConfig) -> PiWebApi {
        let mut cfg = ApiConfig::new(sink);
        cfg.timeouts = timeouts;
        PiWebApi::new(rt(), cfg)
    }

    /// 收集 responder 回执(一次)的通道包装。
    fn collector() -> (mpsc::Receiver<Result<http::Response<Vec<u8>>, ApiError>>, Responder) {
        let (tx, rx) = mpsc::channel();
        (
            rx,
            Box::new(move |r: Result<http::Response<Vec<u8>>, ApiError>| {
                let _ = tx.send(r);
            }),
        )
    }

    fn get(path: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("GET")
            .uri(path)
            .body(Vec::new())
            .unwrap()
    }

    #[test]
    fn runtime_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Runtime>();
        assert_send_sync::<PiWebApi>();
    }

    #[test]
    fn exactly_once_404_unknown_route() {
        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.handle(get("/api/nope"), responder);
        let r = rx.recv_timeout(Duration::from_secs(30)).expect("responder called");
        assert_eq!(r.unwrap_err().status, 404);
    }

    #[test]
    fn gated_auth_providers_route() {
        // Wire B 直连回归:Models 面板读 /api/auth/(all-)providers,原桥
        // 常量 stub 迁到 api 层;缺失时 d.providers undefined → .filter 崩渲染。
        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.handle(get("/api/auth/providers"), responder);
        let resp = rx.recv_timeout(Duration::from_secs(30)).expect("responder").expect("ok");
        assert_eq!(resp.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["providers"], serde_json::json!([]));
    }

    #[test]
    fn error_body_is_json_error_shape() {
        // Wire B 直连前置(A):错误体 {error} JSON(对齐上游 NextResponse.json
        // 系列),前端消费方读 body.error。此前 text/plain 让原生协议路径的
        // 错误信息在前端降级为 "HTTP xxx"。
        let err = ApiError::not_found("no route: GET /api/nope");
        let resp = err.to_response();
        assert_eq!(resp.headers().get("content-type").unwrap().to_str().unwrap(), "application/json");
        let v: serde_json::Value = serde_json::from_slice(resp.body()).expect("json body");
        assert_eq!(v["error"], serde_json::json!("no route: GET /api/nope"));
    }

    #[test]
    fn exactly_once_500_handler_panic() {
        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.handle(get("/api/test-panic"), responder);
        let r = rx.recv_timeout(Duration::from_secs(30)).expect("responder called");
        assert_eq!(r.unwrap_err().status, 500);
    }

    #[test]
    fn exactly_once_504_timeout() {
        let (rx, responder) = collector();
        let timeouts = TimeoutConfig {
            default: Duration::from_millis(50),
            long: Duration::from_millis(50),
        };
        let api = api_with(Arc::new(|_| {}), timeouts);
        api.handle(get("/api/test-sleep?ms=5000"), responder);
        let r = rx.recv_timeout(Duration::from_secs(30)).expect("responder called");
        assert_eq!(r.unwrap_err().status, 504);
    }

    #[test]
    fn exactly_once_503_after_shutdown() {
        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.shutdown();
        api.handle(get("/api/home"), responder);
        let r = rx.recv_timeout(Duration::from_secs(30)).expect("responder called");
        assert_eq!(r.unwrap_err().status, 503);
    }

    #[test]
    fn home_ok_and_timeout_tier_long() {
        // 正常路径:home 200 + JSON
        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.handle(get("/api/home"), responder);
        let resp = rx.recv_timeout(Duration::from_secs(30)).expect("responder").expect("ok");
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value =
            serde_json::from_slice(resp.body()).expect("json body");
        assert!(body.get("home").is_some());

        // 长命令档:Default 极短而 Long 足够 —— test_sleep 挂 Default 档
        // 会被 504;此处验证分档机制本身(Long=5s, sleep 100ms 命中 Long 档
        // 需要长档路由,P2 引入 discover 后补;当前以 504 路径验证短档生效)
        let timeouts = TimeoutConfig {
            default: Duration::from_millis(50),
            long: Duration::from_secs(5),
        };
        let api2 = api_with(Arc::new(|_| {}), timeouts);
        let (rx2, responder2) = collector();
        api2.handle(get("/api/test-sleep?ms=2000"), responder2);
        let r = rx2.recv_timeout(Duration::from_secs(30)).expect("responder called");
        assert_eq!(r.unwrap_err().status, 504);
    }

    #[test]
    fn http_dialect_binary_passthrough() {
        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.handle(get("/api/test-bytes"), responder);
        let resp = rx.recv_timeout(Duration::from_secs(30)).expect("responder").expect("ok");
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get(http::header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(resp.body(), &[0u8, 1, 2, 250, 255]);
    }

    #[test]
    fn sink_panic_is_contained_and_next_event_delivered() {
        let count = Arc::new(AtomicUsize::new(0));
        let boom_then_count = count.clone();
        let sink: EventSink = Arc::new(move |ev: ApiEvent| {
            if let ApiEvent::Host { event, .. } = &ev {
                if event == "boom" {
                    panic!("sink boom");
                }
            }
            boom_then_count.fetch_add(1, Ordering::SeqCst);
        });
        let api = api_with(sink, TimeoutConfig::default());
        api.emit(ApiEvent::Host { event: "boom".into(), payload: json!({}) });
        api.emit(ApiEvent::Host { event: "fine".into(), payload: json!({}) });
        assert_eq!(count.load(Ordering::SeqCst), 1, "panic swallowed, next delivered");
    }

    #[test]
    fn path_normalization() {
        assert_eq!(routes::normalize_path("/api/home/"), "/api/home");
        assert_eq!(routes::normalize_path(""), "/");
        assert_eq!(routes::normalize_path("/"), "/");
        assert_eq!(routes::normalize_path("/api/home"), "/api/home");
    }

    /// P2 golden 回放(sessions_list,形状模式):与 moho-mate 旧实现录制的
    /// fixture(host 仓库 tests/fixtures/api-golden/)比对响应形状。
    /// fixture 不存在时跳过(子模块独立 CI 场景)。
    /// 易变字段(modified/messageCount/firstMessage)类型比对;会话集合按 id
    /// 交集比对;fixture 键必须在新响应中齐全(新增键允许,如真实 worktree
    /// 解析引入的 worktreeBranch —— 属口径变化清单项)。
    #[test]
    fn golden_replay_sessions_list_shape() {
        // sessions 扫描读 HOME(经 default_sessions_root),与 HomeGuard 类测试互斥
        let _home = super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/api-golden/sessions_list.json"
        );
        let Ok(fixture_str) = std::fs::read_to_string(fixture_path) else {
            eprintln!("skip: golden fixture not present");
            return;
        };
        let fixture: serde_json::Value = serde_json::from_str(&fixture_str).expect("fixture json");

        let (rx, responder) = collector();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        api.handle(get("/api/sessions"), responder);
        let resp = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("responder called")
            .expect("sessions_list ok");
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).expect("json body");

        // 顶层形状
        assert!(fixture.get("sessions").is_some(), "fixture top shape");
        assert!(body.get("sessions").is_some(), "response top shape");

        let old = session_by_id(&fixture);
        let new = session_by_id(&body);
        let mut compared = 0usize;
        for (id, old_entry) in &old {
            let Some(new_entry) = new.get(id) else { continue };
            compared += 1;
            for (k, old_v) in old_entry.as_object().expect("entry object") {
                let new_v = new_entry
                    .get(k)
                    .unwrap_or_else(|| panic!("fixture key {k} missing in new response"));
                let volatile = matches!(k.as_str(), "modified" | "messageCount" | "firstMessage");
                if volatile {
                    assert_eq!(
                        json_type(old_v),
                        json_type(new_v),
                        "volatile field {k} type drift"
                    );
                } else {
                    assert_eq!(old_v, new_v, "stable field {k} drifted for {id}");
                }
            }
        }
        eprintln!("golden replay: {compared} sessions shape-compared");
        assert!(compared > 0, "no overlapping sessions to compare");
    }

    fn session_by_id(v: &serde_json::Value) -> std::collections::HashMap<String, serde_json::Value> {
        v["sessions"]
            .as_array()
            .expect("sessions array")
            .iter()
            .filter_map(|s| {
                s.get("id")
                    .and_then(|i| i.as_str())
                    .map(|id| (id.to_string(), s.clone()))
            })
            .collect()
    }

    fn json_type(v: &serde_json::Value) -> &'static str {
        match v {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "bool",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        }
    }

    // ── P2 批次:cwd 三件 + git 两件 ────────────────────────────────────

    /// HOME 环境隔离锁(与 moho-mate 测试的 HOME_LOCK 同模式:
    /// 任何改 HOME 的测试必须先拿锁,避免并行污染)。
    

    struct HomeGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new(tmp: &std::path::Path) -> Self {
            let guard = super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let old = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp);
            Self { _guard: guard, old }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(old) = self.old.take() {
                std::env::set_var("HOME", old);
            }
            // 失效 lib 的 roots TTL 缓存:假 HOME 时代合成的 roots 不能
            // 泄漏给后续(HOME_LOCK 已串行化,但缓存生命周期跨锁)。
            crate::fs::file_access::invalidate_allowed_roots_cache();
        }
    }

    fn post_json(path: &str, body: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("POST")
            .uri(path)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body.as_bytes().to_vec())
            .unwrap()
    }

    fn run(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, ApiError> {
        let (rx, responder) = collector();
        api.handle(req, responder);
        rx.recv_timeout(Duration::from_secs(30)).expect("responder called")
    }

    fn body_json(resp: &http::Response<Vec<u8>>) -> serde_json::Value {
        serde_json::from_slice(resp.body()).expect("json body")
    }

    #[test]
    fn cwd_browse_lists_dirs_sorted_excludes_files() {
        let tmp = tempfile_dir();
        std::fs::create_dir_all(tmp.path().join("Beta")).unwrap();
        std::fs::create_dir_all(tmp.path().join("alpha")).unwrap();
        std::fs::write(tmp.path().join("zz-file.txt"), b"x").unwrap();

        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        let resp = run(&api, get(&format!("/api/cwd/browse?path={}", tmp.path().display())))
            .expect("ok");
        assert_eq!(resp.status(), 200);
        let v = body_json(&resp);
        assert_eq!(v["path"], json!(tmp.path().canonicalize().unwrap().to_string_lossy().to_string()));
        assert!(v["parentPath"].is_string());
        let names: Vec<&str> =
            v["directories"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
        // 大小写不敏感排序;文件排除
        assert_eq!(names, vec!["alpha", "Beta"]);
    }

    #[test]
    fn cwd_browse_404_missing_400_file() {
        let tmp = tempfile_dir();
        let f = tmp.path().join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        let e = run(&api, get("/api/cwd/browse?path=/definitely/not/here")).unwrap_err();
        assert_eq!(e.status, 404);
        let e = run(&api, get(&format!("/api/cwd/browse?path={}", f.display()))).unwrap_err();
        assert_eq!(e.status, 400);
    }

    #[test]
    fn cwd_validate_post_body_merge() {
        let tmp = tempfile_dir();
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        // POST body 路径(验证 body 并入 args)
        let resp = run(
            &api,
            post_json("/api/cwd/validate", &format!(r#"{{"cwd":"{}"}}"#, tmp.path().display())),
        )
        .expect("ok");
        assert_eq!(resp.status(), 200);
        let v = body_json(&resp);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["cwd"], json!(tmp.path().canonicalize().unwrap().to_string_lossy().to_string()));
        // 空 → 400;缺失 → 404
        let e = run(&api, post_json("/api/cwd/validate", r#"{"cwd":"  "}"#)).unwrap_err();
        assert_eq!(e.status, 400);
        let e = run(&api, post_json("/api/cwd/validate", r#"{"cwd":"/nope/xx"}"#)).unwrap_err();
        assert_eq!(e.status, 404);
    }

    #[test]
    fn default_cwd_creates_dated_dir_in_home() {
        let tmp = tempfile_dir();
        let _guard = HomeGuard::new(tmp.path());
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        let resp = run(&api, post_json("/api/default-cwd", "{}")).expect("ok");
        assert_eq!(resp.status(), 200);
        let v = body_json(&resp);
        let cwd = v["cwd"].as_str().expect("cwd");
        assert!(cwd.contains("pi-cwd-"), "dated dir: {cwd}");
        assert!(std::path::Path::new(cwd).is_dir(), "dir created");
    }

    #[test]
    fn git_status_on_this_repo_and_root_gate() {
        // 自播种 roots:不依赖 ~/.pi 真实会话扫描(同进程的 lib 测试可能并发
        // 动 HOME/roots 全局态,本测试的锁管不到它们 —— 播种后与机器状态解耦)
        let repo = env!("CARGO_MANIFEST_DIR");
        crate::fs::allowed_roots::allow_file_root(repo);
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        let resp = run(&api, get(&format!("/api/git/status?cwd={repo}"))).expect("ok");
        assert_eq!(resp.status(), 200);
        let v = body_json(&resp);
        assert_eq!(v["isGitRepository"], json!(true));
        assert!(v["files"].is_array());
        assert!(v["additions"].is_number() && v["deletions"].is_number());
        // 门禁:roots 之外 → 403;cwd 缺失 → 400
        let e = run(&api, get("/api/git/status?cwd=/etc")).unwrap_err();
        assert_eq!(e.status, 403);
        let e = run(&api, get("/api/git/status")).unwrap_err();
        assert_eq!(e.status, 400);
    }

    #[test]
    fn git_diff_lib_real_implementation() {
        let repo = env!("CARGO_MANIFEST_DIR");
        crate::fs::allowed_roots::allow_file_root(repo);
        let api = api_with(Arc::new(|_| {}), TimeoutConfig::default());
        let resp = run(
            &api,
            get(&format!("/api/git/diff?cwd={repo}&path=Cargo.toml")),
        )
        .expect("ok");
        assert_eq!(resp.status(), 200);
        let v = body_json(&resp);
        // lib 真实现(旧 moho 返回 {"supported":false} 未实现 —— 口径变化清单项)
        assert!(v.get("supported").is_some());
        // path 缺失 → 400
        let e = run(&api, get(&format!("/api/git/diff?cwd={repo}"))).unwrap_err();
        assert_eq!(e.status, 400);
    }

    fn tempfile_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }
}
