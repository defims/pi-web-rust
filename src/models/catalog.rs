//! 对齐 `lib/model-catalog.ts`。models.dev 目录的扁平化 + 推荐/搜索(纯计算,无 IO)。
//!
//! - `flatten_models_dev_catalog`:原始 models.dev JSON → 扁平条目列表
//! - `recommend_model_catalog_preset`:按精确 id 匹配推荐元数据与价格
//!   (provider 命中 → base-url 命中 → 条目共识)
//! - `search_model_catalog`:模糊搜索 + 排名(自然数值排序)

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 对齐 `ModelCatalogCost`。models.dev 原始字段为 snake_case(cache_read),
/// 序列化时输出 camelCase。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelCatalogCost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// 对齐 `ModelCatalogEntry`(扁平化后的条目)。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCatalogEntry {
    pub key: String,
    pub provider_id: String,
    pub provider_name: String,
    pub provider_base_url: Option<String>,
    pub id: String,
    pub name: String,
    pub reasoning: Option<bool>,
    pub input: Option<Vec<String>>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub cost: ModelCatalogCost,
}

/// 对齐 `ModelCatalogPreset`。`cost` 仅在价格可靠时填充。
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ModelCatalogPreset {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelCatalogCost>,
}

/// 对齐 `ModelCatalogMatchMethod`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelCatalogMatchMethod {
    Provider,
    BaseUrl,
    Consensus,
    None,
}

/// 价格不可靠时的原因,对齐 TS union 的 `reason`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnreliableReason {
    NoExactMatch,
    NoValidPrice,
    InsufficientSupport,
    Conflict,
}

/// 对齐 `ModelCatalogPriceRecommendation`。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status")]
pub enum ModelCatalogPriceRecommendation {
    #[serde(rename = "reliable")]
    Reliable {
        method: ModelCatalogMatchMethod,
        cost: ModelCatalogCost,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_name: Option<String>,
        support: usize,
        total: usize,
    },
    #[serde(rename = "unreliable")]
    Unreliable {
        reason: UnreliableReason,
        support: usize,
        total: usize,
    },
}

/// 对齐 `ModelCatalogRecommendation`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelCatalogRecommendation {
    #[serde(rename = "exactMatches")]
    pub exact_matches: usize,
    #[serde(rename = "metadataMethod")]
    pub metadata_method: ModelCatalogMatchMethod,
    #[serde(skip_serializing_if = "Option::is_none", rename = "matchedProviderId")]
    pub matched_provider_id: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "matchedProviderName"
    )]
    pub matched_provider_name: Option<String>,
    pub preset: ModelCatalogPreset,
    pub price: ModelCatalogPriceRecommendation,
}

/// 对齐 `CONSENSUS_MIN_SHARE`。
const CONSENSUS_MIN_SHARE: f64 = 0.6;
/// 对齐 `CONSENSUS_MIN_SUPPORT`。winner 达到此数量即视为共识(无论占比)。
const CONSENSUS_MIN_SUPPORT: usize = 5;

/// 对齐 `KNOWN_PROVIDER_HOSTS`。
fn known_provider_hosts(provider_id: &str) -> &'static [&'static str] {
    match normalize_provider(provider_id).as_str() {
        "anthropic" => &["api.anthropic.com"],
        "google" => &["generativelanguage.googleapis.com"],
        "openai" => &["api.openai.com"],
        "openrouter" => &["openrouter.ai"],
        _ => &[],
    }
}

/// 对齐 `SUPPORTED_INPUT_MODALITIES`。
fn supported_input_modalities() -> &'static [&'static str] {
    &["text", "image"]
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// 对齐 `cleanString`。
fn clean_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 对齐 `optionalNonNegativeNumber`。
fn optional_non_negative_number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}

/// 对齐 `optionalPositiveNumber`。
fn optional_positive_number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v > 0.0)
}

/// 对齐 `readCost`。把 models.dev 的 snake_case 字段映射到条目结构。
fn read_cost(value: &Value) -> ModelCatalogCost {
    if !is_record(value) {
        return ModelCatalogCost::default();
    }
    ModelCatalogCost {
        input: optional_non_negative_number(&value["input"]),
        output: optional_non_negative_number(&value["output"]),
        cache_read: optional_non_negative_number(&value["cache_read"]),
        cache_write: optional_non_negative_number(&value["cache_write"]),
    }
}

/// 对齐 `readInputModalities`。只保留 text/image,去重、排序保留出现顺序。
fn read_input_modalities(value: &Value) -> Option<Vec<String>> {
    let obj = value.as_object()?;
    let arr = obj.get("input")?.as_array()?;
    let mut seen = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            let normalized = s.trim().to_lowercase();
            if supported_input_modalities().contains(&normalized.as_str())
                && !seen.contains(&normalized)
            {
                seen.push(normalized);
            }
        }
    }
    if seen.is_empty() {
        None
    } else {
        Some(seen)
    }
}

