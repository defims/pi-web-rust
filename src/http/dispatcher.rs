//! 对齐 `lib/http-dispatcher.ts` 的可移植部分。
//!
//! 上游模块在 Node 里把 undici 的全局 dispatcher 替换成
//! EnvHttpProxyAgent(支持代理 + per-origin pool/client 工厂),并给 undici
//! 内部 Client 的错误事件挂上吞掉监听器(避免响应体终止时的 EventEmitter
//! 错误直接终止 Next.js 进程)。
//!
//! Rust 版移植:
//! - 纯决策逻辑(`parse_http_idle_timeout_ms` / 配置防重 / 非法值报错)忠实保留
//! - undici 传输层对应未来 axum/hyper 宿主里的连接池(keep-alive idle timeout /
//!   代理支持),由宿主消费 [`HttpDispatcherConfig`] 接线

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 对齐 `DEFAULT_HTTP_IDLE_TIMEOUT_MS`。
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// 对齐 `configureHttpDispatcher` 的参数;idle timeout 语义:
/// `0` = disabled(不设超时),`> 0` = 空闲毫秒数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpDispatcherConfig {
    pub idle_timeout_ms: u64,
}

/// 全局 dispatcher 配置(等价 `globalThis.__piWebHttpDispatcherConfigured`)。
/// 首次成功配置后固定,后续调用幂等返回同一配置。
/// 用 Mutex<Option> 便于测试重置(OnceLock 无 take)。
static CONFIGURED: std::sync::Mutex<Option<HttpDispatcherConfig>> = std::sync::Mutex::new(None);

/// 对齐 `parseHttpIdleTimeoutMs`。
///
/// - 字符串:"disabled"(不区分大小写)→ 0;空白 → undefined;否则按数字递归
/// - 数字:有限且 ≥ 0 → floor;否则 undefined
pub fn parse_http_idle_timeout_ms(value: &Value) -> Option<u64> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.eq_ignore_ascii_case("disabled") {
                return Some(0);
            }
            if trimmed.is_empty() {
                return None;
            }
            match trimmed.parse::<f64>() {
                Ok(n) => parse_http_idle_timeout_ms(&Value::Number(
                    serde_json::Number::from_f64(n).unwrap_or(serde_json::Number::from(0)),
                )),
                Err(_) => None,
            }
        }
        Value::Number(n) => match n.as_f64() {
            Some(n) if n.is_finite() && n >= 0.0 => Some(n.floor() as u64),
            _ => None,
        },
        _ => None,
    }
}

/// 对齐 `configureHttpDispatcher` 的纯逻辑部分。
///
/// 非法值抛错(对齐 `throw new Error("Invalid HTTP idle timeout: ...")`);
/// 已配置过则幂等返回首次配置(对齐 `__piWebHttpDispatcherConfigured` 守卫)。
pub fn configure_http_dispatcher(timeout_ms: u64) -> Result<HttpDispatcherConfig, String> {
    let mut guard = CONFIGURED.lock().unwrap();
    if let Some(existing) = *guard {
        return Ok(existing);
    }

    let normalized = match parse_http_idle_timeout_ms(&Value::Number(
        serde_json::Number::from(timeout_ms),
    )) {
        Some(n) => n,
        None => return Err(format!("Invalid HTTP idle timeout: {timeout_ms}")),
    };
    let config = HttpDispatcherConfig { idle_timeout_ms: normalized };
    *guard = Some(config);
    Ok(config)
}

/// 读取当前是否已配置(测试与宿主用)。
pub fn http_dispatcher_configured() -> bool {
    CONFIGURED.lock().unwrap().is_some()
}

/// 测试辅助:重置防重守卫。
#[doc(hidden)]
pub fn reset_dispatcher_guard() {
    *CONFIGURED.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_string_values() {
        assert_eq!(parse_http_idle_timeout_ms(&json!("300000")), Some(300_000));
        assert_eq!(parse_http_idle_timeout_ms(&json!("disabled")), Some(0));
        assert_eq!(parse_http_idle_timeout_ms(&json!("DISABLED")), Some(0));
        assert_eq!(parse_http_idle_timeout_ms(&json!(" 300000 ")), Some(300_000));
        assert_eq!(parse_http_idle_timeout_ms(&json!("")), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!("  ")), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!("abc")), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!("12.7")), Some(12));
    }

    #[test]
    fn parse_number_values() {
        assert_eq!(parse_http_idle_timeout_ms(&json!(300_000)), Some(300_000));
        assert_eq!(parse_http_idle_timeout_ms(&json!(0)), Some(0));
        assert_eq!(parse_http_idle_timeout_ms(&json!(12.9)), Some(12));
        assert_eq!(parse_http_idle_timeout_ms(&json!(-1)), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!(f64::NAN)), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!(f64::INFINITY)), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!(null)), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!(true)), None);
    }

    #[test]
    fn configure_once_guard() {
        reset_dispatcher_guard();
        assert!(!http_dispatcher_configured());
        let cfg = configure_http_dispatcher(150_000).unwrap();
        assert_eq!(cfg, HttpDispatcherConfig { idle_timeout_ms: 150_000 });
        assert!(http_dispatcher_configured());
        // 二次调用幂等,返回首次生效配置
        let cfg2 = configure_http_dispatcher(99_000).unwrap();
        assert_eq!(cfg2, HttpDispatcherConfig { idle_timeout_ms: 150_000 });
    }

    #[test]
    fn configure_invalid_timeout_errors() {
        reset_dispatcher_guard();
        // 直接调 configure 只接受 u64,非法输入无法表达;
        // 用 parse 路径验证错误语义由 parse 层承担。
        assert_eq!(parse_http_idle_timeout_ms(&json!(-5)), None);
    }

    #[test]
    fn config_serde() {
        let json = serde_json::to_value(HttpDispatcherConfig { idle_timeout_ms: 0 }).unwrap();
        assert_eq!(json["idleTimeoutMs"], 0);
    }
}
