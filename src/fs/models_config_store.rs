//! 对齐 `lib/models-config-store.ts`。models.json 读写(带原子写入 + 规范化)。

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// 对齐 `MODEL_COST_KEYS`。
const MODEL_COST_KEYS: &[&str] = &["input", "output", "cacheRead", "cacheWrite"];

/// 对齐 `getModelsConfigPath`。默认 ~/.pi/agent/models.json。
pub fn get_models_config_path(agent_dir: Option<&str>) -> PathBuf {
    let dir = agent_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".pi/agent"));
    dir.join("models.json")
}

/// 对齐 `readModelsConfig`。读 models.json;不存在或解析失败返回 { providers: {} }。
pub fn read_models_config(models_path: &Path) -> Value {
    if !models_path.exists() {
        return serde_json::json!({ "providers": {} });
    }
    match std::fs::read_to_string(models_path) {
        Ok(content) => serde_json::from_str(&content)
            .unwrap_or_else(|_| serde_json::json!({ "providers": {} })),
        Err(_) => serde_json::json!({ "providers": {} }),
    }
}

/// 对齐 `normalizeModelCost`。补全缺失的 cost 字段为 0;含非有限 number 的组返回 None。
fn normalize_model_cost(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    if !MODEL_COST_KEYS.iter().any(|k| obj.contains_key(*k)) {
        return None;
    }
    // 任一已提供的 cost 字段必须是有限 number。
    for k in MODEL_COST_KEYS {
        if let Some(v) = obj.get(*k) {
            let Some(n) = v.as_f64() else {
                return None;
            };
            if !n.is_finite() {
                return None;
            }
        }
    }
    let mut out = obj.clone();
    // 对齐 `MODEL_COST_KEYS.map(k => [k, value[key] ?? 0])`:已提供的原样保留
    // (保持整数/浮点的 JSON 形态),仅补全缺失键为 0。
    for k in MODEL_COST_KEYS {
        if !out.contains_key(*k) {
            out.insert(k.to_string(), serde_json::json!(0));
        }
    }
    Some(Value::Object(out))
}

/// 对齐 `normalizeModelsConfigCosts`。补全部分 cost 组为 0,空组删除 cost。
pub fn normalize_models_config_costs(data: Value) -> Value {
    let mut normalized = data;
    let Some(providers) = normalized
        .get_mut("providers")
        .and_then(Value::as_object_mut)
    else {
        return normalized;
    };
    for provider in providers.values_mut() {
        let Some(models) = provider.get_mut("models").and_then(Value::as_array_mut) else {
            continue;
        };
        for model in models.iter_mut() {
            let Some(mobj) = model.as_object_mut() else {
                continue;
            };
            if !mobj.contains_key("cost") {
                continue;
            }
            let cost = mobj.get("cost").cloned();
            match cost.and_then(|c| normalize_model_cost(&c)) {
                Some(normalized_cost) => {
                    mobj.insert("cost".to_string(), normalized_cost);
                }
                None => {
                    mobj.remove("cost");
                }
            }
        }
    }
    normalized
}

/// 对齐 `sanitizeModelsConfig`。过滤 id 为空白字符串的模型。
pub fn sanitize_models_config(data: Value) -> Value {
    let mut normalized = data;
    let Some(providers) = normalized
        .get_mut("providers")
        .and_then(Value::as_object_mut)
    else {
        return normalized;
    };
    for provider in providers.values_mut() {
        let Some(models) = provider.get_mut("models").and_then(Value::as_array_mut) else {
            continue;
        };
        models.retain(|model| {
            // 对齐 `!isRecord(model) || typeof model.id !== "string" || model.id.trim().length > 0`
            let Some(mobj): Option<&Map<String, Value>> = model.as_object() else {
                return true;
            };
            match mobj.get("id").and_then(Value::as_str) {
                None => true,
                Some(id) => !id.trim().is_empty(),
            }
        });
    }
    normalized
}

/// 对齐 `writeModelsConfig`。先 sanitize → normalize,再原子写入。
///
/// 注:TS 在写后调用全局 `invalidateModelsCache()`;Rust 的缓存是实例化
/// (`ModelsCacheState`),失效由宿主在写入后调用 `state.invalidate()` 负责。
pub async fn write_models_config(data: &Value, models_path: &Path) -> std::io::Result<()> {
    let normalized = normalize_models_config_costs(sanitize_models_config(data.clone()));
    let content = serde_json::to_string_pretty(&normalized).map_err(std::io::Error::other)?;
    // 把 create_dir_all + 原子写入(fs含 fsync)移入线程,避免阻塞 executor。
    let path = models_path.to_path_buf();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // 在线程内直接调用 blocking 版本(此处确实在 thread 里,blocking 是正确的)。
            super::atomic_file::write_private_file_atomic_blocking(&path, &content)
        })();
        let _ = tx.send(result);
    });
    rx.await.map_err(|_| std::io::Error::other("thread panicked"))?
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn write_and_read() {
        let dir = std::env::temp_dir();
        let path = dir.join("pi_web_rust_models_config_test.json");
        let data = json!({"providers": {"test": {"apiKey": "xxx"}}});
        write_models_config(&data, &path).await.unwrap();
        let read = read_models_config(&path);
        assert_eq!(read["providers"]["test"]["apiKey"], "xxx");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_nonexistent() {
        let result = read_models_config(Path::new("/nonexistent/models.json"));
        assert_eq!(result["providers"], json!({}));
    }

    #[test]
    fn sanitize_drops_blank_id_models() {
        let data = json!({
            "providers": { "p": { "models": [
                { "id": "keep-me", "name": "K" },
                { "id": "  ", "name": "blank-id" },
                { "id": "", "name": "empty-id" },
                { "name": "no-id" },
            ] } }
        });
        let sanitized = sanitize_models_config(data);
        let names: Vec<&str> = sanitized["providers"]["p"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap_or(""))
            .collect();
        // 保留 keep-me(非空 id)与 no-id(无 id 字段);删除空白 id。
        assert_eq!(names, vec!["K", "no-id"]);
    }

    #[test]
    fn normalize_fills_missing_cost_fields_and_drops_invalid() {
        let data = json!({
            "providers": { "p": { "models": [
                // 部分组:补 cacheRead/cacheWrite = 0
                { "id": "m1", "cost": { "input": 1, "output": 2 } },
                // 含非有限 number → 删除 cost
                { "id": "m2", "cost": { "input": "oops", "output": 2 } },
                // 完整组保持
                { "id": "m3", "cost": { "input": 3, "output": 4, "cacheRead": 0.1, "cacheWrite": 0.2 } },
            ] } }
        });
        let normalized = normalize_models_config_costs(data);
        let models = normalized["providers"]["p"]["models"].as_array().unwrap();
        assert_eq!(models[0]["cost"]["cacheRead"], json!(0));
        assert_eq!(models[0]["cost"]["cacheWrite"], json!(0));
        assert_eq!(models[0]["cost"]["input"], json!(1));
        assert!(
            models[1].get("cost").is_none(),
            "non-finite cost group dropped"
        );
        assert_eq!(models[2]["cost"]["cacheRead"], json!(0.1));
    }
}
