//! 对齐 `lib/model-scope.ts`。模型作用域解析 + 初始模型选择。
//!
//! `enabledModels` 设置沿用 pi 的 `--models` 语法(glob / 模糊匹配 + 可选
//! `:thinkingLevel` 后缀)。glob 匹配规则委托给引擎
//! (`resolve_model_scope_with_diagnostics` 等价语义),本模块只做编排与选择:
//! - `resolve_visible_models`:清洗 patterns → 调引擎解析 → 收集 thinking pins
//! - `select_initial_model_scope`:按 pi 启动规则挑选模型 + thinking level
//!
//! 引擎异步调用以注入回调形式接入(运行时无关)。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 模型引用(provider + modelId),对齐 `InitialModelScopeOptions` 里的
/// `{ provider, modelId }` 与 `Model` 上被使用的字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub provider: String,
}

impl Model {
    fn matches(&self, provider: &str, model_id: &str) -> bool {
        self.provider == provider && self.id == model_id
    }
}

/// 对齐 SDK 的 `ScopedModel`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedModel {
    pub model: Model,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

/// 对齐 `ModelScopeResult`。
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelScopeResult {
    /// UI 应展示的模型,按 resolver 顺序(无作用域限制时 = 全部可用)。
    pub visible: Vec<Model>,
    /// 保留给 AgentSession 模型循环与扩展的 SDK-native scope。
    pub scoped_models: Vec<ScopedModel>,
    /// `provider/modelId` → `:level` 后缀固定的 thinking level。
    pub thinking_level_pins: HashMap<String, String>,
    /// resolver 诊断(如 pattern 未匹配任何模型)。
    pub warnings: Vec<String>,
}

/// 对齐 `InitialModelScopeOptions`。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialModelScopeOptions {
    pub requested_model: Option<ModelRef>,
    pub default_model: Option<ModelRef>,
    pub thinking_level: Option<String>,
}

/// `{ provider, modelId }` 引用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

/// 对齐 `InitialModelScopeResult`。
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialModelScopeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    pub scoped_models: Vec<ScopedModel>,
}

/// `selectInitialModelScope` 抛出的错误(请求的模型不在启用的作用域内)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelScopeError(pub String);

/// 对齐 `resolveVisibleModels` 的决策逻辑(引擎调用注入)。
///
/// - `patterns` 清洗后为空 → 全部可用模型,无作用域、无警告
/// - 引擎解析结果为空(patterns 没匹配到任何模型)→ 回退到全部可用模型
/// - 否则:visible = scoped 模型,并按 `:level` 后缀收集 thinking pins
pub fn resolve_visible_models(
    patterns: &[String],
    get_available: impl FnOnce() -> Vec<Model>,
    resolve_scope: impl FnOnce(&[String]) -> (Vec<ScopedModel>, Vec<String>),
) -> ModelScopeResult {
    let cleaned: Vec<String> = patterns
        .iter()
        .map(|pattern| pattern.trim().to_string())
        .filter(|pattern| !pattern.is_empty())
        .collect();
    if cleaned.is_empty() {
        return ModelScopeResult {
            visible: get_available(),
            scoped_models: Vec::new(),
            thinking_level_pins: HashMap::new(),
            warnings: Vec::new(),
        };
    }

    let (scoped_models, diagnostics) = resolve_scope(&cleaned);
    let warnings = diagnostics;
    if scoped_models.is_empty() {
        return ModelScopeResult {
            visible: get_available(),
            scoped_models: Vec::new(),
            thinking_level_pins: HashMap::new(),
            warnings,
        };
    }

    // `anthropic/*:high` 把 thinking level 固定到 glob 命中的每个模型上。
    // 客户端预选哪个模型由调用方决定,这里全部上报。
    let thinking_level_pins: HashMap<String, String> = scoped_models
        .iter()
        .filter_map(|scoped| {
            scoped.thinking_level.as_ref().map(|level| {
                (
                    format!("{}/{}", scoped.model.provider, scoped.model.id),
                    level.clone(),
                )
            })
        })
        .collect();
    let visible = scoped_models
        .iter()
        .map(|scoped| scoped.model.clone())
        .collect();

    ModelScopeResult {
        visible,
        scoped_models,
        thinking_level_pins,
        warnings,
    }
}

