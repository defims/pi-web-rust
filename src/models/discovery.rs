//! 对齐 `lib/model-discovery.ts`。provider /models 端点响应解析。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: Option<String>,
}

fn clean_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn model_from_value(value: &Value) -> Option<DiscoveredModel> {
    if let Some(s) = value.as_str() {
        let id = s.trim().to_string();
        return if id.is_empty() { None } else { Some(DiscoveredModel { id, name: None }) };
    }
    let obj = value.as_object()?;
    let raw_id = clean_string(obj.get("id").unwrap_or(&Value::Null))
        .or_else(|| clean_string(obj.get("model").unwrap_or(&Value::Null)))
        .or_else(|| clean_string(obj.get("name").unwrap_or(&Value::Null)))?;
    let id = raw_id.strip_prefix("models/").unwrap_or(&raw_id).to_string();
    if id.is_empty() { return None; }
    let name = clean_string(obj.get("display_name").unwrap_or(&Value::Null))
        .or_else(|| clean_string(obj.get("displayName").unwrap_or(&Value::Null)))
        .or_else(|| {
            if obj.contains_key("id") || obj.contains_key("model") {
                clean_string(obj.get("name").unwrap_or(&Value::Null))
            } else {
                None
            }
        });
    Some(DiscoveredModel {
        name: name.filter(|n| n != &id),
        id,
    })
}

fn list_from_response(value: &Value) -> Vec<Value> {
    if let Some(arr) = value.as_array() {
        return arr.clone();
    }
    let Some(obj) = value.as_object() else { return vec![] };
    for key in &["data", "models", "results", "items"] {
        if let Some(candidate) = obj.get(*key) {
            if let Some(arr) = candidate.as_array() {
                return arr.clone();
            }
            if let Some(o) = candidate.as_object() {
                return o.values().cloned().collect();
            }
        }
    }
    vec![]
}

/// 对齐 `parseDiscoveredModels`。
pub fn parse_discovered_models(value: &Value) -> Vec<DiscoveredModel> {
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    for item in list_from_response(value) {
        if let Some(model) = model_from_value(&item) {
            if seen.insert(model.id.clone()) {
                models.push(model);
            }
        }
    }
    models.sort_by(|a, b| {
        let an = a.name.as_ref().unwrap_or(&a.id);
        let bn = b.name.as_ref().unwrap_or(&b.id);
        an.to_lowercase().cmp(&bn.to_lowercase())
    });
    models
}

/// 对齐 `buildModelsListUrl`。构造 provider /models 端点 URL。
pub fn build_models_list_url(base_url: &str, api: &str) -> String {
    let mut url = base_url.trim().trim_end_matches('/').to_string();
    if !url.to_lowercase().ends_with("/models") {
        if api == "anthropic-messages" && !url.contains("/v1") {
            url.push_str("/v1");
        }
        if api == "google-generative-ai" && !url.contains("/v1beta") {
            url.push_str("/v1beta");
        }
        url.push_str("/models");
    }
    let sep = if url.contains('?') { "&" } else { "?" };
    if api == "anthropic-messages" && !url.contains("limit=") {
        url = format!("{url}{sep}limit=1000");
    }
    if api == "google-generative-ai" && !url.contains("pageSize=") {
        url = format!("{url}{sep}pageSize=1000");
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_array() {
        let resp = json!(["gpt-4", {"id": "claude-3", "name": "Claude 3"}]);
        let models = parse_discovered_models(&resp);
        assert_eq!(models.len(), 2);
        assert!(models.iter().any(|m| m.id == "gpt-4"));
        assert!(models.iter().any(|m| m.id == "claude-3" && m.name.as_deref() == Some("Claude 3")));
    }

    #[test]
    fn parse_with_data_key() {
        let resp = json!({"data": [{"id": "models/gemini", "display_name": "Gemini"}]});
        let models = parse_discovered_models(&resp);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini");
    }

    #[test]
    fn build_url() {
        assert_eq!(build_models_list_url("https://api.test.com/v1", "openai-completions"), "https://api.test.com/v1/models");
        assert_eq!(build_models_list_url("https://api.test.com", "anthropic-messages"), "https://api.test.com/v1/models?limit=1000");
    }
}
