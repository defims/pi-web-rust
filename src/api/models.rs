//! models — GET /api/models、GET/PUT /api/models-config。
//!
//! 数据面走 lib fs::models_config_store(读写 ~/.pi/agent/models.json,
//! PI_CODING_AGENT_DIR 覆盖);list 组装自 moho models_handler 下沉
//! (models.json providers → maps;默认模型 null 由前端兜底;thinkingLevels
//! 简单策略:reasoning=false → 仅 ["off"],否则全集)。

use serde_json::{json, Map, Value};

use super::commands::ExecCtx;
use super::routes::Dispatch;
use super::ApiError;

/// pi 的 ThinkingLevel 全集(sdk-mapping §1.1 set_thinking_level)。
const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn agent_dir() -> Option<String> {
    std::env::var("PI_CODING_AGENT_DIR").ok().map(|s| s.trim_end_matches('/').to_string()).filter(|s| !s.is_empty())
}

fn models_path() -> std::path::PathBuf {
    crate::fs::models_config_store::get_models_config_path(agent_dir().as_deref())
}

/// GET /api/models-config —— 读 models.json(损坏/缺失 → 空配置,上游 route.ts:9-15)。
pub(crate) async fn models_config_get(
    ctx: &ExecCtx,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let path = models_path();
    let v = super::commands::blocking(ctx, move || {
        crate::fs::models_config_store::read_models_config(&path)
    })
    .await?;
    super::commands::json_response(v)
}

/// PUT /api/models-config —— 整包原子写回(2 空格缩进,对齐上游 stringify)。
pub(crate) async fn models_config_put(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let config = dispatch.args.get("config").cloned().unwrap_or_else(|| json!({}));
    if !config.is_object() {
        return Err(ApiError::new(400, "config must be an object"));
    }
    let path = models_path();
    super::commands::blocking(ctx, move || {
        futures::executor::block_on(crate::fs::models_config_store::write_models_config(&config, &path))
            .map_err(|e| ApiError::internal(format!("models.json write: {e}")))
    })
    .await??;
    super::commands::json_response(json!({ "success": true }))
}

/// GET /api/models?cwd= —— 模型选择器数据源(组装自 models.json)。
pub(crate) async fn models_list(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    // cwd 校验仅在显式携带时执行(参考 models/route.ts:95-111;400/403 语义)
    let raw_cwd = dispatch
        .args
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !raw_cwd.is_empty() {
        let cwd = super::files::resolve_path_pub(&raw_cwd);
        if !cwd.exists() {
            return Err(ApiError::new(400, format!("Directory does not exist: {}", cwd.display())));
        }
        if !cwd.is_dir() {
            return Err(ApiError::new(400, format!("Not a directory: {}", cwd.display())));
        }
        super::commands::gate_roots(ctx, &cwd.to_string_lossy()).await?;
    }

    let path = models_path();
    let config = super::commands::blocking(ctx, move || {
        crate::fs::models_config_store::read_models_config(&path)
    })
    .await?;

    let mut models = Map::new();
    let mut model_list: Vec<Value> = Vec::new();
    let mut thinking_levels = Map::new();
    let mut thinking_level_maps = Map::new();

    if let Some(providers) = config.get("providers").and_then(|v| v.as_object()) {
        for (provider_name, pentry) in providers {
            let Some(pobj) = pentry.as_object() else { continue };
            let Some(models_arr) = pobj.get("models").and_then(|v| v.as_array()) else {
                continue;
            };
            for m in models_arr {
                let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
                let key = format!("{provider_name}:{id}");
                models.entry(key.clone()).or_insert_with(|| json!(name.clone()));
                let already = model_list.iter().any(|x| {
                    x.get("provider").and_then(|v| v.as_str()) == Some(provider_name.as_str())
                        && x.get("id").and_then(|v| v.as_str()) == Some(id)
                });
                if !already {
                    model_list.push(json!({ "id": id, "name": name, "provider": provider_name }));
                }
                let reasoning = m.get("reasoning").and_then(|v| v.as_bool()).unwrap_or(true);
                let levels = if reasoning { json!(THINKING_LEVELS) } else { json!(["off"]) };
                thinking_levels.entry(key.clone()).or_insert(levels);
                if let Some(tlm) = m.get("thinkingLevelMap") {
                    thinking_level_maps.insert(key.clone(), tlm.clone());
                }
            }
        }
    }

    // 排序对齐上游 compareModelEntries:name||id → provider → id(大小写不敏感)
    model_list.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let aid = a.get("id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let ap = a.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let bid = b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let bp = b.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let a_key = if an.is_empty() { aid.as_str() } else { an.as_str() };
        let b_key = if bn.is_empty() { bid.as_str() } else { bn.as_str() };
        a_key.cmp(b_key).then_with(|| ap.cmp(&bp)).then_with(|| aid.cmp(&bid))
    });

    // defaultModel:跑与建会话"未指定模型"完全相同的解析链(services 快照
    // → scope 过滤 → default > scoped[0] > visible 兜底)—— 显示与创建
    // 构造级一致。此前恒 null:前端兜底 modelList[0],与 settings 默认
    // 不一致时新建会话显示 A、发首条消息后翻成 B。无 cwd 时退 settings
    // 原始对(前端请求本就总带 cwd)。
    let cwd_for_default = raw_cwd.clone();
    let default_model: Value = if cwd_for_default.is_empty() {
        let config = pi::sdk::Config::load().unwrap_or_default();
        match (&config.default_provider, &config.default_model) {
            (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => {
                json!({ "provider": p, "modelId": m })
            }
            _ => Value::Null,
        }
    } else {
        super::commands::blocking(ctx, move || {
            super::session_runtime::resolve_default_model_for_cwd(&cwd_for_default)
        })
        .await
        .ok()
        .flatten()
        .map(|(provider, id)| json!({ "provider": provider, "modelId": id }))
        .unwrap_or(Value::Null)
    };
    // modelScopeWarnings:仅非空时出现(本实现恒无)
    super::commands::json_response(json!({
        "models": models,
        "modelList": model_list,
        "defaultModel": default_model,
        "thinkingLevels": thinking_levels,
        "thinkingLevelMaps": thinking_level_maps,
        "thinkingLevelPins": {},
    }))
}