/// 对齐 `normalizeProvider`:小写 + 去除非字母数字。
fn normalize_provider(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// 对齐 `normalizeModelId`:小写 + 去掉 `models/` 前缀。
fn normalize_model_id(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .strip_prefix("models/")
        .unwrap_or(value.trim().to_lowercase().as_str())
        .to_string()
}

/// 对齐 `hostname`:`new URL(value).hostname`,小写、去尾部点。
fn hostname(value: Option<&str>) -> Option<String> {
    let value = value?;
    let host = url_hostname(value)?;
    Some(host.trim_end_matches('.').to_lowercase())
}

/// 从 URL 提取 hostname(不含端口)。失败返回 None,对齐 `new URL` 抛错。
fn url_hostname(value: &str) -> Option<String> {
    let scheme_end = value.find("://")?;
    let rest = &value[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return None;
    }
    // 去掉端口(`:` 后接数字)与 userinfo
    let host_part = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host_part.starts_with('[') {
        let close = host_part.find(']')?;
        &host_part[..=close]
    } else {
        let colon = host_part.find(':');
        match colon {
            Some(idx) => &host_part[..idx],
            None => host_part,
        }
    };
    Some(host.to_string())
}

/// 对齐 `hostMatches`:相等或 `.<expected>` 后缀。
fn host_matches(actual: &str, expected: &str) -> bool {
    actual == expected
        || actual
            .strip_suffix(expected)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// 对齐 `providerMatches`。
fn provider_matches(entry: &ModelCatalogEntry, provider_hint: &str) -> bool {
    let normalized_hint = normalize_provider(provider_hint);
    if normalized_hint.is_empty() {
        return false;
    }
    normalize_provider(&entry.provider_id) == normalized_hint
        || normalize_provider(&entry.provider_name) == normalized_hint
}

/// 对齐 `baseUrlMatches`:实际 host 命中已知 provider host 或条目 providerBaseUrl。
fn base_url_matches(entry: &ModelCatalogEntry, base_url: &str) -> bool {
    let Some(actual_host) = hostname(Some(base_url)) else {
        return false;
    };
    let known_hosts = known_provider_hosts(&entry.provider_id);
    let provider_host = hostname(entry.provider_base_url.as_deref());
    known_hosts
        .iter()
        .map(|s| s.to_string())
        .chain(provider_host)
        .any(|candidate| host_matches(&actual_host, &candidate))
}

/// 对齐 `exactModelMatches`。id 归一化后精确匹配,或 `providerId/id` 匹配。
fn exact_model_matches(entry: &ModelCatalogEntry, query: &str) -> bool {
    let normalized_query = normalize_model_id(query);
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_id = normalize_model_id(&entry.id);
    let normalized_full_id = format!("{}/{}", entry.provider_id.to_lowercase(), normalized_id);
    normalized_id == normalized_query || normalized_full_id == normalized_query
}

/// 对齐 `validPrice`:input/output 都定义。
fn valid_price(entry: &ModelCatalogEntry) -> bool {
    entry.cost.input.is_some() && entry.cost.output.is_some()
}

/// 对齐 `modeValue`(带最低占比 0.6,平局判空)。
#[allow(clippy::unnecessary_sort_by)] // 比较项为 tuple 嵌套字段,sort_by_key 需引入借用语义
fn mode_value<T, F>(values: &[T], total: usize, key_for: F) -> Option<T>
where
    T: Clone,
    F: Fn(&T) -> String,
{
    if values.is_empty() || total == 0 {
        return None;
    }
    let mut groups: Vec<(String, (T, usize))> = Vec::new();
    for value in values {
        let key = key_for(value);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, (_, count))) => *count += 1,
            None => groups.push((key, (value.clone(), 1))),
        }
    }
    groups.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    let winner = &groups[0].1;
    if (winner.1 as f64) / (total as f64) < CONSENSUS_MIN_SHARE {
        return None;
    }
    if groups
        .get(1)
        .is_some_and(|(_, (_, count))| *count == winner.1)
    {
        return None;
    }
    Some(winner.0.clone())
}

/// 对齐 `modeNumber`(无占比门槛,仅平局判空)。
#[allow(clippy::unnecessary_sort_by)] // 比较项为 tuple 嵌套字段
fn mode_number(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut groups: Vec<(f64, usize)> = Vec::new();
    for value in values {
        match groups
            .iter_mut()
            .find(|(v, _)| v.to_bits() == value.to_bits())
        {
            Some((_, count)) => *count += 1,
            None => groups.push((*value, 1)),
        }
    }
    groups.sort_by(|a, b| b.1.cmp(&a.1));
    let winner = groups[0];
    if groups.get(1).is_some_and(|(_, count)| *count == winner.1) {
        return None;
    }
    Some(winner.0)
}

/// 对齐 `metadataFromEntry`。
fn metadata_from_entry(entry: &ModelCatalogEntry) -> ModelCatalogPreset {
    ModelCatalogPreset {
        name: Some(entry.name.clone()),
        reasoning: entry.reasoning,
        input: entry.input.clone(),
        context_window: entry.context_window,
        max_tokens: entry.max_tokens,
        cost: None,
    }
}

