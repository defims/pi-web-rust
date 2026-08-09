//! 对齐 `lib/provider-listing.ts`。provider 列表构造(纯逻辑)。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListingInput {
    pub id: String,
    pub name: String,
    pub has_api_key_login: bool,
    pub has_oauth: bool,
    pub oauth_name: Option<String>,
    pub status: ProviderAuthStatus,
    pub credential_type: Option<String>,
    pub model_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthStatus {
    pub configured: bool,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyProviderListing {
    pub id: String,
    pub display_name: String,
    pub configured: bool,
    pub source: Option<String>,
    pub model_count: u64,
    pub supports_oauth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthProviderListing {
    pub id: String,
    pub name: String,
    pub uses_callback_server: bool,
    pub logged_in: bool,
    pub supports_api_key: bool,
}

const CUSTOM_PROVIDER_SOURCES: &[&str] = &["models_json_key", "models_json_command"];

fn oauth_display_name(id: &str, oauth_name: Option<&str>, fallback: &str) -> String {
    match id {
        "openai-codex" => "ChatGPT Plus/Pro".to_string(),
        "github-copilot" => "GitHub Copilot".to_string(),
        _ => oauth_name.unwrap_or(fallback).to_string(),
    }
}

fn dedupe_by_id(providers: &[ProviderListingInput]) -> Vec<&ProviderListingInput> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for p in providers {
        if seen.insert(&p.id) {
            result.push(p);
        }
    }
    result
}

/// 对齐 `buildApiKeyProviderList`。
pub fn build_api_key_provider_list(providers: &[ProviderListingInput]) -> Vec<ApiKeyProviderListing> {
    dedupe_by_id(providers)
        .into_iter()
        .filter(|p| {
            p.has_api_key_login
                && !p
                    .status
                    .source
                    .as_deref()
                    .is_some_and(|s| CUSTOM_PROVIDER_SOURCES.contains(&s))
        })
        .map(|p| {
            let configured = p.status.configured && p.credential_type.as_deref() != Some("oauth");
            ApiKeyProviderListing {
                source: if configured { p.status.source.clone() } else { None },
                id: p.id.clone(),
                display_name: p.name.clone(),
                configured,
                model_count: p.model_count,
                supports_oauth: p.has_oauth,
            }
        })
        .collect()
}

/// 对齐 `buildOAuthProviderList`。
pub fn build_oauth_provider_list(providers: &[ProviderListingInput]) -> Vec<OAuthProviderListing> {
    dedupe_by_id(providers)
        .into_iter()
        .filter(|p| p.has_oauth)
        .map(|p| OAuthProviderListing {
            name: oauth_display_name(&p.id, p.oauth_name.as_deref(), &p.name),
            id: p.id.clone(),
            uses_callback_server: false,
            logged_in: p.credential_type.as_deref() == Some("oauth"),
            supports_api_key: p.has_api_key_login,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, api_key: bool, oauth: bool) -> ProviderListingInput {
        ProviderListingInput {
            id: id.into(),
            name: id.into(),
            has_api_key_login: api_key,
            has_oauth: oauth,
            oauth_name: None,
            status: ProviderAuthStatus { configured: false, source: None },
            credential_type: None,
            model_count: 3,
        }
    }

    #[test]
    fn api_key_list() {
        let providers = vec![
            input("openai", true, false),
            input("anthropic", true, true),
            input("google", false, true),
        ];
        let list = build_api_key_provider_list(&providers);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|p| p.supports_oauth == (p.id == "anthropic")));
    }

    #[test]
    fn oauth_list() {
        let providers = vec![
            input("github-copilot", false, true),
            input("openai", true, false),
        ];
        let list = build_oauth_provider_list(&providers);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "GitHub Copilot");
    }
}