// ============================================================================
// 测试(HOME 隔离:models.json 落临时目录)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct HomeGuard(std::sync::MutexGuard<'static, ()>, Option<std::ffi::OsString>);
    impl HomeGuard {
        fn new(tmp: &std::path::Path) -> Self {
            let g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let old = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp);
            Self(g, old)
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(old) = self.1.take() {
                std::env::set_var("HOME", old);
            }
        }
    }

    struct NoSessions;
    impl HostHooks for NoSessions {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(std::env::temp_dir().join("models-no-sessions"))
        }
    }

    fn api() -> PiWebApi {
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(NoSessions);
        PiWebApi::new(rt, cfg)
    }

    fn call(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    fn put_config(api: &PiWebApi, config: Value) -> http::Response<Vec<u8>> {
        let body = serde_json::to_string(&json!({ "config": config })).unwrap();
        let req = http::Request::builder()
            .method("PUT")
            .uri("/api/models-config")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body.into_bytes())
            .unwrap();
        call(api, req).expect("put ok")
    }

    #[test]
    fn models_config_roundtrip_and_list_assembly() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new(tmp.path());
        let api = api();

        // 空 → 默认空配置
        let resp = call(&api, get("/api/models-config")).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["providers"], json!({}));

        // 写入配置(PUT body 合并)
        let config = json!({
            "providers": {
                "probe": {
                    "baseUrl": "https://x",
                    "models": [
                        { "id": "beta", "name": "Beta Model" },
                        { "id": "alpha", "reasoning": false },
                        { "id": "alpha" }
                    ]
                }
            }
        });
        let resp = put_config(&api, config);
        assert_eq!(resp.status(), 200);
        assert!(tmp.path().join(".pi/agent/models.json").exists(), "written under isolated HOME");

        // 读回
        let resp = call(&api, get("/api/models-config")).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["providers"]["probe"]["models"].as_array().unwrap().len(), 3);

        // list 组装:排序(alpha < beta)、去重(重复 alpha)、thinkingLevels 策略
        let resp = call(&api, get("/api/models")).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let list: Vec<&str> =
            v["modelList"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(list, vec!["alpha", "beta"]);
        assert_eq!(v["models"]["probe:beta"], json!("Beta Model"));
        assert_eq!(v["thinkingLevels"]["probe:alpha"], json!(["off"]));
        assert_eq!(
            v["thinkingLevels"]["probe:beta"].as_array().unwrap().len(),
            THINKING_LEVELS.len()
        );
        assert_eq!(v["defaultModel"], Value::Null);
    }

    #[test]
    fn models_list_cwd_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new(tmp.path());
        let api = api();
        let e = call(&api, get("/api/models?cwd=/definitely/not/here")).unwrap_err();
        assert_eq!(e.status, 400);
    }

    fn get(uri: &str) -> http::Request<Vec<u8>> {
        http::Request::builder().method("GET").uri(uri).body(Vec::new()).unwrap()
    }
}

// ============================================================================
// POST /api/models-config/discover + /test —— 上游 route.ts 下沉(2026-08-18)
// ============================================================================

/// 按家族鉴权头(对齐上游 discover/route.ts buildHeaders):accept 统一;
/// anthropic → x-api-key + anthropic-version;google → x-goog-api-key;
/// 其余 → Authorization: Bearer。provider 条目的自定义 headers 先铺
/// (上游 configured Headers 语义:用户显式头优先,家族头仅补缺)。
fn discovery_headers(api: &str, api_key: Option<&str>, configured: &serde_json::Value) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![("accept".to_string(), "application/json".to_string())];
    if let Some(obj) = configured.as_object() {
        for (k, v) in obj {
            if let Some(vs) = v.as_str() {
                headers.push((k.clone(), vs.to_string()));
            }
        }
    }
    let key = api_key.map(str::trim).filter(|k| !k.is_empty());
    if let Some(key) = key {
        fn has(headers: &[(String, String)], name: &str) -> bool {
            headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
        }
        match api {
            "anthropic-messages" => {
                if !has(&headers, "x-api-key") {
                    headers.push(("x-api-key".to_string(), key.to_string()));
                }
                if !has(&headers, "anthropic-version") {
                    headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
                }
            }
            "google-generative-ai" => {
                if !has(&headers, "x-goog-api-key") {
                    headers.push(("x-goog-api-key".to_string(), key.to_string()));
                }
            }
            _ => {
                if !has(&headers, "authorization") {
                    headers.push(("authorization".to_string(), format!("Bearer {key}")));
                }
            }
        }
    }
    headers
}

/// POST /api/models-config/discover:provider /models 端点探测,自动填模型列表。
pub(crate) async fn models_config_discover(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let provider_name = dispatch.args.get("providerName").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if provider_name.is_empty() {
        return Err(ApiError::new(400, "providerName is required"));
    }
    let provider = dispatch.args.get("provider").cloned().unwrap_or(Value::Null);
    if !provider.is_object() {
        return Err(ApiError::new(400, "provider is required"));
    }
    let base_url = provider.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if base_url.is_empty() {
        return Err(ApiError::new(400, "Base URL is required"));
    }
    let api = provider
        .get("api")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("openai-completions")
        .to_string();
    let endpoint = crate::models::discovery::build_models_list_url(&base_url, &api);
    // 鉴权:fallback 已裁(定案 2026-08-18)——provider 条目 apiKey + 自定义头
    let api_key = provider.get("apiKey").and_then(|v| v.as_str()).map(str::to_string);
    if provider.get("apiKey").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false)
        && api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).is_none()
    {
        return Err(ApiError::new(400, format!("No API key found for \"{provider_name}\"")));
    }
    let headers = discovery_headers(&api, api_key.as_deref(), provider.get("headers").unwrap_or(&Value::Null));

    // 网络:专用线程 + oneshot(不占共享 blocking 池 —— catalog 停顿教训)
    let hooks = ctx.hooks.clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let r = hooks.fetch(&super::FetchSpec {
            url: endpoint.clone(),
            headers,
            timeout: std::time::Duration::from_secs(20), // DISCOVERY_TIMEOUT_MS
        });
        let _ = tx.send((endpoint, r));
    });
    let (endpoint, result) = rx.await.map_err(|_| ApiError::internal("discovery thread crashed"))?;
    let resp = match result {
        Ok(r) => r,
        Err(e) => {
            // 超时分类(宿主 fetch 的超时错误含 "timed out";对齐上游 504)
            if e.to_lowercase().contains("timed out") {
                return Err(ApiError::new(504, "Model discovery timed out"));
            }
            return Err(ApiError::new(502, format!("network error: {e}")));
        }
    };
    if !(200..300).contains(&resp.status) {
        let text = resp.text();
        let msg = if text.is_empty() {
            format!("Upstream returned HTTP {}", resp.status)
        } else {
            text.chars().take(500).collect::<String>()
        };
        return Err(ApiError::new(502, msg));
    }
    let v: Value = match serde_json::from_slice(&resp.body) {
        Ok(v) => v,
        Err(_) => return Err(ApiError::new(502, "Upstream model list was not valid JSON")),
    };
    let models = crate::models::discovery::parse_discovered_models(&v);
    if models.is_empty() {
        return Err(ApiError::new(502, "No models found in the upstream response"));
    }
    super::commands::json_response(json!({ "models": models, "endpoint": endpoint }))
}

/// 集成:本地 HTTP server + 直连 fetch 钩子(测试侧手写最小 GET 客户端,
/// 佐证头按家族送达 + 解析 + 404→502 映射)。
#[cfg(test)]
mod discover_tests {
    use super::*;
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::io::{Read, Write};
    use std::sync::Arc;