/// 对齐 `consensusMetadata`。只统计有该字段的条目,但占比按全部条目算。
fn consensus_metadata(entries: &[ModelCatalogEntry]) -> ModelCatalogPreset {
    let total = entries.len();
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let reasonings: Vec<bool> = entries.iter().filter_map(|e| e.reasoning).collect();
    let inputs: Vec<Vec<String>> = entries.iter().filter_map(|e| e.input.clone()).collect();
    let context_windows: Vec<u64> = entries.iter().filter_map(|e| e.context_window).collect();
    let max_tokens: Vec<u64> = entries.iter().filter_map(|e| e.max_tokens).collect();

    ModelCatalogPreset {
        name: mode_value(&names, total, |v| v.to_lowercase()),
        reasoning: mode_value(&reasonings, total, |v| v.to_string()),
        input: mode_value(&inputs, total, |v| {
            let mut sorted = v.clone();
            sorted.sort();
            sorted.join(",")
        }),
        context_window: mode_value(&context_windows, total, |v| v.to_string()),
        max_tokens: mode_value(&max_tokens, total, |v| v.to_string()),
        cost: None,
    }
}

/// 对齐 `priceFromEntry`。
fn price_from_entry(
    entry: &ModelCatalogEntry,
    method: ModelCatalogMatchMethod,
) -> ModelCatalogPriceRecommendation {
    ModelCatalogPriceRecommendation::Reliable {
        method,
        // 对齐 TS `cacheRead ?? 0, cacheWrite ?? 0`:缺失的 cache 字段补 0(非省略)。
        cost: ModelCatalogCost {
            input: entry.cost.input,
            output: entry.cost.output,
            cache_read: Some(entry.cost.cache_read.unwrap_or(0.0)),
            cache_write: Some(entry.cost.cache_write.unwrap_or(0.0)),
        },
        provider_id: Some(entry.provider_id.clone()),
        provider_name: Some(entry.provider_name.clone()),
        support: 1,
        total: 1,
    }
}

/// 对齐 `consensusPrice`。按 (input, output) 分组,组内 cache 取众数。
#[allow(clippy::unnecessary_sort_by)] // 比较项为 Vec 长度,sort_by_key 需克隆 Vec
fn consensus_price(entries: &[ModelCatalogEntry]) -> ModelCatalogPriceRecommendation {
    let priced: Vec<&ModelCatalogEntry> = entries.iter().filter(|e| valid_price(e)).collect();
    if priced.is_empty() {
        return ModelCatalogPriceRecommendation::Unreliable {
            reason: UnreliableReason::NoValidPrice,
            support: 0,
            total: 0,
        };
    }
    if priced.len() == 1 {
        return ModelCatalogPriceRecommendation::Unreliable {
            reason: UnreliableReason::InsufficientSupport,
            support: 1,
            total: 1,
        };
    }

    let mut groups: Vec<(String, Vec<&ModelCatalogEntry>)> = Vec::new();
    for entry in &priced {
        let input = entry.cost.input.unwrap_or(0.0);
        let output = entry.cost.output.unwrap_or(0.0);
        let key = format!("[{input},{output}]");
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(entry),
            None => groups.push((key, vec![*entry])),
        }
    }
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    let winner = &groups[0];
    // 对齐 TS:`hasConsensus = share >= CONSENSUS_MIN_SHARE || winner.length >= CONSENSUS_MIN_SUPPORT`。
    let has_consensus = (winner.1.len() as f64) / (priced.len() as f64) >= CONSENSUS_MIN_SHARE
        || winner.1.len() >= CONSENSUS_MIN_SUPPORT;
    if groups
        .get(1)
        .is_some_and(|(_, g)| g.len() == winner.1.len())
        || !has_consensus
    {
        return ModelCatalogPriceRecommendation::Unreliable {
            reason: UnreliableReason::Conflict,
            support: winner.1.len(),
            total: priced.len(),
        };
    }

    // 对齐 TS `winner.map(e => e.cost.cacheRead ?? 0)`:缺失值补 0 后再取众数,结果 `?? 0`。
    let cache_reads: Vec<f64> = winner
        .1
        .iter()
        .map(|e| e.cost.cache_read.unwrap_or(0.0))
        .collect();
    let cache_writes: Vec<f64> = winner
        .1
        .iter()
        .map(|e| e.cost.cache_write.unwrap_or(0.0))
        .collect();
    ModelCatalogPriceRecommendation::Reliable {
        method: ModelCatalogMatchMethod::Consensus,
        cost: ModelCatalogCost {
            input: winner.1[0].cost.input,
            output: winner.1[0].cost.output,
            cache_read: Some(mode_number(&cache_reads).unwrap_or(0.0)),
            cache_write: Some(mode_number(&cache_writes).unwrap_or(0.0)),
        },
        provider_id: None,
        provider_name: None,
        support: winner.1.len(),
        total: priced.len(),
    }
}

