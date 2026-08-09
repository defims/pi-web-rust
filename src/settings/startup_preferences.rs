//! 对齐 `lib/startup-preferences.ts`。启动偏好的显式选择持久化。
//!
//! SettingsManager 操作(引擎侧)抽象为 [`SettingsOps`] trait 注入,
//! 本模块只做决策 + 按序调用 + flush(语义与 TS 逐条对齐)。

use serde::{Deserialize, Serialize};

use crate::models::ModelRef;

/// 对齐 `ExplicitStartupPreferences`(浏览器里的显式选择)。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplicitStartupPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

/// 对齐 `EffectiveStartupPreferences`(会话实际生效的值)。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveStartupPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    pub thinking_level: String,
    pub supports_thinking: bool,
}

/// 对齐 `{ modelDefaultChanged }` 返回值。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupPreferencesOutcome {
    pub model_default_changed: bool,
}

/// SettingsManager 的操作面(trait,供 pi_agent_rust 接线)。
pub trait SettingsOps {
    /// `setDefaultModelAndProvider(provider, modelId)`。
    fn set_default_model_and_provider(&mut self, provider: &str, model_id: &str);
    /// `setDefaultThinkingLevel(level)`。
    fn set_default_thinking_level(&mut self, level: &str);
    /// `flush()`。
    fn flush(&mut self);
}

/// 对齐 `persistExplicitStartupPreferences`。
///
/// - 显式选择为空 → 直接返回 `{ modelDefaultChanged: false }`(不 flush)
/// - 显式模型与生效模型一致 → 写默认模型/提供商
/// - 显式 thinking level 且(模型支持 thinking 或生效 level 非 "off")
///   → 写默认 thinking level
/// - 最后 flush,返回是否改过模型默认
pub fn persist_explicit_startup_preferences(
    settings: &mut dyn SettingsOps,
    explicit: &ExplicitStartupPreferences,
    effective: &EffectiveStartupPreferences,
) -> StartupPreferencesOutcome {
    if explicit.model.is_none() && explicit.thinking_level.is_none() {
        return StartupPreferencesOutcome { model_default_changed: false };
    }

    let mut model_default_changed = false;

    if let (Some(explicit_model), Some(effective_model)) = (&explicit.model, &effective.model) {
        if explicit_model.provider == effective_model.provider
            && explicit_model.model_id == effective_model.model_id
        {
            settings.set_default_model_and_provider(&effective_model.provider, &effective_model.model_id);
            model_default_changed = true;
        }
    }

    if let Some(_explicit_level) = &explicit.thinking_level {
        if effective.supports_thinking || effective.thinking_level != "off" {
            settings.set_default_thinking_level(&effective.thinking_level);
        }
    }

    settings.flush();
    StartupPreferencesOutcome { model_default_changed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSettings {
        calls: Vec<String>,
    }

    impl SettingsOps for RecordingSettings {
        fn set_default_model_and_provider(&mut self, provider: &str, model_id: &str) {
            self.calls.push(format!("setDefaultModelAndProvider({provider},{model_id})"));
        }
        fn set_default_thinking_level(&mut self, level: &str) {
            self.calls.push(format!("setDefaultThinkingLevel({level})"));
        }
        fn flush(&mut self) {
            self.calls.push("flush()".to_string());
        }
    }

    fn model(provider: &str, model_id: &str) -> ModelRef {
        ModelRef { provider: provider.to_string(), model_id: model_id.to_string() }
    }

    fn effective(model: Option<ModelRef>, thinking_level: &str, supports_thinking: bool) -> EffectiveStartupPreferences {
        EffectiveStartupPreferences {
            model,
            thinking_level: thinking_level.to_string(),
            supports_thinking,
        }
    }

    #[test]
    fn nothing_explicit_skips_flush() {
        let mut settings = RecordingSettings::default();
        let outcome = persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences::default(),
            &effective(None, "off", false),
        );
        assert_eq!(outcome, StartupPreferencesOutcome { model_default_changed: false });
        assert!(settings.calls.is_empty());
    }

    #[test]
    fn matching_model_persists_default() {
        let mut settings = RecordingSettings::default();
        let outcome = persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences { model: Some(model("p", "m1")), thinking_level: None },
            &effective(Some(model("p", "m1")), "off", false),
        );
        assert_eq!(outcome, StartupPreferencesOutcome { model_default_changed: true });
        assert_eq!(settings.calls, vec![
            "setDefaultModelAndProvider(p,m1)".to_string(),
            "flush()".to_string(),
        ]);
    }

    #[test]
    fn mismatched_model_skips_model_write() {
        let mut settings = RecordingSettings::default();
        let outcome = persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences { model: Some(model("p", "m2")), thinking_level: None },
            &effective(Some(model("p", "m1")), "off", false),
        );
        assert_eq!(outcome.model_default_changed, false);
        // 显式 model 但 mismatch → 不写模型,但仍 flush
        assert_eq!(settings.calls, vec!["flush()".to_string()]);
    }

    #[test]
    fn thinking_level_conditions() {
        // supportsThinking=true → 写
        let mut settings = RecordingSettings::default();
        persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences { model: None, thinking_level: Some("high".to_string()) },
            &effective(None, "high", true),
        );
        assert_eq!(settings.calls, vec![
            "setDefaultThinkingLevel(high)".to_string(),
            "flush()".to_string(),
        ]);

        // supportsThinking=false 且生效 level = "off" → 不写
        let mut settings = RecordingSettings::default();
        persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences { model: None, thinking_level: Some("high".to_string()) },
            &effective(None, "off", false),
        );
        assert_eq!(settings.calls, vec!["flush()".to_string()]);

        // supportsThinking=false 但生效 level ≠ "off" → 写
        let mut settings = RecordingSettings::default();
        persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences { model: None, thinking_level: Some("low".to_string()) },
            &effective(None, "low", false),
        );
        assert_eq!(settings.calls, vec![
            "setDefaultThinkingLevel(low)".to_string(),
            "flush()".to_string(),
        ]);
    }

    #[test]
    fn combined_path() {
        let mut settings = RecordingSettings::default();
        let outcome = persist_explicit_startup_preferences(
            &mut settings,
            &ExplicitStartupPreferences {
                model: Some(model("p", "m1")),
                thinking_level: Some("high".to_string()),
            },
            &effective(Some(model("p", "m1")), "high", true),
        );
        assert_eq!(outcome.model_default_changed, true);
        assert_eq!(settings.calls, vec![
            "setDefaultModelAndProvider(p,m1)".to_string(),
            "setDefaultThinkingLevel(high)".to_string(),
            "flush()".to_string(),
        ]);
    }

    #[test]
    fn serde_shapes() {
        let explicit = ExplicitStartupPreferences {
            model: Some(model("p", "m1")),
            thinking_level: Some("high".to_string()),
        };
        let json = serde_json::to_value(&explicit).unwrap();
        assert_eq!(json["model"]["provider"], "p");
        assert_eq!(json["model"]["modelId"], "m1");
        assert_eq!(json["thinkingLevel"], "high");

        let outcome = StartupPreferencesOutcome { model_default_changed: true };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["modelDefaultChanged"], true);
    }
}