/// 对齐 `selectInitialModelScope`。
///
/// 镜像 pi 的启动规则:优先显式选择,否则用作用域内的默认模型,再否则取
/// resolver 顺序的第一个模型。除非调用方显式给了 thinking level,否则应用
/// 被选 scoped 模型的 thinking pin。请求的模型不在作用域内时返回
/// [`ModelScopeError`](对应 TS 的 throw)。
pub fn select_initial_model_scope(
    scope: &ModelScopeResult,
    options: &InitialModelScopeOptions,
) -> Result<InitialModelScopeResult, ModelScopeError> {
    let requested_ref = options.requested_model.as_ref();
    let default_ref = options.default_model.as_ref();
    let requested = requested_ref.and_then(|ref_| {
        scope
            .visible
            .iter()
            .find(|model| model.matches(&ref_.provider, &ref_.model_id))
    });
    if let Some(ref_) = requested_ref {
        if requested.is_none() {
            return Err(ModelScopeError(format!(
                "Model is not available in the enabled scope: {}/{}",
                ref_.provider, ref_.model_id
            )));
        }
    }

    let requested_scoped = match requested {
        Some(model) => scope.scoped_models.iter().find(|scoped| {
            scoped.model == *model || scoped.model.matches(&model.provider, &model.id)
        }),
        None => None,
    };
    let default_scoped = if requested.is_none() {
        default_ref.and_then(|ref_| {
            scope
                .scoped_models
                .iter()
                .find(|scoped| scoped.model.matches(&ref_.provider, &ref_.model_id))
        })
    } else {
        None
    };
    let fallback_scoped = if requested.is_none() {
        default_scoped.or_else(|| scope.scoped_models.first())
    } else {
        None
    };
    let default_visible = if requested.is_none() && fallback_scoped.is_none() {
        default_ref.and_then(|ref_| {
            scope
                .visible
                .iter()
                .find(|model| model.matches(&ref_.provider, &ref_.model_id))
        })
    } else {
        None
    };

    let selected_model = requested
        .or(fallback_scoped.map(|s| &s.model))
        .or(default_visible);
    let scoped_selection = requested_scoped.or(fallback_scoped);
    let thinking_level = options
        .thinking_level
        .clone()
        .or_else(|| scoped_selection.and_then(|s| s.thinking_level.clone()));

    Ok(InitialModelScopeResult {
        model: selected_model.cloned(),
        thinking_level,
        scoped_models: scope.scoped_models.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, provider: &str, name: &str) -> Model {
        Model {
            id: id.to_string(),
            name: name.to_string(),
            provider: provider.to_string(),
        }
    }

    fn scoped(model: Model, thinking_level: Option<&str>) -> ScopedModel {
        ScopedModel {
            model,
            thinking_level: thinking_level.map(|s| s.to_string()),
        }
    }

    #[test]
    fn empty_patterns_returns_all_available() {
        let result = resolve_visible_models(
            &[],
            || vec![model("a", "p1", "A")],
            |_| unreachable!("resolver must not be called"),
        );
        assert_eq!(result.visible, vec![model("a", "p1", "A")]);
        assert!(result.scoped_models.is_empty());
        assert!(result.warnings.is_empty());
        assert!(result.thinking_level_pins.is_empty());
    }

    #[test]
    fn blank_patterns_filtered() {
        let result = resolve_visible_models(
            &["  ".to_string(), "".to_string()],
            || vec![model("a", "p1", "A")],
            |_| unreachable!("resolver must not be called"),
        );
        assert_eq!(result.visible.len(), 1);
    }

    #[test]
    fn empty_scope_falls_back_to_available() {
        let result = resolve_visible_models(
            &["p1/*".to_string()],
            || vec![model("a", "p1", "A"), model("b", "p2", "B")],
            |patterns| {
                assert_eq!(patterns, &["p1/*".to_string()]);
                (vec![], vec!["pattern matched nothing".to_string()])
            },
        );
        assert_eq!(result.visible.len(), 2);
        assert!(result.scoped_models.is_empty());
        assert_eq!(result.warnings, vec!["pattern matched nothing".to_string()]);
    }

    #[test]
    fn scoped_visible_and_pins() {
        let result = resolve_visible_models(
            &["anthropic/*:high".to_string()],
            || unreachable!("no fallback when scope non-empty"),
            |_| {
                (
                    vec![
                        scoped(model("claude-x", "anthropic", "Claude X"), Some("high")),
                        scoped(model("claude-y", "anthropic", "Claude Y"), None),
                    ],
                    vec![],
                )
            },
        );
        assert_eq!(result.visible.len(), 2);
        assert_eq!(result.scoped_models.len(), 2);
        assert_eq!(
            result
                .thinking_level_pins
                .get("anthropic/claude-x")
                .map(|s| s.as_str()),
            Some("high")
        );
        assert!(result
            .thinking_level_pins
            .get("anthropic/claude-y")
            .is_none());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn select_requested_model() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1"), model("m2", "p", "M2")],
            scoped_models: vec![scoped(model("m1", "p", "M1"), Some("high"))],
            thinking_level_pins: HashMap::new(),
            warnings: vec![],
        };
        let result = select_initial_model_scope(
            &scope,
            &InitialModelScopeOptions {
                requested_model: Some(ModelRef {
                    provider: "p".to_string(),
                    model_id: "m1".to_string(),
                }),
                default_model: None,
                thinking_level: None,
            },
        )
        .unwrap();
        assert_eq!(result.model.as_ref().map(|m| m.id.as_str()), Some("m1"));
        // 未显式给 level → 应用 scoped pin
        assert_eq!(result.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn select_requested_missing_errors() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1")],
            scoped_models: vec![],
            thinking_level_pins: HashMap::new(),
            warnings: vec![],
        };
        let err = select_initial_model_scope(
            &scope,
            &InitialModelScopeOptions {
                requested_model: Some(ModelRef {
                    provider: "p".to_string(),
                    model_id: "nope".to_string(),
                }),
                default_model: None,
                thinking_level: None,
            },
        )
        .unwrap_err();
        assert!(err.0.contains("nope"));
        // 无 requested 时不报错
        assert!(select_initial_model_scope(&scope, &InitialModelScopeOptions::default()).is_ok());
    }

    #[test]
    fn select_default_in_scope() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1"), model("m2", "p", "M2")],
            scoped_models: vec![scoped(model("m2", "p", "M2"), Some("medium"))],
            thinking_level_pins: HashMap::new(),
            warnings: vec![],
        };
        let result = select_initial_model_scope(
            &scope,
            &InitialModelScopeOptions {
                requested_model: None,
                default_model: Some(ModelRef {
                    provider: "p".to_string(),
                    model_id: "m2".to_string(),
                }),
                thinking_level: None,
            },
        )
        .unwrap();
        assert_eq!(result.model.as_ref().map(|m| m.id.as_str()), Some("m2"));
        assert_eq!(result.thinking_level.as_deref(), Some("medium"));
    }

    #[test]
    fn select_falls_back_to_first_scoped() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1"), model("m2", "p", "M2")],
            scoped_models: vec![scoped(model("m1", "p", "M1"), None)],
            thinking_level_pins: HashMap::new(),
            warnings: vec![],
        };
        let result =
            select_initial_model_scope(&scope, &InitialModelScopeOptions::default()).unwrap();
        assert_eq!(result.model.as_ref().map(|m| m.id.as_str()), Some("m1"));
        assert_eq!(result.thinking_level, None);
    }

    #[test]
    fn select_default_visible_when_no_scope() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1"), model("m2", "p", "M2")],
            scoped_models: vec![],
            thinking_level_pins: HashMap::new(),
            warnings: vec![],
        };
        let result = select_initial_model_scope(
            &scope,
            &InitialModelScopeOptions {
                requested_model: None,
                default_model: Some(ModelRef {
                    provider: "p".to_string(),
                    model_id: "m2".to_string(),
                }),
                thinking_level: None,
            },
        )
        .unwrap();
        assert_eq!(result.model.as_ref().map(|m| m.id.as_str()), Some("m2"));
        assert_eq!(result.thinking_level, None);
    }

    #[test]
    fn explicit_thinking_level_wins() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1")],
            scoped_models: vec![scoped(model("m1", "p", "M1"), Some("low"))],
            thinking_level_pins: HashMap::new(),
            warnings: vec![],
        };
        let result = select_initial_model_scope(
            &scope,
            &InitialModelScopeOptions {
                requested_model: Some(ModelRef {
                    provider: "p".to_string(),
                    model_id: "m1".to_string(),
                }),
                default_model: None,
                thinking_level: Some("high".to_string()),
            },
        )
        .unwrap();
        assert_eq!(result.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn serialize_shapes() {
        let scope = ModelScopeResult {
            visible: vec![model("m1", "p", "M1")],
            scoped_models: vec![scoped(model("m1", "p", "M1"), Some("high"))],
            thinking_level_pins: HashMap::from([("p/m1".to_string(), "high".to_string())]),
            warnings: vec!["w".to_string()],
        };
        let json = serde_json::to_value(&scope).unwrap();
        assert_eq!(json["visible"][0]["provider"], "p");
        assert_eq!(json["scopedModels"][0]["thinkingLevel"], "high");
        assert_eq!(json["thinkingLevelPins"]["p/m1"], "high");
        assert_eq!(json["warnings"][0], "w");

        let result = InitialModelScopeResult {
            model: None,
            thinking_level: None,
            scoped_models: vec![],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.get("model").is_none());
        assert!(json.get("thinkingLevel").is_none());
        assert_eq!(json["scopedModels"], serde_json::json!([]));
    }
}