/// 对齐 `flattenModelsDevCatalog`。原始 models.dev JSON → 扁平条目。
pub fn flatten_models_dev_catalog(value: &Value) -> Vec<ModelCatalogEntry> {
    let Some(obj) = value.as_object() else {
        return vec![];
    };
    let mut entries = Vec::new();

    for (provider_id, raw_provider) in obj {
        if !raw_provider.is_object() {
            continue;
        }
        let Some(raw_models) = raw_provider.get("models").filter(|m| m.is_object()) else {
            continue;
        };
        let provider_name =
            clean_string(&raw_provider["name"]).unwrap_or_else(|| provider_id.clone());
        let provider_base_url = clean_string(&raw_provider["api"]);

        let Some(models_obj) = raw_models.as_object() else {
            continue;
        };
        for (fallback_id, raw_model) in models_obj {
            if !raw_model.is_object() {
                continue;
            }
            let Some(id) = clean_string(&raw_model["id"]).or_else(|| Some(fallback_id.clone()))
            else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let name = clean_string(&raw_model["name"]).unwrap_or_else(|| id.clone());
            let mut entry = ModelCatalogEntry {
                key: format!("{provider_id}/{id}"),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
                provider_base_url: provider_base_url.clone(),
                id: id.clone(),
                name,
                reasoning: None,
                input: None,
                context_window: None,
                max_tokens: None,
                cost: read_cost(&raw_model["cost"]),
            };
            if let Some(reasoning) = raw_model.get("reasoning").and_then(|v| v.as_bool()) {
                entry.reasoning = Some(reasoning);
            }
            let input = read_input_modalities(&raw_model["modalities"]);
            if input.is_some() {
                entry.input = input;
            }
            if raw_model.get("limit").is_some_and(|l| l.is_object()) {
                let limit = &raw_model["limit"];
                if let Some(context) = optional_positive_number(&limit["context"]).map(|v| v as u64)
                {
                    entry.context_window = Some(context);
                }
                if let Some(output) = optional_positive_number(&limit["output"]).map(|v| v as u64) {
                    entry.max_tokens = Some(output);
                }
            }
            entries.push(entry);
        }
    }

    entries
}

/// 对齐 `recommendModelCatalogPreset`。
pub fn recommend_model_catalog_preset(
    entries: &[ModelCatalogEntry],
    query: &str,
    provider_hint: &str,
    base_url: &str,
) -> ModelCatalogRecommendation {
    let exact_entries: Vec<&ModelCatalogEntry> = entries
        .iter()
        .filter(|e| exact_model_matches(e, query))
        .collect();
    if exact_entries.is_empty() {
        return ModelCatalogRecommendation {
            exact_matches: 0,
            metadata_method: ModelCatalogMatchMethod::None,
            matched_provider_id: None,
            matched_provider_name: None,
            preset: ModelCatalogPreset::default(),
            price: ModelCatalogPriceRecommendation::Unreliable {
                reason: UnreliableReason::NoExactMatch,
                support: 0,
                total: 0,
            },
        };
    }

    let provider_entries: Vec<&ModelCatalogEntry> = exact_entries
        .iter()
        .copied()
        .filter(|e| provider_matches(e, provider_hint))
        .collect();
    let base_url_entries: Vec<&ModelCatalogEntry> = exact_entries
        .iter()
        .copied()
        .filter(|e| base_url_matches(e, base_url))
        .collect();
    let metadata_entry = provider_entries
        .first()
        .copied()
        .or_else(|| base_url_entries.first().copied());
    let metadata_method = if !provider_entries.is_empty() {
        ModelCatalogMatchMethod::Provider
    } else if !base_url_entries.is_empty() {
        ModelCatalogMatchMethod::BaseUrl
    } else {
        ModelCatalogMatchMethod::Consensus
    };
    let mut preset = match metadata_entry {
        Some(entry) => metadata_from_entry(entry),
        None => consensus_metadata(
            &exact_entries
                .iter()
                .map(|e| (*e).clone())
                .collect::<Vec<_>>(),
        ),
    };

    let provider_price = provider_entries.iter().copied().find(|e| valid_price(e));
    let base_url_price = base_url_entries.iter().copied().find(|e| valid_price(e));
    let price = match provider_price {
        Some(entry) => price_from_entry(entry, ModelCatalogMatchMethod::Provider),
        None => match base_url_price {
            Some(entry) => price_from_entry(entry, ModelCatalogMatchMethod::BaseUrl),
            None => consensus_price(
                &exact_entries
                    .iter()
                    .map(|e| (*e).clone())
                    .collect::<Vec<_>>(),
            ),
        },
    };
    if let ModelCatalogPriceRecommendation::Reliable { cost, .. } = &price {
        preset.cost = Some(cost.clone());
    }

    ModelCatalogRecommendation {
        exact_matches: exact_entries.len(),
        metadata_method,
        matched_provider_id: metadata_entry.map(|e| e.provider_id.clone()),
        matched_provider_name: metadata_entry.map(|e| e.provider_name.clone()),
        preset,
        price,
    }
}

/// 对齐 `matchRank`。query 已由调用方归一化,此处为精确小写比较。
fn match_rank(entry: &ModelCatalogEntry, query: &str, provider_hint: &str) -> f64 {
    let id = entry.id.to_lowercase();
    let name = entry.name.to_lowercase();
    let provider_id = entry.provider_id.to_lowercase();
    let provider_name = entry.provider_name.to_lowercase();
    let full_id = format!("{provider_id}/{id}");

    let mut rank = 20.0;
    if query.is_empty() {
        rank = 10.0;
    } else if id == query || full_id == query {
        rank = 0.0;
    } else if name == query {
        rank = 1.0;
    } else if id.starts_with(query) || name.starts_with(query) {
        rank = 2.0;
    } else if full_id.starts_with(query) || provider_id == query || provider_name == query {
        rank = 3.0;
    } else if id.contains(query) || name.contains(query) {
        rank = 4.0;
    } else if full_id.contains(query) || provider_name.contains(query) {
        rank = 5.0;
    }

    if rank < 20.0
        && !provider_hint.is_empty()
        && (provider_id == provider_hint || provider_name == provider_hint)
    {
        rank -= 0.5;
    }
    rank
}

