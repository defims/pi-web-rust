//! events — ApiEvent 定义与事件信封契约。
//!
//! 事件出口是宿主注入的 [`EventSink`](crate::api::EventSink) 回调(经
//! `ApiConfig` 传入),crate 侧每次调用包 catch_unwind;宿主实现必须非阻塞。
//!
//! ## 信封契约(对齐上游 parity,docs/api-embed-plan.md §二)
//!
//! - 事件经 sink 以本枚举(serde)交付,宿主序列化后按前端期待的事件名推送
//!   (`agent_event` / `queue_update` / …,信封由宿主侧 JS 桥消费)
//! - **顺序仅单源有序**:同一会话任务内 FIFO;跨源(会话事件 vs 宿主注入)
//!   交错不保证,前端以 reconcile 容忍(挂载恢复 + 运行中轮询已存在)
//! - **订阅重放留位**:上游 `AgentSessionWrapper.onEvent` 订阅时重放
//!   `pendingUiRequests`(晚连接不漏 extension UI 请求);extension 面启用时
//!   需在会话运行时补齐同语义 —— 见上游 rpc-manager.ts onEvent
//! - 无 Critical/Volatile 分级(通道已删,SSE 壳出现时再定义;消费端容忍)

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 会话运行时与宿主注入的事件统一出口。
///
/// P1 仅含宿主注入通道与测试面;P3 扩充会话事件(Agent 透传/合成事件,
/// 对齐上游 `toClientAgentEvent` 服务端过滤语义)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiEvent {
    /// 宿主侧事件注入(moho_action 进度 / 日志上传回执等 legacy 链路的归宿)。
    /// `event` 为宿主自定义事件名,`payload` 透传。
    Host { event: String, payload: Value },
    /// 引擎/合成事件(agent_event 通道)。`payload` 为已过滤的 wire 形状
    /// (toClientAgentEvent 语义;前端 MohoEventSource 消费,connected 首帧
    /// 由前端 shim 合成 —— 事件线无连接概念,对齐现状机制)。
    Agent { payload: Value },
}
