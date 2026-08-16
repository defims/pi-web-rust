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

    // defaultModel:null —— 默认由 pi settings.json 决定,前端 loadModels 兜底取首个
    // modelScopeWarnings:仅非空时出现(本实现恒无)
    super::commands::json_response(json!({
        "models": models,
        "modelList": model_list,
        "defaultModel": Value::Null,
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

    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard(std::sync::MutexGuard<'static, ()>, Option<std::ffi::OsString>);
    impl HomeGuard {
        fn new(tmp: &std::path::Path) -> Self {
            let g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
