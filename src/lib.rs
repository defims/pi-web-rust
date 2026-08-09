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
//! | `lib/compaction-summary.ts` | `compaction` | 🟡 退化(body-only,regex 待补) |
//! | (其余 lib/ 文件) | — | ❌ 待改写 |

pub mod auth;
pub mod compaction;
pub mod diff;
pub mod file;
pub mod fs;
pub mod git;
pub mod image;
pub mod models;
pub mod security;
pub mod session;
pub mod tools;
pub mod ui;
