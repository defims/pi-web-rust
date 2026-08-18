//! 对齐 `lib/model-discovery-auth.ts`。模型发现认证的解析编排。
//!
//! 上游流程:写临时 models.json(注入一个合成发现模型 `__pi_web_model_discovery__`)
//! → 创建 ModelRuntime → 读错误 → 取模型 → getAuth(回退兼容请求配置的 headers)
//! → finally 清理临时目录。
//!
//! 引擎(ModelRuntime)尚未在 pi_agent_rust 提供,Rust 版把引擎操作抽象成
//! [`ModelDiscoveryEngine`] trait 注入;临时目录 IO 忠实实现(RAII 清理,
//! 等价 TS 的 finally)。async 版本经 std::thread + oneshot(运行时无关)。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 对齐 `ModelDiscoveryAuth`。`apiKey` 可为空,headers 只保留字符串值。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDiscoveryAuth {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub headers: HashMap<String, String>,
}

/// 引擎 `getAuth` 的返回值,对齐 `resolved.auth`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedAuth {
    pub api_key: Option<String>,
    pub headers: HashMap<String, String>,
}

/// ModelRuntime 的操作面(trait,供 pi_agent_rust 接线)。
pub trait ModelDiscoveryEngine {
    /// `modelRuntime.getError()`,返回加载错误消息。
    fn load_error(&self) -> Option<String>;
    /// `modelRuntime.getModel(provider, modelId)` 是否存在。
    fn get_model(&self, provider: &str, model_id: &str) -> bool;
    /// `modelRuntime.getAuth(model)`,解析失败返回 None。
    fn get_auth(&self, provider: &str, model_id: &str) -> Option<ResolvedAuth>;
    /// `modelRuntime.getCompatibilityRequestConfig(model).headers`。
    fn compatibility_headers(&self, provider: &str, model_id: &str) -> HashMap<String, String>;
}

/// 对齐 `discoveryModelId`。
pub const DISCOVERY_MODEL_ID: &str = "__pi_web_model_discovery__";

/// 对齐 `mkdtempSync(join(tmpdir(), "pi-web-model-discovery-"))` 的前缀。
pub fn discovery_temp_prefix() -> String {
    "pi-web-model-discovery-".to_string()
}

/// 对齐 TS 的 `stringRecord`:只保留字符串值,其余丢弃。
pub fn string_record(value: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if let Some(s) = val.as_str() {
                out.insert(key.clone(), s.to_string());
            }
        }
    }
    out
}

/// 对齐 models.json 的文档形状:
/// `{ providers: { [providerName]: { ...provider, models: [{ id: discoveryModelId }] } } }`。
/// 注意:models 必须是数组,而非 models.dev 风格的 map。
pub fn build_discovery_models_document(provider_name: &str, provider: &Value) -> Value {
    let mut provider_obj = match provider.as_object() {
        Some(obj) => obj.clone(),
        None => serde_json::Map::new(),
    };
    provider_obj.insert(
        "models".to_string(),
        Value::Array(vec![serde_json::json!({ "id": DISCOVERY_MODEL_ID })]),
    );
    let mut providers = serde_json::Map::new();
    providers.insert(provider_name.to_string(), Value::Object(provider_obj));
    serde_json::json!({ "providers": Value::Object(providers) })
}

/// 临时目录 RAII 守卫(等价 TS 的 `finally { rmSync(tempDir, recursive, force) }`)。
pub(crate) struct TempDirGuard(PathBuf);