/// 对齐 `localeCompare`(sensitivity: "base"):小写后比较。
fn locale_compare_base(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// 对齐 `localeCompare`(numeric: true, sensitivity: "base"):
/// 小写 + 自然数值排序(数字段按数值比较)。
fn locale_compare_numeric_base(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    natural_cmp(&a, &b)
}

/// 自然排序:数字段按数值(去前导零后比长度,再比字节),其余按字节。
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let (mut i, mut j) = (0, 0);
    loop {
        let ai = ab.get(i).copied();
        let bj = bb.get(j).copied();
        let (Some(a_byte), Some(b_byte)) = (ai, bj) else {
            return (ab.len() - i).cmp(&(bb.len() - j));
        };
        if a_byte.is_ascii_digit() && b_byte.is_ascii_digit() {
            let mut ai_end = i;
            while ai_end < ab.len() && ab[ai_end].is_ascii_digit() {
                ai_end += 1;
            }
            let mut bj_end = j;
            while bj_end < bb.len() && bb[bj_end].is_ascii_digit() {
                bj_end += 1;
            }
            let num_a = &a[i..ai_end];
            let num_b = &b[j..bj_end];
            let order = compare_digit_runs(num_a, num_b);
            if order != std::cmp::Ordering::Equal {
                return order;
            }
            i = ai_end;
            j = bj_end;
        } else if a_byte != b_byte {
            return a_byte.cmp(&b_byte);
        } else {
            i += 1;
            j += 1;
        }
    }
}

/// 比较两个纯数字串的数值大小(去前导零后比长度,再比字节序)。
fn compare_digit_runs(a: &str, b: &str) -> std::cmp::Ordering {
    let a_trim = a.trim_start_matches('0');
    let b_trim = b.trim_start_matches('0');
    if a_trim.is_empty() && b_trim.is_empty() {
        return std::cmp::Ordering::Equal;
    }
    if a_trim.is_empty() {
        return std::cmp::Ordering::Less;
    }
    if b_trim.is_empty() {
        return std::cmp::Ordering::Greater;
    }
    match a_trim.len().cmp(&b_trim.len()) {
        std::cmp::Ordering::Equal => a_trim.cmp(b_trim),
        other => other,
    }
}

