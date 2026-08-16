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

mod commands;
pub mod events;
mod file_index;
mod files;
mod models;
pub mod routes;
mod sessions;

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

    /// 额外工具注册(moho_tools 注入面)。
    fn extra_tools(&self) -> Vec<String> {
        Vec::new()
    }

    /// 会话根目录覆盖(moho-mate 的 AppConfig.chat.session_dir 注入面;
    /// None = crate 默认:PI_CODING_AGENT_DIR/sessions 或 ~/.pi/agent/sessions,
    /// 对齐上游 getAgentDir)。
    fn sessions_root(&self) -> Option<std::path::PathBuf> {
        None
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

    /// 渲染为 http 响应(错误 body 为纯文本,对齐上游 404 形态;
    /// JSON 错误体的逐路由差异由 P2 口径盘点定夺)。
    pub fn to_response(&self) -> http::Response<Vec<u8>> {
        http::Response::builder()
            .status(self.status)
            .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(self.message.clone().into_bytes())
            .expect("static response builder")
    }
}

/// 嵌入配置。
pub struct ApiConfig {
    pub sink: EventSink,
    pub hooks: Arc<dyn HostHooks>,
    pub timeouts: TimeoutConfig,
}

impl ApiConfig {
    pub fn new(sink: EventSink) -> Self {
        Self {
            sink,
            hooks: Arc::new(NoopHooks),
            timeouts: TimeoutConfig::default(),
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
}

impl PiWebApi {
    pub fn new(rt: Arc<Runtime>, cfg: ApiConfig) -> Self {
        Self(Arc::new(Inner { rt, cfg, shutdown: AtomicBool::new(false) }))
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
        let timeout = self.0.cfg.timeouts.for_class(dispatch.timeout_class);
        let ctx = commands::ExecCtx {
            rt: self.0.rt.clone(),
            hooks: self.0.cfg.hooks.clone(),
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

    /// 宿主事件注入(legacy 链路的归宿);sink panic 被 catch_unwind 吞掉
    /// 并忽略(尽力而为语义,见契约三纪律)。
    pub fn emit(&self, event: ApiEvent) {
        let sink = self.0.cfg.sink.clone();
        let _ = std::panic::catch_unwind(AssertUnwindSafe(move || sink(event)));
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
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new(tmp: &std::path::Path) -> Self {
            let guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