impl TempDirGuard {
    pub(crate) fn create(prefix: &str) -> io::Result<Self> {
        let base = std::env::temp_dir();
        // mkdtemp 等价语义:不断尝试不存在的唯一名
        let mut attempts = 0u32;
        loop {
            let dir = base.join(format!("{prefix}{}-{attempts}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(TempDirGuard(dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    attempts += 1;
                    if attempts > 1000 {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "could not allocate unique temp dir",
                        ));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 对齐 `resolveModelDiscoveryAuth` 的同步版(在调用方线程执行 IO + 引擎调用)。
pub fn resolve_model_discovery_auth_blocking(
    provider_name: &str,
    provider: &Value,
    engine: &dyn ModelDiscoveryEngine,
) -> Result<ModelDiscoveryAuth, String> {
    let temp_dir = TempDirGuard::create(&discovery_temp_prefix())
        .map_err(|e| format!("failed to create temp dir: {e}"))?;
    let models_path = temp_dir.path().join("models.json");

    let document = build_discovery_models_document(provider_name, provider);
    fs::write(
        &models_path,
        serde_json::to_string_pretty(&document).unwrap_or_default(),
    )
    .map_err(|e| format!("failed to write models.json: {e}"))?;

    // 对齐 `modelRuntime.getError()` → throw
    if let Some(error) = engine.load_error() {
        return Err(error);
    }
    // 对齐 `getModel(providerName, discoveryModelId)` → throw
    if !engine.get_model(provider_name, DISCOVERY_MODEL_ID) {
        return Err(format!("Unable to load provider \"{provider_name}\""));
    }

    if let Some(resolved) = engine.get_auth(provider_name, DISCOVERY_MODEL_ID) {
        return Ok(ModelDiscoveryAuth {
            api_key: resolved.api_key,
            headers: resolved.headers,
        });
    }

    Ok(ModelDiscoveryAuth {
        api_key: None,
        headers: engine.compatibility_headers(provider_name, DISCOVERY_MODEL_ID),
    })
}

/// 对齐 `resolveModelDiscoveryAuth` 的 async 版(经 std::thread + oneshot)。
pub async fn resolve_model_discovery_auth(
    provider_name: &str,
    provider: &Value,
    engine: std::sync::Arc<dyn ModelDiscoveryEngine + Send + Sync>,
) -> Result<ModelDiscoveryAuth, String> {
    let provider_name = provider_name.to_string();
    let provider = provider.clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result =
            resolve_model_discovery_auth_blocking(&provider_name, &provider, engine.as_ref());
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| "model discovery auth thread panicked".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakeEngine {
        load_error: Option<String>,
        has_model: bool,
        auth: Option<ResolvedAuth>,
        compat_headers: HashMap<String, String>,
    }

    impl ModelDiscoveryEngine for FakeEngine {
        fn load_error(&self) -> Option<String> {
            self.load_error.clone()
        }
        fn get_model(&self, _provider: &str, _model_id: &str) -> bool {
            self.has_model
        }
        fn get_auth(&self, _provider: &str, _model_id: &str) -> Option<ResolvedAuth> {
            self.auth.clone()
        }
        fn compatibility_headers(
            &self,
            _provider: &str,
            _model_id: &str,
        ) -> HashMap<String, String> {
            self.compat_headers.clone()
        }
    }

    #[test]
    fn document_shape() {
        let doc =
            build_discovery_models_document("openai", &json!({"api": "https://api.openai.com"}));
        assert_eq!(doc["providers"]["openai"]["api"], "https://api.openai.com");
        assert_eq!(
            doc["providers"]["openai"]["models"][0]["id"],
            DISCOVERY_MODEL_ID
        );
        // 非对象 provider 也被包裹成对象
        let doc2 = build_discovery_models_document("x", &json!(null));
        assert_eq!(
            doc2["providers"]["x"]["models"][0]["id"],
            DISCOVERY_MODEL_ID
        );
    }

    #[test]
    fn string_record_filters_non_strings() {
        let v = json!({"a": "x", "b": 42, "c": null, "d": ["y"]});
        let rec = string_record(&v);
        assert_eq!(rec.len(), 1);
        assert_eq!(rec.get("a").map(|s| s.as_str()), Some("x"));
        assert_eq!(string_record(&json!([])), HashMap::new());
        assert_eq!(string_record(&json!("str")), HashMap::new());
    }

    #[test]
    fn auth_path_returns_api_key() {
        let engine = FakeEngine {
            load_error: None,
            has_model: true,
            auth: Some(ResolvedAuth {
                api_key: Some("sk-123".to_string()),
                headers: HashMap::from([("x-api-key".to_string(), "sk-123".to_string())]),
            }),
            compat_headers: HashMap::new(),
        };
        let result = resolve_model_discovery_auth_blocking("openai", &json!({}), &engine).unwrap();
        assert_eq!(result.api_key.as_deref(), Some("sk-123"));
        assert_eq!(
            result.headers.get("x-api-key").map(|s| s.as_str()),
            Some("sk-123")
        );
    }

    #[test]
    fn auth_fallback_to_compat_headers() {
        let engine = FakeEngine {
            load_error: None,
            has_model: true,
            auth: None,
            compat_headers: HashMap::from([("authorization".to_string(), "Bearer t".to_string())]),
        };
        let result = resolve_model_discovery_auth_blocking("openai", &json!({}), &engine).unwrap();
        assert_eq!(result.api_key, None);
        assert_eq!(
            result.headers.get("authorization").map(|s| s.as_str()),
            Some("Bearer t")
        );
    }

    #[test]
    fn load_error_propagates() {
        let engine = FakeEngine {
            load_error: Some("bad models.json".to_string()),
            has_model: true,
            auth: None,
            compat_headers: HashMap::new(),
        };
        let err = resolve_model_discovery_auth_blocking("openai", &json!({}), &engine).unwrap_err();
        assert_eq!(err, "bad models.json");
    }

    #[test]
    fn missing_model_errors() {
        let engine = FakeEngine {
            load_error: None,
            has_model: false,
            auth: None,
            compat_headers: HashMap::new(),
        };
        let err = resolve_model_discovery_auth_blocking("openai", &json!({}), &engine).unwrap_err();
        assert!(err.contains("Unable to load provider \"openai\""));
    }

    #[test]
    fn temp_dir_cleaned_up() {
        let engine = FakeEngine {
            load_error: None,
            has_model: true,
            auth: None,
            compat_headers: HashMap::new(),
        };
        let _ = resolve_model_discovery_auth_blocking("openai", &json!({}), &engine).unwrap();
        // 临时目录已随 RAII 清理:扫描 tmp 下不应残留本次会话的目录
        let leftover: Vec<String> = fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with(&discovery_temp_prefix())
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(leftover.is_empty());
    }
}
