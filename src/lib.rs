//! pi-web-rust — agegr/pi-web 后端的 Rust 版本。
//!
//! 前端与上游逐字节一致(React + Next.js UI,经外部 sync 同步);
//! 后端从 TypeScript(Next.js API routes + Node.js lib/)逐步改写为 Rust,
//! 引用 `pi_agent_rust`(defims fork,对齐 earendil-works/pi TS SDK)作为引擎。
//!
//! 对齐基准:agegr/pi-web `lib/` Node-only 文件 + `app/api/` 路由语义。
//! 对齐进度见 `docs/pi-web-rust-mapping.md`。
//!
//! ## 模块映射(上游 lib/ → Rust src/)
//!
//! | 上游 TS | Rust 模块 | 状态 |
//! |---|---|---|
//! | `lib/git-types.ts` | `git::types` | ✅ |
//! | `lib/git-status.ts` | `git::status` | ✅ |
//! | `lib/file-paths.ts` | `file::paths` | ✅ |
//! | `lib/file-types.ts` | `file::types` | ✅ |
//! | `lib/tool-presets.ts` | `tools` | ✅ |
//! | `lib/image-attachments.ts` | `image` | ✅ |
//! | `lib/patch.ts` | `diff` | ✅ |
//! | `lib/compaction-summary.ts` | `compaction` | ✅ |
//! | `lib/request-security.ts` | `security::request_security` | ✅ |
//! | `lib/model-catalog.ts` | `models::catalog` | ✅ |
//! | `lib/model-scope.ts` | `models::scope` | ✅ |
//! | `lib/models-cache.ts` | `models::cache` | ✅ |
//! | `lib/model-discovery-auth.ts` | `models::discovery_auth` | ✅ |
//! | `lib/bounded-form-data.ts` | `http::bounded_form_data` | ✅ |
//! | `lib/startup-preferences.ts` | `settings::startup_preferences` | ✅ |
//! | `lib/provider-credential-store.ts` | `models::credential_store` | ✅ |
//! | `lib/http-dispatcher.ts` | `http::dispatcher` | 🟡 纯逻辑(undici 传输层待宿主接线) |
//! | `lib/skill-lock.ts` | `skills::skill_lock` | ✅ |
//! | `lib/skills-service.ts` | `skills::skills_service` | ✅ |
//! | `lib/npx.ts` | `skills::npx` | ✅ |
//! | `lib/skill-updates.ts` | `skills::skill_updates` | ✅ |
//! | `lib/session-title.ts` | `session::title` | ✅ |
//! | `lib/streaming-message.ts` + `lib/normalize.ts` | `ui::streaming_message` | ✅ |
//! | `lib/session-file-references.ts` | `security::session_references` | ✅ |
//! | `lib/session-reader.ts`(纯计算部分) | `session::entries` | ✅ |
//! | `lib/rpc-manager.ts`(纯逻辑部分) | `session::rpc` | ✅ |
//! | `lib/file-fuzzy.ts` | `file::fuzzy` | ✅ |
//! | `lib/file-dirent.ts` | `fs::file_dirent` | ✅ |
//! | `lib/provider-listing-runtime.ts` | `models::provider_listing_runtime` | ✅ |
//! | `lib/app-update.ts` | `app_update` | ✅ |
//! | `lib/paths.ts` | `paths` | ✅ |
//! | `lib/project-tree.ts` | `project_tree` | ✅ |
//! | `lib/api-types.ts` | — | 🟡 纯类型(shared 请求/响应形状,待 serde 结构定义) |
//! | `lib/agent-event-stream.ts` | — | 🟡 SSE 绑定层,留给宿主(moho-mate chat_thread)接线 |
//! | (其余客户端 lib/ 文件) | — | ❌ 浏览器状态/纯前端,不属后端移植范围 |

pub mod app_update;
pub mod auth;
pub mod compaction;
pub mod diff;
pub mod file;
pub mod fs;
pub mod git;
pub mod http;
pub mod image;
pub mod models;
pub mod paths;
pub mod project_tree;
pub mod security;
pub mod session;
pub mod settings;
pub mod skills;
pub mod tools;
pub mod ui;

/// 嵌入契约层(feature = "api",见宿主仓库 docs/api-embed-plan.md)。
/// 纯逻辑层(lib/)之上的路由/命令/事件出口;默认关闭。
#[cfg(feature = "api")]
pub mod api;
