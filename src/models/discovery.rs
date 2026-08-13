//! 对齐 `lib/model-discovery.ts`。provider /models 端点响应解析。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

/// 对齐 TS 的 `/\/v\d+(?:beta)?$/i`(末尾版本段,任意版本号 + 可选 beta)。
static VERSION_SUFFIX_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)/v\d+(?:beta)?$").expect("valid version suffix regex")
});

/// 对齐 TS `url.pathname.replace(/\/+$/, "")`。
fn trim_trailing_slashes(s: &str) -> &str {
    let trimmed = s.trim_end_matches('/');
    trimmed
}

/// 对齐 TS `url.pathname.replace(/\/+/g, "/")`(折叠多余斜杠)。
fn collapse_slashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_slash = false;
    for ch in s.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(ch);
            prev_slash = false;
        }
    }
    out
}

/// 对齐 TS `url.searchParams.has(key)`。解析 query 串,判定是否存在某参数名
/// (含无值形式 `?limit`)。fragment 不参与(对齐 URL 解析)。
fn has_query_param(url: &str, key: &str) -> bool {
    let Some(q_start) = url.find('?') else {
        return false;
    };
    let after = &url[q_start + 1..];
    let query = after.split('#').next().unwrap_or(after);
    for entry in query.split('&') {
        let k = entry.split('=').next().unwrap_or("");
        if k == key {
            return true;
        }
    }
    false
}

/// 对齐 `(a ?? id).localeCompare(b ?? id, _, { numeric: true, sensitivity: "base" })`。
///
/// 大小写不敏感 + 数字段按数值比较(natural sort)。重音不敏感(sensitivity:"base")
/// 依赖 ICU,此处未完整复制;模型 id 极少含重音,ASCII 场景与上游一致。
fn natural_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let a_lower: Vec<char> = a.to_lowercase().chars().collect();
    let b_lower: Vec<char> = b.to_lowercase().chars().collect();
    let (mut ai, mut bi) = (0usize, 0usize);
    while ai < a_lower.len() && bi < b_lower.len() {
        let ac = a_lower[ai];
        let bc = b_lower[bi];
        if ac.is_ascii_digit() && bc.is_ascii_digit() {
            let mut av: u64 = 0;
            let mut aj = ai;
            while aj < a_lower.len() && a_lower[aj].is_ascii_digit() {
                av = av
                    .saturating_mul(10)
                    .saturating_add(((a_lower[aj] as u8) - b'0') as u64);
                aj += 1;
            }
            let mut bv: u64 = 0;
            let mut bj = bi;
            while bj < b_lower.len() && b_lower[bj].is_ascii_digit() {
                bv = bv
                    .saturating_mul(10)
                    .saturating_add(((b_lower[bj] as u8) - b'0') as u64);
                bj += 1;
            }
            match av.cmp(&bv) {
                std::cmp::Ordering::Equal => {
                    ai = aj;
                    bi = bj;
                }
                ord => return ord,
            }
        } else {
            match ac.cmp(&bc) {
                std::cmp::Ordering::Equal => {
                    ai += 1;
                    bi += 1;
                }
                ord => return ord,
            }
        }
    }
    a_lower.len().cmp(&b_lower.len())
}

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
        return if id.is_empty() {
            None
        } else {
            Some(DiscoveredModel { id, name: None })
        };
    }
    let obj = value.as_object()?;
    let raw_id = clean_string(obj.get("id").unwrap_or(&Value::Null))
        .or_else(|| clean_string(obj.get("model").unwrap_or(&Value::Null)))
        .or_else(|| clean_string(obj.get("name").unwrap_or(&Value::Null)))?;
    let id = raw_id
        .strip_prefix("models/")
        .unwrap_or(&raw_id)
        .to_string();
    if id.is_empty() {
        return None;
    }
    let name = clean_string(obj.get("display_name").unwrap_or(&Value::Null))
        .or_else(|| clean_string(obj.get("displayName").unwrap_or(&Value::Null)))
        .or_else(|| {
            // 对齐 TS:`(cleanString(value.id) || cleanString(value.model)) ? cleanString(value.name) : undefined`
            // 按「值是否为非空字符串」而非「键是否存在」判定。
            let id_or_model = clean_string(obj.get("id").unwrap_or(&Value::Null))
                .or_else(|| clean_string(obj.get("model").unwrap_or(&Value::Null)));
            if id_or_model.is_some() {
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
    let Some(obj) = value.as_object() else {
        return vec![];
    };
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
        natural_compare(an, bn)
    });
    models
}

/// 把 URL 拆成 `(scheme://authority, path+query+fragment)`。path 以 '/' 开头或为空。
fn split_url_path(url: &str) -> (&str, &str) {
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = scheme_end + 3;
        let authority_end = url[after_scheme..]
            .find(|c| c == '/' || c == '?' || c == '#')
            .map(|i| after_scheme + i)
            .unwrap_or(url.len());
        return (&url[..authority_end], &url[authority_end..]);
    }
    ("", url)
}

/// 对齐 `buildModelsListUrl`。构造 provider /models 端点 URL。
///
/// 忠实复刻:`new URL(baseUrl.trim())` → 操作 pathname + searchParams。
/// 版本段判定用末尾锚定正则 `/\/v\d+(?:beta)?$/i`(非子串 contains);
/// 参数存在性用 `searchParams.has`(非子串 `contains("limit=")`)。
pub fn build_models_list_url(base_url: &str, api: &str) -> String {
    let trimmed = base_url.trim();
    let (origin, tail) = split_url_path(trimmed);
    let (path_raw, query_fragment) = match tail.find(|c| c == '?' || c == '#') {
        Some(i) => (&tail[..i], &tail[i..]),
        None => (tail, ""),
    };
    let mut path = trim_trailing_slashes(path_raw).to_string();
    if !path.to_lowercase().ends_with("/models") {
        if api == "anthropic-messages" && !VERSION_SUFFIX_RE.is_match(&path) {
            path.push_str("/v1");
        }
        if api == "google-generative-ai" && !VERSION_SUFFIX_RE.is_match(&path) {
            path.push_str("/v1beta");
        }
        path.push_str("/models");
        path = collapse_slashes(&path);
    }
    let mut url = format!("{origin}{path}{query_fragment}");
    let sep = if url.contains('?') { "&" } else { "?" };
    if api == "anthropic-messages" && !has_query_param(&url, "limit") {
        url = format!("{url}{sep}limit=1000");
    }
    if api == "google-generative-ai" && !has_query_param(&url, "pageSize") {
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
        assert!(models
            .iter()
            .any(|m| m.id == "claude-3" && m.name.as_deref() == Some("Claude 3")));
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
        assert_eq!(
            build_models_list_url("https://api.test.com/v1", "openai-completions"),
            "https://api.test.com/v1/models"
        );
        assert_eq!(
            build_models_list_url("https://api.test.com", "anthropic-messages"),
            "https://api.test.com/v1/models?limit=1000"
        );
    }
}
