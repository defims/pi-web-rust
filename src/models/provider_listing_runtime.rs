//! 对齐 `lib/provider-listing-runtime.ts`。ModelRuntime ↔ provider 列表的适配层。
//!
//! 把引擎的模型/凭证/提供商信息收集成 `provider_listing::ProviderListingInput`
//! 列表。凭证类型只收 `api_key` / `oauth`;损坏的 auth.json 不能清空列表
//! (回退到 per-provider auth status)。引擎操作经 [`ProviderRuntime`] trait 注入。

use crate::models::provider_listing::ProviderListingInput;

/// 引擎的提供商/模型/凭证操作面。
pub trait ProviderRuntime {
    /// 所有模型(`getModels()`),返回 `(provider, modelId)` 列表。
    fn models(&self) -> Vec<(String, String)>;
    /// `listCredentials()`:provider 凭证的类型(只关心 api_key/oauth)。
    fn list_credentials(&self) -> Result<Vec<(String, String)>, String>;
    /// 单个 provider 定义(对齐 `provider.auth.apiKey?.login` / `auth.oauth`)。
    fn provider_auth(&self, provider_id: &str) -> ProviderAuthDecl;
    /// `getProviderAuthStatus(providerId)`。
    fn auth_status(&self, provider_id: &str) -> AuthStatus;
}

/// 对齐 `provider.auth` 的相关声明。
#[derive(Debug, Clone, Default)]
pub struct ProviderAuthDecl {
    /// `provider.auth.apiKey?.login`。
    pub api_key_login: bool,
    /// `provider.auth.oauth` 是否存在。
    pub has_oauth: bool,
    /// `provider.auth.oauth?.name`。
    pub oauth_name: Option<String>,
}

/// 对齐 `getProviderAuthStatus` 的返回。
#[derive(Debug, Clone, Default)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: Option<String>,
}

/// 对齐 `collectProviderListingInputs`。
///
/// 凭证类型以 `list_credentials` 优先(失败回退为空 map);
/// 与 `getProviderAuthStatus` 并存,后者始终参与。
pub fn collect_provider_listing_inputs(
    runtime: &dyn ProviderRuntime,
    provider_names: &[(String, String)], // (id, name)
) -> Vec<ProviderListingInput> {
    let models = runtime.models();
    let mut credential_types: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if let Ok(credentials) = runtime.list_credentials() {
        for (provider_id, credential_type) in credentials {
            if credential_type == "api_key" || credential_type == "oauth" {
                credential_types.insert(provider_id, credential_type);
            }
        }
    }

    provider_names
        .iter()
        .map(|(id, name)| {
            let auth = runtime.provider_auth(id);
            let status = runtime.auth_status(id);
            let model_count = models
                .iter()
                .filter(|(provider, _)| provider == id)
                .count();
            ProviderListingInput {
                id: id.clone(),
                name: name.clone(),
                has_api_key_login: auth.api_key_login,
                has_oauth: auth.has_oauth,
                oauth_name: auth.oauth_name,
                status: crate::models::provider_listing::ProviderAuthStatus {
                    configured: status.configured,
                    source: status.source,
                },
                credential_type: credential_types.get(id).cloned(),
                model_count: model_count as u64,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRuntime {
        models: Vec<(String, String)>,
        credentials: Result<Vec<(String, String)>, String>,
        auths: std::collections::HashMap<String, ProviderAuthDecl>,
        statuses: std::collections::HashMap<String, AuthStatus>,
    }

    impl ProviderRuntime for FakeRuntime {
        fn models(&self) -> Vec<(String, String)> {
            self.models.clone()
        }
        fn list_credentials(&self) -> Result<Vec<(String, String)>, String> {
            self.credentials.clone()
        }
        fn provider_auth(&self, provider_id: &str) -> ProviderAuthDecl {
            self.auths.get(provider_id).cloned().unwrap_or_default()
        }
        fn auth_status(&self, provider_id: &str) -> AuthStatus {
            self.statuses.get(provider_id).cloned().unwrap_or_default()
        }
    }

    #[test]
    fn collects_inputs() {
        let runtime = FakeRuntime {
            models: vec![
                ("openai".to_string(), "gpt-4o".to_string()),
                ("openai".to_string(), "gpt-4o-mini".to_string()),
                ("anthropic".to_string(), "claude".to_string()),
            ],
            credentials: Ok(vec![
                ("openai".to_string(), "api_key".to_string()),
                ("anthropic".to_string(), "oauth".to_string()),
            ]),
            auths: std::collections::HashMap::from([
                (
                    "openai".to_string(),
                    ProviderAuthDecl { api_key_login: true, has_oauth: true, oauth_name: Some("OpenAI OAuth".to_string()) },
                ),
                (
                    "anthropic".to_string(),
                    ProviderAuthDecl { api_key_login: true, has_oauth: true, oauth_name: Some("Anthropic (Claude Pro/Max)".to_string()) },
                ),
            ]),
            statuses: std::collections::HashMap::from([
                ("openai".to_string(), AuthStatus { configured: true, source: Some("env".to_string()) }),
            ]),
        };
        let names = vec![
            ("openai".to_string(), "OpenAI".to_string()),
            ("anthropic".to_string(), "Anthropic".to_string()),
            ("local".to_string(), "Local".to_string()),
        ];
        let inputs = collect_provider_listing_inputs(&runtime, &names);
        assert_eq!(inputs.len(), 3);

        let openai = inputs.iter().find(|i| i.id == "openai").unwrap();
        assert_eq!(openai.name, "OpenAI");
        assert_eq!(openai.has_api_key_login, true);
        assert_eq!(openai.has_oauth, true);
        assert_eq!(openai.oauth_name.as_deref(), Some("OpenAI OAuth"));
        assert_eq!(openai.status.configured, true);
        assert_eq!(openai.status.source.as_deref(), Some("env"));
        assert_eq!(openai.credential_type.as_deref(), Some("api_key"));
        assert_eq!(openai.model_count, 2);

        let anthropic = inputs.iter().find(|i| i.id == "anthropic").unwrap();
        assert_eq!(anthropic.credential_type.as_deref(), Some("oauth"));
        assert_eq!(anthropic.model_count, 1);

        // 无 auth/status/凭证的 provider → 默认值
        let local = inputs.iter().find(|i| i.id == "local").unwrap();
        assert_eq!(local.has_api_key_login, false);
        assert_eq!(local.has_oauth, false);
        assert_eq!(local.status.configured, false);
        assert_eq!(local.credential_type, None);
        assert_eq!(local.model_count, 0);
    }

    #[test]
    fn damaged_credentials_falls_back() {
        let runtime = FakeRuntime {
            models: vec![],
            credentials: Err("Invalid auth.json".to_string()),
            auths: std::collections::HashMap::from([(
                "p".to_string(),
                ProviderAuthDecl { api_key_login: true, has_oauth: false, oauth_name: None },
            )]),
            statuses: std::collections::HashMap::from([(
                "p".to_string(),
                AuthStatus { configured: true, source: Some("models_json_key".to_string()) },
            )]),
        };
        let inputs = collect_provider_listing_inputs(&runtime, &[("p".to_string(), "P".to_string())]);
        assert_eq!(inputs.len(), 1);
        // 凭证损坏 → credential_type 缺失,但 auth status 仍在
        assert_eq!(inputs[0].credential_type, None);
        assert_eq!(inputs[0].status.configured, true);
    }
}
