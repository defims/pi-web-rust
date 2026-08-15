//! commands — ApiCore 命令处理器(路由表的执行面)。
//!
//! P1 骨架:命令分发框架 + 首个命令(home);P2 按
//! docs/api-embed-plan.md §六 填充首批命令(sessions/models/files/git/cwd),
//! 处理器内部调用 lib 模块(session/fs/git/models)与引擎(trait 注入)。

use serde_json::json;

use super::routes::Dispatch;
use super::ApiError;

/// 命令执行结果统一为 http::Response(传输方言直通;JSON/字节由命令自定)。
pub(crate) async fn execute(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    match dispatch.command {
        "home" => home(dispatch).await,
        #[cfg(test)]
        "test_sleep" => test_sleep(dispatch).await,
        #[cfg(test)]
        "test_panic" => panic!("test_panic: intentional"),
        #[cfg(test)]
        "test_bytes" => test_bytes(dispatch).await,
        other => Err(ApiError::not_found(format!("unknown command: {other}"))),
    }
}

/// GET /api/home —— 返回用户主目录(对齐上游 app/api/home/route.ts)。
async fn home(_dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    json_response(json!({ "home": home }))
}

// ── helpers ─────────────────────────────────────────────────────────────

pub(crate) fn json_response(body: serde_json::Value) -> Result<http::Response<Vec<u8>>, ApiError> {
    let body = serde_json::to_vec(&body)
        .map_err(|e| ApiError::internal(format!("serialize response: {e}")))?;
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("static response builder"))
}

// ── 测试专用命令(exactly-once / 超时 / panic 路径的测试靶) ─────────────

#[cfg(test)]
async fn test_sleep(dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let ms = dispatch
        .args
        .get("ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000);
    asupersync::time::sleep(asupersync::time::wall_now(), std::time::Duration::from_millis(ms)).await;
    json_response(json!({ "slept": ms }))
}

#[cfg(test)]
async fn test_bytes(_dispatch: Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, "application/octet-stream")
        .body(vec![0u8, 1, 2, 250, 255])
        .expect("static response builder"))
}
