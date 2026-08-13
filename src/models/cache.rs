//! 对齐 `lib/models-cache.ts`。模型数据 TTL 缓存 + 在途合并 + 代际失效。
//!
//! 语义对齐 TS 的 `globalThis.__piModelsCacheState`:
//! - TTL 60s,到期条目在写入新条目时惰性清理
//! - 同一 cwd 并发加载共享同一个在途 future(经 `futures::future::Shared`)
//! - `invalidate_models_cache` 增加代际并清空;在途加载完成后若代际已变则不再写缓存
//! - 上限 32 条,超出时按插入序逐出最旧条目
//!
//! 引擎加载回调以 async 闭包注入(运行时无关,测试用 tokio)。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use futures::FutureExt;

/// 对齐 `MODELS_CACHE_TTL_MS`。
const MODELS_CACHE_TTL: Duration = Duration::from_millis(60_000);
/// 对齐 `MAX_MODELS_CACHE_ENTRIES`。
const MAX_MODELS_CACHE_ENTRIES: usize = 32;

/// 对齐 `ModelsData`。`defaultModel` 为 `null` 时显式序列化为 null(非省略)。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsData {
    pub models: HashMap<String, String>,
    #[serde(rename = "modelList")]
    pub model_list: Vec<ModelListEntry>,
    pub default_model: Option<ModelRef>,
    pub thinking_levels: HashMap<String, Vec<String>>,
    pub thinking_level_maps: HashMap<String, HashMap<String, Option<String>>>,
    /// `provider/modelId` → `enabledModels` `:level` 后缀固定的 thinking level。
    pub thinking_level_pins: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_error: Option<String>,
    /// 解析 `enabledModels` 作用域的警告(如 pattern 未匹配任何模型)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_scope_warnings: Option<Vec<String>>,
}

/// `modelList` 条目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListEntry {
    pub id: String,
    pub name: String,
    pub provider: String,
}

/// `{ provider, modelId }` 引用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

struct CacheEntry {
    data: ModelsData,
    expires_at: Instant,
}

/// 对齐 `ModelsCacheState`。插入序由 `entries` 的 Vec 顺序承载(≤32 条,线性扫描足够)。
#[derive(Default)]
pub struct ModelsCacheState {
    inner: Mutex<CacheStateInner>,
}

#[derive(Default)]
struct CacheStateInner {
    /// 插入序保持的条目列表(逐出按最先插入)。
    entries: Vec<(String, CacheEntry)>,
    /// 在途加载:(load_id, Shared future),load_id 用于身份守卫。
    in_flight: HashMap<String, (u64, futures::future::Shared<BoxFuture>)>,
    generation: u64,
}

/// Shared future 要求内部 future `Send + Sync`(futures 0.3 的 `Shared<F>` 约束)。
type BoxFuture = std::pin::Pin<Box<dyn Future<Output = ModelsData> + Send + Sync>>;

impl ModelsCacheState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 对齐 `invalidateModelsCache`。
    pub fn invalidate(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        inner.entries.clear();
        inner.in_flight.clear();
    }
}

/// 对齐 `withModelRuntimeError`。
pub fn with_model_runtime_error(data: ModelsData, model_error: Option<String>) -> ModelsData {
    match model_error {
        Some(error) => ModelsData {
            model_error: Some(error),
            ..data
        },
        None => data,
    }
}

/// 对齐 `SAFE_MODEL_LOAD_FAILURE_MESSAGE`。
///
/// 刻意不插值捕获到的错误:SDK 错误可能含路径与 provider 细节。
pub const SAFE_MODEL_LOAD_FAILURE_MESSAGE: &str =
    "Model list is temporarily unavailable. Check your configuration and try again.";

/// 对齐 `withSafeModelLoadFailure(data)`。用脱敏的固定文案替换 modelError。
pub fn with_safe_model_load_failure(data: ModelsData) -> ModelsData {
    ModelsData {
        model_error: Some(SAFE_MODEL_LOAD_FAILURE_MESSAGE.to_string()),
        ..data
    }
}

/// 对齐 `loadModelsWithCache`(用当前时间)。
pub async fn load_models_with_cache<F, Fut>(
    state: &ModelsCacheState,
    cwd: &str,
    loader: F,
) -> ModelsData
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ModelsData> + Send + Sync + 'static,
{
    load_models_with_cache_at(state, cwd, Instant::now(), loader).await
}