/// 对齐 `searchModelCatalog`。
pub fn search_model_catalog(
    entries: &[ModelCatalogEntry],
    query: &str,
    provider_hint: &str,
    limit: usize,
) -> Vec<ModelCatalogEntry> {
    let normalized_query = query.trim().to_lowercase();
    let normalized_provider = provider_hint.trim().to_lowercase();
    let floored = limit;
    let capped_limit = (if floored == 0 { 50 } else { floored.min(100) }).max(1);

    let mut ranked: Vec<(f64, &ModelCatalogEntry)> = entries
        .iter()
        .map(|entry| {
            (
                match_rank(entry, &normalized_query, &normalized_provider),
                entry,
            )
        })
        .filter(|(rank, _)| normalized_query.is_empty() || *rank < 20.0)
        .collect();

    ranked.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| locale_compare_base(&a.1.provider_name, &b.1.provider_name))
            .then_with(|| locale_compare_numeric_base(&a.1.name, &b.1.name))
            .then_with(|| locale_compare_numeric_base(&a.1.id, &b.1.id))
    });

    ranked
        .into_iter()
        .take(capped_limit)
        .map(|(_, entry)| entry.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(
        id: &str,
        provider_id: &str,
        provider_name: &str,
        cost: ModelCatalogCost,
    ) -> ModelCatalogEntry {
        ModelCatalogEntry {
            key: format!("{provider_id}/{id}"),
            provider_id: provider_id.to_string(),
            provider_name: provider_name.to_string(),
            provider_base_url: None,
            id: id.to_string(),
            name: id.to_string(),
            reasoning: None,
            input: None,
            context_window: None,
            max_tokens: None,
            cost,
        }
    }

    #[test]
    fn flatten_basic_catalog() {
        let catalog = json!({
            "anthropic": {
                "name": "Anthropic",
                "api": "https://api.anthropic.com",
                "models": {
                    "claude-3-5-sonnet": {
                        "id": "claude-3-5-sonnet-20241022",
                        "name": "Claude 3.5 Sonnet",
                        "cost": { "input": 3.0, "output": 15.0, "cache_read": 0.3 },
                        "reasoning": true,
                        "modalities": { "input": ["text", "image"] },
                        "limit": { "context": 200000, "output": 8192 }
                    }
                }
            }
        });
        let entries = flatten_models_dev_catalog(&catalog);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.key, "anthropic/claude-3-5-sonnet-20241022");
        assert_eq!(e.provider_id, "anthropic");
        assert_eq!(e.provider_name, "Anthropic");
        assert_eq!(
            e.provider_base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(e.id, "claude-3-5-sonnet-20241022");
        assert_eq!(e.name, "Claude 3.5 Sonnet");
        assert_eq!(e.reasoning, Some(true));
        assert_eq!(e.input, Some(vec!["text".to_string(), "image".to_string()]));
        assert_eq!(e.context_window, Some(200000));
        assert_eq!(e.max_tokens, Some(8192));
        assert_eq!(e.cost.input, Some(3.0));
        assert_eq!(e.cost.output, Some(15.0));
        assert_eq!(e.cost.cache_read, Some(0.3));
        assert_eq!(e.cost.cache_write, None);
    }

    #[test]
    fn flatten_fallback_and_filters() {
        let catalog = json!({
            "p": {
                "models": {
                    "m1": { "id": "", "name": "no id" },         // id 为空 → 用 fallback?不,cleanString("") → None → fallback "m1"
                    "m2": "string not object",                    // 跳过
                    "m3": { "name": "M3", "cost": { "input": -1, "output": 1.5 } }, // 负 input 丢弃
                }
            }
        });
        let entries = flatten_models_dev_catalog(&catalog);
        // m1:id 空 → cleanString(None) → fallback "m1"
        assert!(entries.iter().any(|e| e.id == "m1"));
        // m2 非对象跳过
        assert!(!entries.iter().any(|e| e.id == "m2"));
        let m3 = entries.iter().find(|e| e.id == "m3").unwrap();
        assert_eq!(m3.name, "M3");
        assert_eq!(m3.cost.input, None);
        assert_eq!(m3.cost.output, Some(1.5));
    }

    #[test]
    fn flatten_skips_non_object_models() {
        assert_eq!(
            flatten_models_dev_catalog(&json!({"p": {"models": 5}})),
            Vec::<ModelCatalogEntry>::new()
        );
        assert_eq!(
            flatten_models_dev_catalog(&json!(null)),
            Vec::<ModelCatalogEntry>::new()
        );
        assert_eq!(
            flatten_models_dev_catalog(&json!([])),
            Vec::<ModelCatalogEntry>::new()
        );
    }

    #[test]
    fn read_input_modalities_dedup_and_filter() {
        let v = json!({"input": ["TEXT", "text", "image", "audio", "  video  ", 42]});
        assert_eq!(
            read_input_modalities(&v),
            Some(vec!["text".to_string(), "image".to_string()])
        );
        assert_eq!(read_input_modalities(&json!({"input": []})), None);
        assert_eq!(read_input_modalities(&json!({"input": ["audio"]})), None);
        assert_eq!(read_input_modalities(&json!({})), None);
        assert_eq!(read_input_modalities(&json!("x")), None);
    }

    #[test]
    fn hostname_and_matches() {
        assert_eq!(
            hostname(Some("https://api.openai.com/v1")),
            Some("api.openai.com".to_string())
        );
        assert_eq!(
            hostname(Some("http://openrouter.ai:8080/x")),
            Some("openrouter.ai".to_string())
        );
        assert_eq!(hostname(Some("openrouter.ai")), None); // 无 scheme → None
        assert_eq!(
            hostname(Some("HTTPS://API.OPENAI.COM.")),
            Some("api.openai.com".to_string())
        );
        assert_eq!(hostname(None), None);
        assert!(host_matches("api.anthropic.com", "anthropic.com"));
        assert!(host_matches("api.anthropic.com", "api.anthropic.com"));
        assert!(!host_matches("evil-anthropic.com", "anthropic.com"));
        assert!(!host_matches("anthropic.com.evil.com", "anthropic.com"));
    }

    #[test]
    fn normalize_functions() {
        assert_eq!(normalize_provider(" Open AI! "), "openai");
        assert_eq!(normalize_provider("Anthropic"), "anthropic");
        assert_eq!(normalize_model_id(" models/claude-3 "), "claude-3");
        assert_eq!(normalize_model_id("Claude-3"), "claude-3");
    }

    #[test]
    fn exact_matches() {
        let e = entry(
            "models/foo/bar",
            "acme",
            "Acme",
            ModelCatalogCost::default(),
        );
        // normalizeModelId 去掉 models/ 前缀
        assert!(exact_model_matches(&e, "models/foo/bar"));
        assert!(exact_model_matches(&e, "foo/bar"));
        // providerId/id 全小写拼接
        assert!(exact_model_matches(&e, "acme/foo/bar"));
        assert!(!exact_model_matches(&e, "foo/baz"));
        assert!(!exact_model_matches(&e, ""));
    }

    #[test]
    fn mode_value_semantics() {
        let values = vec!["a", "a", "b"];
        assert_eq!(mode_value(&values, 3, |v| v.to_string()), Some("a"));
        // 低于 0.6 占比
        let values = vec!["a", "b", "c"];
        assert_eq!(mode_value(&values, 3, |v| v.to_string()), None);
        // 平局
        let values = vec!["a", "a", "b", "b"];
        assert_eq!(mode_value(&values, 4, |v| v.to_string()), None);
        // 空
        assert_eq!(mode_value::<&str, _>(&[], 3, |v| v.to_string()), None);
        assert_eq!(mode_value(&vec!["x"], 0, |v| v.to_string()), None);
        // 恰好 0.6
        let values = vec!["a", "a", "a", "b", "b"];
        assert_eq!(mode_value(&values, 5, |v| v.to_string()), Some("a"));
    }

    #[test]
    fn mode_number_semantics() {
        assert_eq!(mode_number(&[1.0, 1.0, 2.0]), Some(1.0));
        assert_eq!(mode_number(&[1.0, 2.0]), None); // 平局
        assert_eq!(mode_number(&[]), None);
    }

    #[test]
    fn flatten_and_recommend_preset() {
        let catalog = json!({
            "openai": {
                "name": "OpenAI",
                "api": "https://api.openai.com",
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o",
                        "name": "GPT-4o",
                        "cost": { "input": 2.5, "output": 10.0 },
                        "limit": { "context": 128000 }
                    },
                    "gpt-4o-mini": {
                        "id": "gpt-4o-mini",
                        "name": "GPT-4o mini",
                        "cost": { "input": 0.15, "output": 0.6 }
                    }
                }
            },
            "acme": {
                "name": "Acme",
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o",
                        "name": "Acme GPT-4o",
                        "cost": { "input": 3.0, "output": 12.0 }
                    }
                }
            }
        });
        let entries = flatten_models_dev_catalog(&catalog);
        let rec = recommend_model_catalog_preset(&entries, "gpt-4o", "", "");
        // provider hint 为空 → 无 provider/base-url 命中 → consensus
        assert_eq!(rec.exact_matches, 2);
        assert_eq!(rec.metadata_method, ModelCatalogMatchMethod::Consensus);
        assert_eq!(rec.matched_provider_id, None);
        // consensus:两个条目价格不同 → conflict
        assert_eq!(
            rec.price,
            ModelCatalogPriceRecommendation::Unreliable {
                reason: UnreliableReason::Conflict,
                support: 1,
                total: 2,
            }
        );
        // preset 未填充 cost
        assert_eq!(rec.preset.cost, None);

        // provider hint 命中 openai → provider 命中,价格可靠
        let rec2 = recommend_model_catalog_preset(&entries, "gpt-4o", "openai", "");
        assert_eq!(rec2.metadata_method, ModelCatalogMatchMethod::Provider);
        assert_eq!(rec2.matched_provider_id.as_deref(), Some("openai"));
        assert_eq!(rec2.preset.name.as_deref(), Some("GPT-4o"));
        assert_eq!(rec2.preset.context_window, Some(128000));
        assert_eq!(
            rec2.preset.cost,
            Some(ModelCatalogCost {
                input: Some(2.5),
                output: Some(10.0),
                // 对齐 TS priceFromEntry:`cacheRead ?? 0, cacheWrite ?? 0`(缺失补 0)。
                cache_read: Some(0.0),
                cache_write: Some(0.0),
            })
        );
        assert_eq!(
            rec2.price,
            ModelCatalogPriceRecommendation::Reliable {
                method: ModelCatalogMatchMethod::Provider,
                cost: ModelCatalogCost {
                    input: Some(2.5),
                    output: Some(10.0),
                    // 对齐 TS priceFromEntry:`cacheRead ?? 0, cacheWrite ?? 0`(缺失补 0)。
                    cache_read: Some(0.0),
                    cache_write: Some(0.0),
                },
                provider_id: Some("openai".to_string()),
                provider_name: Some("OpenAI".to_string()),
                support: 1,
                total: 1,
            }
        );

        // base-url 命中(api.openai.com)
        let rec3 =
            recommend_model_catalog_preset(&entries, "gpt-4o", "", "https://api.openai.com/v1");
        assert_eq!(rec3.metadata_method, ModelCatalogMatchMethod::BaseUrl);
        assert_eq!(rec3.matched_provider_id.as_deref(), Some("openai"));
        assert_eq!(rec3.preset.name.as_deref(), Some("GPT-4o"));

        // 无精确匹配
        let rec4 = recommend_model_catalog_preset(&entries, "nonexistent", "", "");
        assert_eq!(rec4.exact_matches, 0);
        assert_eq!(rec4.metadata_method, ModelCatalogMatchMethod::None);
        assert_eq!(
            rec4.price,
            ModelCatalogPriceRecommendation::Unreliable {
                reason: UnreliableReason::NoExactMatch,
                support: 0,
                total: 0,
            }
        );
    }

    #[test]
    fn consensus_price_semantics() {
        let mk = |input: f64, output: f64, cache_read: Option<f64>| {
            entry(
                "x",
                "p",
                "P",
                ModelCatalogCost {
                    input: Some(input),
                    output: Some(output),
                    cache_read,
                    cache_write: None,
                },
            )
        };
        // 两个相同价格 → reliable consensus, cache 取众数
        let entries = vec![
            mk(1.0, 2.0, Some(0.1)),
            mk(1.0, 2.0, Some(0.1)),
            mk(1.0, 2.0, Some(0.2)),
        ];
        let price = consensus_price(&entries);
        assert_eq!(
            price,
            ModelCatalogPriceRecommendation::Reliable {
                method: ModelCatalogMatchMethod::Consensus,
                cost: ModelCatalogCost {
                    input: Some(1.0),
                    output: Some(2.0),
                    cache_read: Some(0.1),
                    // 对齐 TS consensusPrice:`modeNumber(winner.map(e => e.cost.cacheWrite ?? 0)) ?? 0`
                    // 所有 entry 的 cacheWrite 缺失 → 补 0 → 众数 0。
                    cache_write: Some(0.0),
                },
                provider_id: None,
                provider_name: None,
                support: 3,
                total: 3,
            }
        );

        // 只有 1 个有效价格 → insufficient-support
        let single = vec![mk(1.0, 2.0, None)];
        assert_eq!(
            consensus_price(&single),
            ModelCatalogPriceRecommendation::Unreliable {
                reason: UnreliableReason::InsufficientSupport,
                support: 1,
                total: 1,
            }
        );

        // 无有效价格
        let none = vec![entry(
            "x",
            "p",
            "P",
            ModelCatalogCost {
                input: None,
                output: None,
                cache_read: None,
                cache_write: None,
            },
        )];
        assert_eq!(
            consensus_price(&none),
            ModelCatalogPriceRecommendation::Unreliable {
                reason: UnreliableReason::NoValidPrice,
                support: 0,
                total: 0,
            }
        );
    }

    #[test]
    fn search_ranking_and_limit() {
        let entries = vec![
            entry("gpt-4o", "openai", "OpenAI", ModelCatalogCost::default()),
            entry(
                "gpt-4o-mini",
                "openai",
                "OpenAI",
                ModelCatalogCost::default(),
            ),
            entry(
                "claude-3-5-sonnet",
                "anthropic",
                "Anthropic",
                ModelCatalogCost::default(),
            ),
            entry(
                "claude-3-haiku",
                "anthropic",
                "Anthropic",
                ModelCatalogCost::default(),
            ),
        ];
        // 空 query → rank 10,按 providerName/name/id 自然排序
        let all = search_model_catalog(&entries, "", "", 50);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].id, "claude-3-5-sonnet"); // Anthropic 在 OpenAI 前
        assert_eq!(all[3].id, "gpt-4o-mini"); // gpt-4o-mini 自然数值序在 gpt-4o 后

        // 查询 "gpt" → 精确前缀命中优先
        let gpt = search_model_catalog(&entries, "gpt", "", 50);
        assert_eq!(gpt[0].id, "gpt-4o");
        assert_eq!(gpt[1].id, "gpt-4o-mini");

        // providerHint 加分:anthropic 下查 "claude" → claude-3-5-sonnet 先
        let anthropic = search_model_catalog(&entries, "claude", "anthropic", 50);
        assert_eq!(anthropic[0].id, "claude-3-5-sonnet");
        assert_eq!(anthropic[1].id, "claude-3-haiku");

        // limit 截断与下限
        assert_eq!(search_model_catalog(&entries, "", "", 0).len(), 50.min(4));
        assert_eq!(search_model_catalog(&entries, "", "", 2).len(), 2);
        assert_eq!(search_model_catalog(&entries, "", "", 500).len(), 4);
    }

    #[test]
    fn natural_sort_ordering() {
        assert_eq!(
            locale_compare_numeric_base("gpt-4o", "gpt-4o-mini"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            locale_compare_numeric_base("a2", "a10"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            locale_compare_numeric_base("a10", "a2"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            locale_compare_numeric_base("a2", "a2"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            locale_compare_numeric_base("GPT-4O", "gpt-4o"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn serialize_shapes() {
        let rec = ModelCatalogRecommendation {
            exact_matches: 1,
            metadata_method: ModelCatalogMatchMethod::Provider,
            matched_provider_id: Some("openai".to_string()),
            matched_provider_name: Some("OpenAI".to_string()),
            preset: ModelCatalogPreset {
                name: Some("GPT-4o".to_string()),
                reasoning: Some(true),
                input: Some(vec!["text".to_string(), "image".to_string()]),
                context_window: Some(128000),
                max_tokens: None,
                cost: Some(ModelCatalogCost {
                    input: Some(2.5),
                    output: Some(10.0),
                    cache_read: None,
                    cache_write: None,
                }),
            },
            price: ModelCatalogPriceRecommendation::Reliable {
                method: ModelCatalogMatchMethod::BaseUrl,
                cost: ModelCatalogCost {
                    input: Some(2.5),
                    output: Some(10.0),
                    cache_read: None,
                    cache_write: None,
                },
                provider_id: Some("openai".to_string()),
                provider_name: Some("OpenAI".to_string()),
                support: 1,
                total: 1,
            },
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["exactMatches"], 1);
        assert_eq!(json["metadataMethod"], "provider");
        assert_eq!(json["matchedProviderId"], "openai");
        assert!(json["preset"]["cost"]["input"].is_number());
        // Option::None 字段不序列化
        assert!(json["preset"].get("maxTokens").is_none());
        assert!(json["price"]["method"].is_string());
        assert_eq!(json["price"]["method"], "base-url");
        assert!(json["preset"]["cost"].get("cacheRead").is_none());
    }

    #[test]
    fn unreliable_serialize_shape() {
        let rec = ModelCatalogRecommendation {
            exact_matches: 0,
            metadata_method: ModelCatalogMatchMethod::None,
            matched_provider_id: None,
            matched_provider_name: None,
            preset: ModelCatalogPreset::default(),
            price: ModelCatalogPriceRecommendation::Unreliable {
                reason: UnreliableReason::NoExactMatch,
                support: 0,
                total: 0,
            },
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["metadataMethod"], "none");
        assert_eq!(json["price"]["status"], "unreliable");
        assert_eq!(json["price"]["reason"], "no-exact-match");
        // preset 全空 → 无字段
        assert!(json["preset"]
            .as_object()
            .map(|o| o.is_empty())
            .unwrap_or(false));
    }
}