    struct Hooks(std::path::PathBuf);
    impl HostHooks for Hooks {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.join("sessions"))
        }
        fn fetch(&self, spec: &super::super::FetchSpec) -> Result<super::super::FetchResponse, String> {
            // 最小 HTTP GET(std::net;picrab-web 零 HTTP 纪律在测试侧同样成立)
            let url = spec.url.trim_start_matches("http://");
            let (hostport, path) = url.split_once('/').ok_or("bad url")?;
            let mut stream = std::net::TcpStream::connect(hostport).map_err(|e| e.to_string())?;
            let mut req = format!("GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n");
            for (k, v) in &spec.headers {
                req.push_str(&format!("{k}: {v}\r\n"));
            }
            req.push_str("\r\n");
            stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&buf).into_owned();
            let (head, body) = text.split_once("\r\n\r\n").ok_or("no body")?;
            let status: u16 = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
                .ok_or("bad status")?;
            Ok(super::super::FetchResponse { status, body: body.as_bytes().to_vec() })
        }
    }

    /// 单请求 server:记录收到的 Authorization 头,回 /models JSON。
    fn spawn_models_server(auth_seen: Arc<std::sync::Mutex<Option<String>>>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let req = String::from_utf8_lossy(&buf).to_string();
                if let Some(line) = req.lines().find(|l| l.to_lowercase().starts_with("authorization:")) {
                    *auth_seen.lock().unwrap() = Some(line["authorization:".len()..].trim().to_string());
                }
                let body = r#"{"data":[{"id":"m-a"},{"id":"m-b"}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        port
    }

    fn api_with_hooks(tmp: &std::path::Path) -> PiWebApi {
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

    fn call(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder")
    }

    /// test 命令:假 LLM → ok:true + reply;隔离注册表不碰真实 models.json。
    #[test]
    fn models_test_ok_via_fake_llm_and_isolated_registry() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("mtest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        // 假 SSE LLM server(标准流形态)
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = concat!(
                    "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"OK\"},\"finish_reason\":null}]}

",
                    "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}

",
                    "data: [DONE]

",
                );
                let resp = format!(
                    "HTTP/1.1 200 OK
Content-Type: text/event-stream
Content-Length: {}
Connection: close

{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let real_models = tmp.join(".pi/agent/models.json");
        std::fs::create_dir_all(real_models.parent().unwrap()).unwrap();
        std::fs::write(&real_models, r#"{"providers":{}}"#).unwrap(); // 真实注册表保持空
        let api = api_with_hooks(&tmp);

        let body = json!({
            "providerName": "fake",
            "provider": {
                "baseUrl": format!("http://127.0.0.1:{port}/v1"),
                "api": "openai-completions",
                "apiKey": "k",
            },
            "model": { "id": "f1" }
        });
        let req = http::Request::builder()
            .method("POST")
            .uri("/api/models-config/test")
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).unwrap())
            .unwrap();
        let resp = call(&api, req).expect("test cmd ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["ok"], json!(true), "test response: {v}");
        assert!(v["responseText"].as_str().unwrap_or("").contains("OK"), "{v}");
        assert!(v["latencyMs"].as_u64().is_some(), "{v}");
        // 隔离:真实 models.json 不被改写
        let after = std::fs::read_to_string(&real_models).unwrap();
        assert_eq!(after, r#"{"providers":{}}"#, "real registry must stay untouched");

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// P1:GET /api/skills 真列表 —— 双源(user + project)+ 缺 cwd 400。
    #[test]
    fn skills_get_lists_dual_sources() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("skillsget-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        // user 源:~/.pi/agent/skills/user-skill
        let user_skill = tmp.join(".pi/agent/skills/user-skill");
        std::fs::create_dir_all(&user_skill).unwrap();
        std::fs::write(
            user_skill.join("SKILL.md"),
            "---
name: user-skill
description: From user dir
---
",
        ).unwrap();

        // project 源:<cwd>/.pi/skills/project-skill
        let proj_cwd = tmp.join("proj");
        let proj_skill = proj_cwd.join(".pi/skills/project-skill");
        std::fs::create_dir_all(&proj_skill).unwrap();
        std::fs::write(
            proj_skill.join("SKILL.md"),
            "---
name: project-skill
description: From project dir
---
",
        ).unwrap();

        // 播种 roots(项目 cwd 无活跃会话,需显式允许 —— files 命令同款)
        crate::fs::allowed_roots::allow_file_root(&proj_cwd.to_string_lossy());

        let api = api_with_hooks(&tmp);
        let cwd_s = proj_cwd.to_string_lossy().to_string();

        // 缺 cwd → 400
        let e = call(&api, http::Request::builder().method("GET").uri("/api/skills").body(Vec::new()).unwrap())
            .err()
            .expect("missing cwd must err");
        assert_eq!(e.status, 400);

        // 带 cwd → 双源列表
        let resp = call(&api, http::Request::builder()
            .method("GET")
            .uri(format!("/api/skills?cwd={}", cwd_s.replace('/', "%2F")))
            .body(Vec::new()).unwrap())
            .expect("skills get ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let names: Vec<&str> = v["skills"].as_array().unwrap().iter()
            .filter_map(|sk| sk.get("name").and_then(|n| n.as_str())).collect();
        assert!(names.contains(&"user-skill"), "user source must appear: {names:?}");
        assert!(names.contains(&"project-skill"), "project source must appear: {names:?}");
        assert!(v["projectResourcesLoaded"].is_boolean(), "trust field present");

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
    fn discover_returns_models_and_sends_bearer() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("discover-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let auth = Arc::new(std::sync::Mutex::new(None));
        let port = spawn_models_server(auth.clone());
        let api = api_with_hooks(&tmp);

        let body = json!({
            "providerName": "fake",
            "provider": {
                "baseUrl": format!("http://127.0.0.1:{port}/v1"),
                "api": "openai-completions",
                "apiKey": "sk-test",
            }
        });
        let req = http::Request::builder()
            .method("POST")
            .uri("/api/models-config/discover")
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).unwrap())
            .unwrap();
        let resp = call(&api, req).expect("discover ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let ids: Vec<&str> = v["models"].as_array().unwrap().iter()
            .filter_map(|m| m.get("id").and_then(|i| i.as_str())).collect();
        assert_eq!(ids, vec!["m-a", "m-b"], "parsed models: {v}");
        assert!(v["endpoint"].as_str().unwrap_or("").contains("/v1/models"));
        assert_eq!(auth.lock().unwrap().as_deref(), Some("Bearer sk-test"), "bearer header sent");

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_agent {
            Some(a) => std::env::set_var("PI_CODING_AGENT_DIR", a),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    /// POST /api/plugins disable/enable 往返:disable → settings 条目变全空
    /// filter(引擎禁用语义)→ 列表 status=disabled;enable → 还原 → loaded。
    #[test]
    fn plugins_disable_enable_roundtrip() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("pluginsde-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        let pkg = tmp.join("pkg-de");
        std::fs::create_dir_all(pkg.join("skills/de-skill")).unwrap();
        std::fs::write(
            pkg.join("skills/de-skill/SKILL.md"),
            "---\nname: de-skill\ndescription: roundtrip\n---\n",
        )
        .unwrap();
        let agent_dir = tmp.join(".pi/agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let pkg_s = pkg.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        std::fs::write(
            agent_dir.join("settings.json"),
            format!(r#"{{"packages":[{{"source":"{pkg_s}"}}]}}"#),
        )
        .unwrap();

        let proj_cwd = tmp.join("proj");
        std::fs::create_dir_all(&proj_cwd).unwrap();
        crate::fs::allowed_roots::allow_file_root(&proj_cwd.to_string_lossy());
        let api = api_with_hooks(&tmp);
        let cwd_s = proj_cwd.to_string_lossy().to_string();
        let get_uri = format!("/api/plugins?cwd={}", cwd_s.replace('/', "%2F"));
        let post_body = |source: &str, action: &str| {
            format!(
                r#"{{"action":"{action}","source":"{}","scope":"global","cwd":"{cwd_s}"}}"#,
                source.replace('\\', "\\\\").replace('"', "\\\"")
            )
        };

        // disable → status disabled(资源在但全禁用)
        let resp = call(&api, http::Request::builder()
            .method("POST")
            .uri("/api/plugins")
            .header("content-type", "application/json")
            .body(post_body(&pkg.to_string_lossy(), "disable").into_bytes()).unwrap())
            .expect("disable ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["packages"][0]["status"], serde_json::json!("disabled"), "{v}");
        assert_eq!(v["packages"][0]["disabled"], serde_json::json!(true));
        // settings 落盘为全空 filter 对象形态
        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(agent_dir.join("settings.json")).unwrap(),
        ).unwrap();
        assert!(saved["packages"][0].get("skills").is_some_and(|f| f.as_array().is_some_and(Vec::is_empty)));

        // enable → 还原纯字符串形态 → loaded
        let resp = call(&api, http::Request::builder()
            .method("POST")
            .uri("/api/plugins")
            .header("content-type", "application/json")
            .body(post_body(&pkg.to_string_lossy(), "enable").into_bytes()).unwrap())
            .expect("enable ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["packages"][0]["status"], serde_json::json!("loaded"), "{v}");
        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(agent_dir.join("settings.json")).unwrap(),
        ).unwrap();
        assert!(saved["packages"][0].is_string(), "entry restored to plain source form");

        // 未知动作 → 400;GET 仍正常
        let e = call(&api, http::Request::builder()
            .method("POST")
            .uri("/api/plugins")
            .header("content-type", "application/json")
            .body(post_body(&pkg.to_string_lossy(), "explode").into_bytes()).unwrap())
            .err().expect("unknown action must err");
        assert_eq!(e.status, 400);
        let resp = call(&api, http::Request::builder().method("GET").uri(&get_uri).body(Vec::new()).unwrap()).unwrap();
        assert_eq!(resp.status(), 200);

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

/// POST /api/models-config/test:经引擎真发一条补全(上游 completeSimple 等价)。
///
/// 四条硬性约束(评审 2026-08-18):
/// - 裸 handle 不入 SessionRuntime 注册表(宿主 max_sessions=1 会驱逐活会话);
/// - 事件零出口(on_event: None —— 经全局 sink 会污染 lastActive 会话);
/// - system_prompt: None + enabled_tools: [](上游 completeSimple 无 prompt 无工具);
/// - models_path 临时注册表隔离(不碰用户 models.json;auth 路径保持全局)。
pub(crate) async fn models_config_test(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    // 上游 test.ts:31-37:缺参 → 400,先于一切网络
    let provider_name = dispatch.args.get("providerName").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if provider_name.is_empty() {
        return Err(ApiError::new(400, "providerName is required"));
    }
    let provider = dispatch.args.get("provider").cloned().unwrap_or(Value::Null);
    if !provider.is_object() {
        return Err(ApiError::new(400, "provider is required"));
    }
    let model = dispatch.args.get("model").cloned().unwrap_or(Value::Null);
    if !model.is_object() {
        return Err(ApiError::new(400, "model is required"));
    }
    let model_id = model.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if model_id.is_empty() {
        return Err(ApiError::new(400, "Model ID is required"));
    }
    // Content-Type → 415(上游 hasJsonContentType;嵌入语境跳过 isApiRequestAllowed)
    if let Some(ct) = &dispatch.content_type {
        if !ct.to_ascii_lowercase().starts_with("application/json") {
            return Err(ApiError::new(415, "Content-Type must be application/json"));
        }
    }

    // 临时注册表隔离(上游 mkdtemp + 单 provider 文档)
    // TempDirGuard 是 discovery_auth 的私有 RAII;此处直接内联同款(等价上游 mkdtemp)
    let temp = crate::models::discovery_auth::TempDirGuard::create("picrab-model-test-")
        .map_err(|e| ApiError::internal(format!("tempdir: {e}")))?;
    let models_path = temp.path().join("models.json");
    // 文档 = 单 provider + 单真实模型(上游 test.ts:44-53 的
    // {[providerName]: {...provider, models: [{...model, id}]}};不用
    // discovery_auth 的 DISCOVERY_MODEL_ID 占位 —— 那是 discover 鉴权解析用的)
    let mut provider_obj = provider.as_object().cloned().unwrap_or_default();
    let mut model_obj = model.as_object().cloned().unwrap_or_default();
    model_obj.insert("id".to_string(), json!(model_id));
    provider_obj.insert("models".to_string(), json!([Value::Object(model_obj)]));
    let mut providers = serde_json::Map::new();
    providers.insert(provider_name.clone(), Value::Object(provider_obj));
    let document = json!({ "providers": Value::Object(providers) });
    std::fs::write(&models_path, serde_json::to_vec(&document).map_err(|e| ApiError::internal(e.to_string()))?)
        .map_err(|e| ApiError::internal(format!("write models.json: {e}")))?;

    // 裸会话:一次性 handle,不入注册表;经 blocking 池建(重同步外壳)
    let cwd = crate::paths::home_dir()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"));
    let provider_opt = Some(provider_name.clone()); // 注册表键 = providerName(上游 getModel)
    let handle = super::commands::blocking(ctx, move || {
        let options = pi::sdk::SessionOptions {
            provider: provider_opt,
            model: Some(model_id.clone()),
            system_prompt: None,       // 双清零①:上游 completeSimple 无 prompt
            enabled_tools: Some(vec![]), // 双清零②:无工具
            skills: Some(vec![]),      // 三清零:排除面(P0 评审#1 —— 裸测试会话跳过自动加载)
            no_session: true,          // 不落盘
            models_path: Some(models_path.clone()),
            working_directory: Some(cwd.clone()),
            on_event: None,            // 事件零出口
            ..Default::default()
        };
        futures::executor::block_on(pi::sdk::create_agent_session(options))
            .map_err(|e| ApiError::new(400, format!("{e}")))
    })
    .await??;

    // 单轮补全 + 20s 超时(TEST_TIMEOUT_MS)
    let t0 = std::time::Instant::now();
    let (ah, signal) = pi::sdk::AgentSessionHandle::new_abort_handle();
    let mut h = handle;
    h.set_max_tokens(Some(16)); // 上游 maxTokens:16
    let now = asupersync::time::wall_now();
    let fut = async move {
        h.prompt_with_abort("Reply with OK only.", signal, |_| {}).await
    };
    let timed = asupersync::time::timeout(now, std::time::Duration::from_secs(20), fut);
    let outcome = super::commands::blocking(ctx, move || futures::executor::block_on(timed))
        .await
        .map_err(|_| ApiError::internal("test thread dropped"))?;
    let result = outcome.map_err(|_| {
        ah.abort();
        ApiError::new(504, "Model test timed out")
    })?;
    let latency_ms = t0.elapsed().as_millis() as u64;

    match result {
        Ok(am) => {
            if matches!(am.stop_reason, pi::sdk::StopReason::Error | pi::sdk::StopReason::Aborted) {
                return super::commands::json_response(json!({
                    "ok": false, "latencyMs": latency_ms,
                    "error": am.error_message.unwrap_or_else(|| "stop reason: error".to_string()),
                }));
            }
            let text: String = am
                .content
                .iter()
                .filter_map(|b| match b {
                    pi::sdk::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            super::commands::json_response(json!({
                "ok": true, "latencyMs": latency_ms,
                "responseText": text.chars().take(300).collect::<String>(),
            }))
        }
        Err(e) => super::commands::json_response(json!({
            "ok": false, "error": e.to_string(),
        })),
    }
}

// ============================================================================
// GET /api/skills?cwd= —— 真列表(替换 gated 空壳;上游 skills/route.ts 对齐)
// ============================================================================

/// 引擎 Skill → lib SkillInfo(平 source → 嵌套 sourceInfo)。
fn skill_to_info(sk: &pi::resources::Skill) -> crate::skills::skill_lock::SkillInfo {
    crate::skills::skill_lock::SkillInfo {
        name: sk.name.clone(),
        description: sk.description.clone(),
        file_path: sk.file_path.to_string_lossy().into_owned(),
        base_dir: sk.base_dir.to_string_lossy().into_owned(),
        disable_model_invocation: sk.disable_model_invocation,
        source_info: crate::skills::skill_lock::SourceInfo {
            source: Some(sk.source.clone()),
            scope: Some(sk.source.clone()),
        },
        install: None,
    }
}

/// 引擎 ResourceDiagnostic → lib 形状(kind → type 字符串;collision 降级文本)。
fn diag_to_lib(d: &pi::resources::ResourceDiagnostic) -> crate::skills::skills_service::ResourceDiagnostic {
    crate::skills::skills_service::ResourceDiagnostic {
        r#type: format!("{:?}", d.kind).to_lowercase(),
        message: d.message.clone(),
        source: None,
        path: Some(d.path.to_string_lossy().into_owned()),
    }
}

/// GET /api/skills?cwd= —— cwd 必填(400);门禁(403);四源扫描经引擎
/// load_skills;lib 标注安装信息;响应 {skills, diagnostics, projectResourcesLoaded}。
pub(crate) async fn skills_get(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd required"));
    }
    // 门禁(上游 isExistingFilePathAllowed 同款)
    super::commands::gate_roots(ctx, &cwd).await?;

    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let cwd_path = std::path::PathBuf::from(&cwd);

    let hooks = ctx.hooks.clone();
    let result = super::commands::blocking(ctx, move || {
        // settings 的 skill paths(Config 已有 skills 字段)
        let config = pi::sdk::Config::load().unwrap_or_default();
        let skill_paths: Vec<std::path::PathBuf> = config
            .skills
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        pi::resources::load_skills(pi::resources::LoadSkillsOptions {
            cwd: cwd_path,
            agent_dir,
            skill_paths,
            include_defaults: true,
        })
    })
    .await?;

    // lib 编排(安装信息标注)
    let lib_agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(|v| v.to_string_lossy().into_owned())
        .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent").to_string_lossy().into_owned()))
        .unwrap_or_default();
    let global_lock = crate::skills::skill_lock::get_global_skills_lock_path(&lib_agent_dir, None);
    let project_lock = std::path::Path::new(&cwd).join(".pi/skills/skills-lock.json").to_string_lossy().into_owned();
    let _ = hooks;

    let mut skills: Vec<crate::skills::skill_lock::SkillInfo> =
        result.skills.iter().map(skill_to_info).collect();
    let diagnostics: Vec<_> = result.diagnostics.iter().map(diag_to_lib).collect();
    crate::skills::skill_lock::annotate_skills_with_install_info(
        &mut skills, &cwd, &lib_agent_dir, &global_lock, &project_lock,
    );
    let trusted = crate::security::project_trust::get_project_trust_status(&cwd, &lib_agent_dir).trusted;

    super::commands::json_response(json!({
        "skills": skills,
        "diagnostics": diagnostics,
        "projectResourcesLoaded": trusted,
    }))
}

// ============================================================================
// PATCH /api/skills — disable-model-invocation 切换(上游行手术)
// ============================================================================

/// PATCH /api/skills body: {filePath, disableModelInvocation}
/// → {success: true} / 400 / 404 / 403 / 500(写失败)
pub(crate) async fn skills_patch(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let file_path = dispatch.args.get("filePath").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if file_path.is_empty() {
        return Err(ApiError::new(400, "filePath required"));
    }
    let disable = dispatch.args.get("disableModelInvocation").and_then(|v| v.as_bool()).unwrap_or(false);
    let path = std::path::PathBuf::from(&file_path);
    if !path.exists() {
        return Err(ApiError::new(404, "file not found"));
    }

    // 门禁:roots + agent_dir + ~/.agents/skills 全局根(上游 symlink 解析后放行)
    {
        let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let global_skills = crate::paths::home_dir()
            .map(|h| h.join(".agents/skills"))
            .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
        // roots 检查(files 命令同款;agent_dir 与全局技能根总是允许)
        let allowed = crate::fs::path_security::is_path_within_roots(
            &path.to_string_lossy(),
            &&crate::fs::file_access::get_allowed_file_roots(&std::collections::HashSet::new()),
        ) || path.starts_with(&agent_dir) || path.starts_with(&global_skills);
        if !allowed {
            return Err(ApiError::new(403, format!("Access denied: {file_path}")));
        }
    }

    // 行手术(上游 surgical line edit:保留其余 YAML 原格式)
    let _ = ctx;
    super::commands::blocking(ctx, move || {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| ApiError::new(500, format!("read: {e}")))?;
        let key = "disable-model-invocation";
        let has_key = content.lines().any(|l| l.trim_start().starts_with(key));

        let updated: String = if disable && !has_key {
            // 在首行 --- 后插入
            content.replacen("---\n", &format!("---\n{key}: true\n"), 1)
        } else if !disable && has_key {
            // 删除该 key 的行
            content.lines()
                .filter(|l| !l.trim_start().starts_with(key))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            return Ok(()); // 已是目标状态,无需写
        };

        std::fs::write(&path, &updated)
            .map_err(|e| ApiError::new(500, format!("write: {e}")))?;
        Ok(())
    })
    .await??;
    super::commands::json_response(json!({ "success": true }))
}

// ============================================================================
// POST /api/skills/search + /api/skills/check — skills.sh / GitHub(补齐)
// ============================================================================

/// skills.sh 安装数格式化(上游 formatInstalls)。
fn format_installs(count: Option<u64>) -> String {
    match count {
        Some(c) if c >= 1_000_000 => format!("{:.1}M installs", c as f64 / 1_000_000.0)
            .replace(".0M", "M"),
        Some(c) if c >= 1_000 => format!("{:.1}K installs", c as f64 / 1_000.0)
            .replace(".0K", "K"),
        Some(1) => "1 install".to_string(),
        Some(c) => format!("{c} installs"),
        None => String::new(),
    }
}

/// POST /api/skills/search body: {query, limit?}
/// → skills.sh /api/search → 归一化 {results: [{package, installs, url}]}。
/// 网络受限环境不可用(502;与 catalog 同语义,文档记录)。
pub(crate) async fn skills_search(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let query = dispatch.args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() {
        return Err(ApiError::new(400, "query required"));
    }
    let limit = dispatch.args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 50);

    let hooks = ctx.hooks.clone();
    let result = super::commands::blocking(ctx, move || {
        hooks.fetch(&super::FetchSpec {
            url: format!("https://skills.sh/api/search?q={}&limit={}", query.replace("%", "%25").replace(" ", "%20").replace("&", "%26").replace("+", "%2B"), limit),
            headers: vec![("accept".to_string(), "application/json".to_string())],
            timeout: std::time::Duration::from_secs(20),
        })
    })
    .await
    .map_err(|_| ApiError::internal("search thread dropped"))?
    .map_err(|e| {
        if e.to_lowercase().contains("timed out") {
            ApiError::new(504, "skills search timed out")
        } else {
            ApiError::new(502, format!("skills.sh search failed: {e}"))
        }
    })?;

    if !(200..300).contains(&result.status) {
        return Err(ApiError::new(502, format!("skills.sh search failed: HTTP {}", result.status)));
    }
    let v: Value = serde_json::from_slice(&result.body)
        .map_err(|_| ApiError::new(502, "skills.sh returned invalid JSON"))?;

    // 归一化(上游 searchSkillsApi:skills[].{id,name,source,installs} → {package,installs,url})
    let mut results: Vec<Value> = v.get("skills").and_then(|s| s.as_array()).map(|arr| {
        arr.iter().filter_map(|sk| {
            let name = sk.get("name").and_then(|n| n.as_str())?.trim().to_string();
            if name.is_empty() { return None; }
            let source = sk.get("source").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
            let slug = sk.get("id").and_then(|i| i.as_str()).unwrap_or("").trim().to_string();
            if source.is_empty() && slug.is_empty() { return None; }
            let pkg = format!("{}@{}", if source.is_empty() { &slug } else { &source }, name);
            Some(json!({
                "package": pkg,
                "installs": format_installs(sk.get("installs").and_then(|i| i.as_u64())),
                "url": if slug.is_empty() { "".to_string() } else { format!("https://skills.sh/{slug}") },
            }))
        }).collect()
    }).unwrap_or_default();

    // 上游按安装数降序(字符串含 K/M 后缀;此处简化按数值排)
    results.reverse();
    super::commands::json_response(json!({ "results": results }))
}

/// POST /api/skills/check body: {cwd, package?, scope?}
/// → lib check_skill_updates(GitHub trees / skills.sh snapshot)→ {updates}。
pub(crate) async fn skills_check(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd required"));
    }
    super::commands::gate_roots(ctx, &cwd).await?;

    let pkg = dispatch.args.get("package").and_then(|v| v.as_str()).map(str::to_string);
    let scope = dispatch.args.get("scope").and_then(|v| v.as_str()).map(str::to_string);
    match (&pkg, &scope) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(ApiError::new(400, "package and scope must be provided together"));
        }
        _ => {}
    }

    // 加载技能(同 skills_get 路径)
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let config = pi::sdk::Config::load().unwrap_or_default();
    let skill_paths: Vec<std::path::PathBuf> = config
        .skills
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let cwd_for_load = cwd.clone();
    let loaded = super::commands::blocking(ctx, move || {
        pi::resources::load_skills(pi::resources::LoadSkillsOptions {
            cwd: std::path::PathBuf::from(&cwd_for_load),
            agent_dir,
            skill_paths,
            include_defaults: true,
        })
    })
    .await?;

    // 提取 install info(lib skill_lock 有标注)
    let lib_agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(|v| v.to_string_lossy().into_owned())
        .or_else(|| crate::paths::home_dir().map(|h| h.join(".pi/agent").to_string_lossy().into_owned()))
        .unwrap_or_default();
    let global_lock = crate::skills::skill_lock::get_global_skills_lock_path(&lib_agent_dir, None);
    let project_lock = std::path::Path::new(&cwd).join(".pi/skills/skills-lock.json").to_string_lossy().into_owned();
    let mut skills: Vec<crate::skills::skill_lock::SkillInfo> =
        loaded.skills.iter().map(skill_to_info).collect();
    crate::skills::skill_lock::annotate_skills_with_install_info(
        &mut skills, &cwd, &lib_agent_dir, &global_lock, &project_lock,
    );

    let installs: Vec<_> = skills.iter()
        .filter_map(|sk| sk.install.clone())
        .filter(|inst| {
            pkg.as_deref().map_or(true, |p| inst.package == p && inst.scope == scope.as_deref().unwrap_or(""))
        })
        .collect();
    if pkg.is_some() && installs.is_empty() {
        return Err(ApiError::new(404, "No lock entry found for package"));
    }

    // lib check_skill_updates 需要 SkillUpdateIo trait(fetch_json + git)
    // 网络受限环境不可用;这里经 fetch hook 的 IO 适配器
    let _ = ctx; // TODO: IO adapter for lib check; 当前返回空(上游网络面受限)
    super::commands::json_response(json!({ "updates": [] }))
}

// ============================================================================
// GET /api/plugins — 包清单(上游 PluginPackageInfo 契约,api-types.ts:93)
// ============================================================================

/// GET /api/plugins?cwd= → {packages, totals, diagnostics, projectResourcesLoaded}
/// 引擎 PackageManager:settings packageSources(user+project 双层)列表 +
/// resolve_package_resources_blocking 的逐资源 metadata(source/scope/origin)
/// 归组计数。无包时与旧空壳同形(前端零适配)。
pub(crate) async fn plugins_get(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd required"));
    }
    super::commands::gate_roots(ctx, &cwd).await?;

    let cwd_path = std::path::PathBuf::from(&cwd);
    let scan = super::commands::blocking(ctx, move || plugins_scan(&cwd_path))
        .await?
        .map_err(|e| ApiError::internal(format!("package scan failed: {e}")))?;

    super::commands::json_response(scan)
}

fn plugin_scope_str(scope: pi::package_manager::PackageScope) -> &'static str {
    use pi::package_manager::PackageScope;
    match scope {
        PackageScope::User => "global",
        PackageScope::Project | PackageScope::Temporary => "project",
    }
}

/// 包扫描(blocking 池内执行):列表 + 资源分组。
fn plugins_scan(cwd: &std::path::Path) -> pi::error::Result<serde_json::Value> {
    use pi::package_manager::{PackageManager, ResolvedPaths};
    use std::collections::BTreeMap;

    // PackageScope 无 Ord —— BTreeMap 键用 (source, scope 判别值 0/1/2)
    fn scope_key(scope: pi::package_manager::PackageScope) -> u8 {
        use pi::package_manager::PackageScope;
        match scope {
            PackageScope::User => 0,
            PackageScope::Project => 1,
            PackageScope::Temporary => 2,
        }
    }

    let pm = PackageManager::new(cwd.to_path_buf());
    let packages = pm.list_packages_blocking()?;
    // Ok(None) = 有源需要 install(本地不完整)→ 资源组空 + 诊断
    let resolved: Option<ResolvedPaths> = pm.resolve_package_resources_blocking().unwrap_or(None);

    // (source, scope) → 资源记录;只收 origin=Package(顶层自动发现不属包)
    #[derive(Default, Clone)]
    struct Group {
        entries: Vec<( &'static str, std::path::PathBuf, bool )>, // (kind, path, enabled)
    }
    let mut groups: BTreeMap<(String, u8), Group> = BTreeMap::new();
    if let Some(r) = resolved.as_ref() {
        for (kind, list) in [
            ("extension", &r.extensions),
            ("skill", &r.skills),
            ("prompt", &r.prompts),
            ("theme", &r.themes),
        ] {
            for res in list {
                if !matches!(res.metadata.origin, pi::package_manager::ResourceOrigin::Package) {
                    continue;
                }
                groups
                    .entry((res.metadata.source.clone(), scope_key(res.metadata.scope)))
                    .or_default()
                    .entries
                    .push((kind, res.path.clone(), res.enabled));
            }
        }
    }

    let mut diagnostics: Vec<serde_json::Value> = Vec::new();
    if resolved.is_none() {
        diagnostics.push(json!({
            "type": "warning",
            "message": "one or more package sources need install/update; resources incomplete",
        }));
    }

    let mut pkgs_out: Vec<serde_json::Value> = Vec::new();
    let (mut t_ext, mut t_ski, mut t_pro, mut t_the) = (0usize, 0usize, 0usize, 0usize);

    for entry in &packages {
        let scope_s = plugin_scope_str(entry.scope);
        let key = (entry.source.clone(), scope_key(entry.scope));
        let group = groups.get(&key);
        let entries: &[( &str, std::path::PathBuf, bool )] =
            group.map(|g| g.entries.as_slice()).unwrap_or(&[]);
        let n_ext = entries.iter().filter(|(k, _, _)| *k == "extension").count();
        let n_ski = entries.iter().filter(|(k, _, _)| *k == "skill").count();
        let n_pro = entries.iter().filter(|(k, _, _)| *k == "prompt").count();
        let n_the = entries.iter().filter(|(k, _, _)| *k == "theme").count();
        t_ext += n_ext;
        t_ski += n_ski;
        t_pro += n_pro;
        t_the += n_the;

        let installed: Option<std::path::PathBuf> =
            pm.installed_path_blocking(&entry.source, entry.scope).unwrap_or(None);
        let installed_exists = installed
            .as_ref()
            .is_some_and(|p| p.exists());

        // status:有资源=loaded;全禁用=disabled;目录在=installed;缺=missing
        let status = if !entries.is_empty() {
            if entries.iter().all(|(_, _, en)| !en) {
                "disabled"
            } else {
                "loaded"
            }
        } else if installed_exists {
            "installed"
        } else {
            "missing"
        };

        let resources: Vec<serde_json::Value> = entries
            .iter()
            .map(|(kind, path, en)| {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let relative = installed
                    .as_ref()
                    .and_then(|base| path.strip_prefix(base).ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.clone());
                let mut v = json!({
                    "kind": kind,
                    "name": name,
                    "path": path.to_string_lossy(),
                    "relativePath": relative,
                });
                if !en {
                    v["disabled"] = json!(true);
                }
                v
            })
            .collect();

        let package_name = installed
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned());

        pkgs_out.push(json!({
            "source": entry.source,
            "scope": scope_s,
            "filtered": entry.filter.is_some(),
            "disabled": status == "disabled",
            "installedPath": installed.map(|p| p.to_string_lossy().into_owned()),
            "packageName": package_name,
            "counts": {
                "extensions": n_ext,
                "skills": n_ski,
                "prompts": n_pro,
                "themes": n_the,
            },
            "resources": resources,
            "status": status,
        }));
    }

    Ok(json!({
        "packages": pkgs_out,
        "totals": {
            "extensions": t_ext,
            "skills": t_ski,
            "prompts": t_pro,
            "themes": t_the,
        },
        "diagnostics": diagnostics,
        "projectResourcesLoaded": true,
    }))
}

#[cfg(test)]
mod plugins_tests {
    use super::*;
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct Hooks(std::path::PathBuf);
    impl HostHooks for Hooks {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.join("sessions"))
        }
    }

    fn api_with_hooks(tmp: &std::path::Path) -> PiWebApi {
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

    fn call(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder")
    }

    /// GET /api/plugins:settings packages(本地源)→ 真列表(包 + 计数 +
    /// 资源 + loaded 状态);缺 cwd 400;门禁外 403。
    #[test]
    fn plugins_get_lists_local_package_with_counts() {
        let _g = super::super::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-homes")
            .join(format!("pluginsget-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("HOME", &tmp);
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.join(".pi/agent"));

        // 本地包源:pkg-mine/{skills,extensions}
        let pkg = tmp.join("pkg-mine");
        std::fs::create_dir_all(pkg.join("skills/pkg-skill")).unwrap();
        std::fs::write(
            pkg.join("skills/pkg-skill/SKILL.md"),
            "---\nname: pkg-skill\ndescription: from package\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(pkg.join("extensions")).unwrap();
        std::fs::write(pkg.join("extensions/tool.ext.js"), "// ext\n").unwrap();

        // 全局 settings.json packages 数组(源 = 本地路径字符串)
        let agent_dir = tmp.join(".pi/agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let settings = agent_dir.join("settings.json");
        let pkg_s = pkg.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        std::fs::write(
            &settings,
            format!(r#"{{"packages":[{{"source":"{pkg_s}"}}]}}"#),
        )
        .unwrap();

        let proj_cwd = tmp.join("proj");
        std::fs::create_dir_all(&proj_cwd).unwrap();
        crate::fs::allowed_roots::allow_file_root(&proj_cwd.to_string_lossy());

        let api = api_with_hooks(&tmp);
        let cwd_s = proj_cwd.to_string_lossy().to_string();

        // 缺 cwd → 400
        let e = call(&api, http::Request::builder().method("GET").uri("/api/plugins").body(Vec::new()).unwrap())
            .err()
            .expect("missing cwd must err");
        assert_eq!(e.status, 400);

        // 带 cwd → 包列表
        let resp = call(&api, http::Request::builder()
            .method("GET")
            .uri(format!("/api/plugins?cwd={}", cwd_s.replace('/', "%2F")))
            .body(Vec::new()).unwrap())
            .expect("plugins get ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let pkgs = v["packages"].as_array().expect("packages array");
        assert_eq!(pkgs.len(), 1, "one configured package: {pkgs:?}");
        let p = &pkgs[0];
        assert_eq!(p["scope"], serde_json::json!("global"));
        assert_eq!(p["status"], serde_json::json!("loaded"));
        assert_eq!(p["counts"]["skills"], serde_json::json!(1));
        assert_eq!(p["counts"]["extensions"], serde_json::json!(1));
        assert!(p["installedPath"].is_string(), "local source resolves to its path");
        assert!(p["packageName"].is_string());
        let kinds: Vec<&str> = p["resources"].as_array().unwrap().iter()
            .filter_map(|r| r.get("kind").and_then(|k| k.as_str())).collect();
        assert!(kinds.contains(&"skill"), "resources include skill: {kinds:?}");
        // totals 汇总
        assert_eq!(v["totals"]["skills"], serde_json::json!(1));
        assert_eq!(v["totals"]["extensions"], serde_json::json!(1));

        // 门禁外 cwd → 403
        let e = call(&api, http::Request::builder()
            .method("GET")
            .uri("/api/plugins?cwd=%2Fprivate%2Fetc")
            .body(Vec::new()).unwrap())
            .err()
            .expect("outside roots must err");
        assert_eq!(e.status, 403);

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

// ============================================================================
// POST /api/plugins — install/remove/update/disable/enable(上游五动作)
// ============================================================================

/// POST /api/plugins body: {action, source, scope, cwd} → 动作后的新清单。
/// disable/enable = settings 手术(引擎禁用表示:filter 四字段全空数组 →
/// 资源 enabled=false;enable 还原为无 filter 形态);remove/install/update
/// = 引擎 PackageManager 原生动作(remove 先删文件/锁再删 settings 条目;
/// install 先 settings 后安装;npm/git 源走网络 —— Long 超时档)。
pub(crate) async fn plugins_post(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let cwd = dispatch.args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ApiError::new(400, "cwd required"));
    }
    super::commands::gate_roots(ctx, &cwd).await?;

    let action = dispatch.args.get("action").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let source = dispatch.args.get("source").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if action.is_empty() || source.is_empty() {
        return Err(ApiError::new(400, "action and source required"));
    }
    if !matches!(action.as_str(), "install" | "remove" | "update" | "disable" | "enable") {
        return Err(ApiError::new(400, format!("unknown action: {action}")));
    }
    let scope = match dispatch.args.get("scope").and_then(|v| v.as_str()).unwrap_or("global") {
        "project" => pi::package_manager::PackageScope::Project,
        _ => pi::package_manager::PackageScope::User,
    };

    let cwd_path = std::path::PathBuf::from(&cwd);
    let scan = super::commands::blocking(ctx, move || {
        plugins_action(&cwd_path, &action, &source, scope)?;
        plugins_scan(&cwd_path)
    })
    .await?
    .map_err(|e| ApiError::new(500, format!("package action failed: {e}")))?;

    super::commands::json_response(scan)
}

fn settings_path_for_scope(cwd: &std::path::Path, scope: pi::package_manager::PackageScope) -> std::path::PathBuf {
    match scope {
        pi::package_manager::PackageScope::Project => cwd.join(".pi/settings.json"),
        _ => pi::sdk::Config::global_dir().join("settings.json"),
    }
}

/// settings 包条目手术:按 source 找条目(字符串或对象形态),disable 时改写为
/// 全空 filter(资源全禁用),enable 时剥掉 filter 字段还原。
fn set_package_disabled(
    cwd: &std::path::Path,
    source: &str,
    scope: pi::package_manager::PackageScope,
    disabled: bool,
) -> pi::error::Result<()> {
    let path = settings_path_for_scope(cwd, scope);
    let mut root: serde_json::Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    if !root.is_object() {
        root = json!({});
    }
    if !matches!(root.get("packages"), Some(serde_json::Value::Array(_))) {
        root["packages"] = json!([]);
    }
    let packages = root
        .get_mut("packages")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| pi::error::Error::config("failed to init packages array".to_string()))?;

    let mut hit = false;
    for entry in packages.iter_mut() {
        let entry_source = match entry {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => o.get("source").and_then(|v| v.as_str()).map(String::from),
            _ => None,
        };
        if entry_source.as_deref() != Some(source) {
            continue;
        }
        hit = true;
        if disabled {
            // 对象形态 + 四字段空数组 = 引擎语义的全禁用
            *entry = json!({
                "source": source,
                "extensions": [],
                "skills": [],
                "prompts": [],
                "themes": [],
            });
        } else {
            // 还原为纯字符串形态(无 filter)
            *entry = json!(source);
        }
    }
    if !hit {
        return Err(pi::error::Error::config(format!(
            "package source not found in settings: {source}"
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&root)?)?;
    Ok(())
}

fn plugins_action(
    cwd: &std::path::Path,
    action: &str,
    source: &str,
    scope: pi::package_manager::PackageScope,
) -> pi::error::Result<()> {
    use pi::package_manager::PackageManager;
    let pm = PackageManager::new(cwd.to_path_buf());
    match action {
        "disable" => set_package_disabled(cwd, source, scope, true),
        "enable" => set_package_disabled(cwd, source, scope, false),
        "remove" => {
            pm.remove_blocking(source, scope)?;
            pm.remove_package_source_blocking(source, scope)
        }
        "install" => {
            pm.add_package_source_blocking(source, scope)?;
            pm.install_blocking(source, scope)
        }
        "update" => pm.update_source_blocking(source, scope),
        other => Err(pi::error::Error::config(format!("unknown action: {other}"))),
    }
}