/// `load_models_with_cache` 的可注入时钟版(测试用,语义与 TS 的 `Date.now()` 对齐)。
pub async fn load_models_with_cache_at<F, Fut>(
    state: &ModelsCacheState,
    cwd: &str,
    now: Instant,
    loader: F,
) -> ModelsData
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ModelsData> + Send + Sync + 'static,
{
    // 锁只在这段块内持有:命中缓存直接返回,命中在途则取出 Shared 后释放锁再 await。
    let existing = {
        let mut inner = state.inner.lock().unwrap();
        if let Some((_, entry)) = inner.entries.iter().find(|(key, _)| key == cwd) {
            if entry.expires_at > now {
                return entry.data.clone();
            }
            inner.entries.retain(|(key, _)| key != cwd);
        }
        inner.in_flight.get(cwd).map(|(_, shared)| shared.clone())
    };
    if let Some(shared) = existing {
        return shared.await;
    }

    let generation = state.inner.lock().unwrap().generation;
    let load_id = new_load_id();
    let boxed: BoxFuture = Box::pin(loader());
    let shared = boxed.shared();
    let waiting = shared.clone();
    {
        let mut inner = state.inner.lock().unwrap();
        inner.in_flight.insert(cwd.to_string(), (load_id, shared));
    }

    let data = waiting.await;

    {
        let mut inner = state.inner.lock().unwrap();
        // 身份守卫(对齐 TS 的 `inFlight.get(cwd) === loadPromise`):
        // 仅当自己是当前在途条目时才移除,避免并发重入互相删除。
        if let Some((id, _)) = inner.in_flight.get(cwd) {
            if *id == load_id {
                inner.in_flight.remove(cwd);
            }
        }
        if inner.generation == generation {
            // 清理到期条目(与 TS 一致:写入前先清)
            let now = Instant::now();
            inner.entries.retain(|(_, entry)| entry.expires_at > now);
            while inner.entries.len() >= MAX_MODELS_CACHE_ENTRIES {
                if inner.entries.is_empty() {
                    break;
                }
                inner.entries.remove(0);
            }
            inner.entries.push((
                cwd.to_string(),
                CacheEntry {
                    data: data.clone(),
                    expires_at: now + MODELS_CACHE_TTL,
                },
            ));
        }
    }

    data
}

static LOAD_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn new_load_id() -> u64 {
    LOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sample_data(model_id: &str) -> ModelsData {
        ModelsData {
            model_list: vec![ModelListEntry {
                id: model_id.to_string(),
                name: model_id.to_string(),
                provider: "p".to_string(),
            }],
            ..ModelsData::default()
        }
    }

    #[tokio::test]
    async fn fresh_load_stores_and_hits() {
        let state = ModelsCacheState::new();
        let now = Instant::now();
        let first =
            load_models_with_cache_at(&state, "/a", now, || async { sample_data("m1") }).await;
        assert_eq!(first.model_list[0].id, "m1");

        // 缓存命中,loader 不再执行
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        let hit =
            load_models_with_cache_at(&state, "/a", now + Duration::from_secs(30), move || {
                calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { sample_data("never") }
            })
            .await;
        assert_eq!(hit.model_list[0].id, "m1");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn expired_entry_reloads() {
        let state = ModelsCacheState::new();
        let now = Instant::now();
        let _ = load_models_with_cache_at(&state, "/a", now, || async { sample_data("old") }).await;
        let refreshed =
            load_models_with_cache_at(&state, "/a", now + Duration::from_secs(61), || async {
                sample_data("new")
            })
            .await;
        assert_eq!(refreshed.model_list[0].id, "new");
    }

    #[tokio::test]
    async fn in_flight_coalesces_concurrent_loaders() {
        let state = Arc::new(ModelsCacheState::new());
        let now = Instant::now();

        let state1 = state.clone();
        let handle1 = tokio::spawn(async move {
            load_models_with_cache_at(&state1, "/a", now, || async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                sample_data("slow")
            })
            .await
        });
        // 稍后发起第二个加载,应复用同一个在途 future
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let state2 = state.clone();
        let handle2 = tokio::spawn(async move {
            load_models_with_cache_at(&state2, "/a", now, || async {
                sample_data("should-not-run")
            })
            .await
        });

        let (a, b) = tokio::join!(handle1, handle2);
        assert_eq!(a.unwrap().model_list[0].id, "slow");
        assert_eq!(b.unwrap().model_list[0].id, "slow");
    }

    #[tokio::test]
    async fn invalidate_clears_cache_and_in_flight() {
        let state = ModelsCacheState::new();
        let now = Instant::now();
        let _ = load_models_with_cache_at(&state, "/a", now, || async { sample_data("m1") }).await;
        state.invalidate();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        let reloaded =
            load_models_with_cache_at(&state, "/a", now + Duration::from_secs(1), move || {
                calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { sample_data("m2") }
            })
            .await;
        assert_eq!(reloaded.model_list[0].id, "m2");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidation_during_flight_skips_cache_write() {
        let state = Arc::new(ModelsCacheState::new());
        let now = Instant::now();
        let state1 = state.clone();
        let handle = tokio::spawn(async move {
            load_models_with_cache_at(&state1, "/a", now, || async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                sample_data("in-flight")
            })
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        state.invalidate();
        let data = handle.await.unwrap();
        assert_eq!(data.model_list[0].id, "in-flight");

        // 代际已变 → 不写缓存:下次加载应重新执行 loader
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        let reloaded =
            load_models_with_cache_at(&state, "/a", now + Duration::from_secs(1), move || {
                calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { sample_data("fresh") }
            })
            .await;
        assert_eq!(reloaded.model_list[0].id, "fresh");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn eviction_keeps_most_recent_insertions() {
        let state = ModelsCacheState::new();
        let now = Instant::now();
        // 塞满上限 + 1
        for i in 0..=MAX_MODELS_CACHE_ENTRIES {
            let cwd = format!("/c{i}");
            let id = format!("m{i}");
            let _ =
                load_models_with_cache_at(
                    &state,
                    &cwd,
                    now,
                    move || async move { sample_data(&id) },
                )
                .await;
        }
        // 最旧(第 0 条)被逐出
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        let evicted =
            load_models_with_cache_at(&state, "/c0", now + Duration::from_secs(1), move || {
                calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { sample_data("reload-0") }
            })
            .await;
        assert_eq!(evicted.model_list[0].id, "reload-0");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // 最新的仍命中
        let hit = load_models_with_cache_at(
            &state,
            &format!("/c{}", MAX_MODELS_CACHE_ENTRIES),
            now + Duration::from_secs(1),
            || async { sample_data("no") },
        )
        .await;
        assert_eq!(
            hit.model_list[0].id,
            format!("m{}", MAX_MODELS_CACHE_ENTRIES)
        );
    }

    #[test]
    fn with_error_appends_or_preserves() {
        let data = sample_data("m1");
        let with_err = with_model_runtime_error(data.clone(), Some("boom".to_string()));
        assert_eq!(with_err.model_error.as_deref(), Some("boom"));
        let preserved = with_model_runtime_error(data.clone(), None);
        assert_eq!(preserved.model_error, None);
        // 原始数据不被修改
        assert_eq!(data.model_error, None);
    }

    #[test]
    fn serialize_shapes() {
        let data = ModelsData {
            models: HashMap::from([("p/m1".to_string(), "M1".to_string())]),
            model_list: vec![ModelListEntry {
                id: "m1".to_string(),
                name: "M1".to_string(),
                provider: "p".to_string(),
            }],
            default_model: None,
            thinking_levels: HashMap::from([("p".to_string(), vec!["high".to_string()])]),
            thinking_level_maps: HashMap::from([(
                "p".to_string(),
                HashMap::from([("m1".to_string(), Some("high".to_string()))]),
            )]),
            thinking_level_pins: HashMap::from([("p/m1".to_string(), "high".to_string())]),
            model_error: None,
            model_scope_warnings: Some(vec!["w".to_string()]),
        };
        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["models"]["p/m1"], "M1");
        assert_eq!(json["modelList"][0]["id"], "m1");
        // defaultModel null 显式输出
        assert!(json["defaultModel"].is_null());
        assert_eq!(json["thinkingLevels"]["p"][0], "high");
        assert_eq!(json["thinkingLevelMaps"]["p"]["m1"], "high");
        assert_eq!(json["thinkingLevelPins"]["p/m1"], "high");
        // 未设置的 error 省略
        assert!(json.get("modelError").is_none());
        assert_eq!(json["modelScopeWarnings"][0], "w");
    }
}
