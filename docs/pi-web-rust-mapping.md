# defims/pi-web-rust ↔ agegr/pi-web 对齐

> `defims/pi-web-rust` 是 [agegr/pi-web](https://github.com/agegr/pi-web) 的 Rust 版本,
> 持续跟随上游。
>
> agegr/pi-web 是 pi 引擎的 Next.js Web UI(前端 + Node.js 后端)。
> defims/pi-web-rust 的目标:**前端保持与上游一致**(经 sync 同步),
> **后端逐步从 TypeScript 改写为 Rust**,引用 `defims/pi_agent_rust` 作为引擎。
>
> - **对齐基准**:agegr/pi-web 的前端(components/hooks/lib 客户端代码)+ 后端语义(lib/ Node-only 文件 + app/api/ 路由)
> - **被对齐对象**:defims/pi-web-rust(前端 = 上游原版;后端 = 待 Rust 改写)
> - **引擎依赖**:defims/pi_agent_rust(对齐 earendil-works/pi,见 [defims/pi_agent_rust](https://github.com/defims/pi_agent_rust) 仓库的 `docs/sdk-mapping.md`)

## 架构对比

| | agegr/pi-web(TS 原版) | defims/pi-web-rust(Rust 版) |
|---|---|---|
| 前端 | Next.js + React(浏览器) | **相同**(sync 同步,逐字节一致) |
| 后端 | Next.js API routes(Node.js) | **Rust crate**(引用 pi_agent_rust) |
| 引擎 | @earendil-works/pi-coding-agent(TS SDK) | pi_agent_rust(Rust,对齐 TS SDK) |
| 通信 | HTTP fetch + EventSource | 待定(axum server / Tauri IPC / 其它) |
| 会话存储 | SessionManager(TS) | pi_agent_rust Session(Rust) |
| 模型注册 | ModelRuntime(TS) | pi_agent_rust ModelRegistry(Rust) |

## 前端同步

前端代码(components/hooks/lib 客户端文件)与上游**逐字节一致**,经
`cargo xtask sync-pi-web` 同步(消费方 moho-mate 的 xtask,在其仓库根运行):

```bash
# 追上游 release(本仓库)
git fetch --tags && git checkout v0.9.0

# 同步前端到消费方(覆盖未改动的文件,override 文件只输出 diff;在 moho-mate 根运行)
cargo xtask sync-pi-web --dry-run
cargo xtask sync-pi-web
```

### override 清单(消费方 moho-mate 的 `scripts/pi-web-overrides.txt`)

消费方 fork 改过的前端文件(同步时不自动覆盖,只输出 diff 供人工 review):

| 文件 | 改动性质 |
|---|---|
| `lib/agent-event-wire.ts` | shim 化(不依赖 TS SDK 联合类型) |

### 类型 shim(消费方 moho-mate 的 `frontend/src/pi-shim/`)

上游前端 import `@earendil-works/pi-coding-agent` / `@earendil-works/pi-agent-core`
(TS SDK 类型),Rust 版无这些包。消费方用 vite/tsconfig alias 指向本地 shim:

| shim 文件 | 替代的 TS 包 | 导出 |
|---|---|---|
| `pi-shim/pi-coding-agent.ts` | @earendil-works/pi-coding-agent | ResourceDiagnostic / AgentSessionEvent / JsonAgentSessionEvent / SessionManager / SettingsManager / SlashCommandInfo / Theme |
| `pi-shim/pi-agent-core.ts` | @earendil-works/pi-agent-core | AgentMessage |

详见消费方 moho-mate 的 `docs/frontend-fork-audit.md`。

---

## 后端改写(TS → Rust)

agegr/pi-web 后端分三层,Rust 改写逐层推进:

### 层 1:`app/api/` 路由层(39 个 route.ts → Rust handlers)

| 上游路由 | 方法 | 核心逻辑 | Rust 改写状态 |
|---|---|---|---|
| `/api/agent/[id]` | POST | rpc-manager 命令分发(25 case) | ❌ 待改写 |
| `/api/agent/[id]/events` | GET | SSE 事件流 | ❌ 待改写 |
| `/api/agent/new` | POST | 新建会话 | ❌ 待改写 |
| `/api/sessions` | GET | listAllSessions | ❌ 待改写 |
| `/api/sessions/[id]` | GET/PATCH/DELETE | 会话 CRUD | ❌ 待改写 |
| `/api/sessions/[id]/context` | GET | buildSessionContext | ❌ 待改写 |
| `/api/models` | GET | 模型列表 + 作用域 | ❌ 待改写 |
| `/api/models-config` | GET/PUT | models.json 读写 | ❌ 待改写 |
| `/api/files/[...path]` | GET/POST | 文件读写/上传 | ❌ 待改写 |
| `/api/git/status` `/api/git/diff` | GET | git 操作 | ❌ 待改写 |
| `/api/cwd/browse` | GET | 目录浏览 | ❌ 待改写 |
| ... | | | (共 39 个,完整清单见消费方 moho-mate 的 `docs/port-gaps-audit.md`) |

### 层 2:`lib/` 服务端逻辑(28 个 Node-only .ts → Rust 模块)

| 上游文件 | 行数 | 功能 | Rust 改写状态 |
|---|---|---|---|
| `lib/rpc-manager.ts` | 1292 | agent 会话注册表 + RPC 命令分发器 | 🟡 部分(纯逻辑 session::rpc;AgentSessionWrapper 引擎绑定待宿主) |
| `lib/session-reader.ts` | 350 | jsonl 会话读取 + 缓存 | 🟡 部分(reader.rs + entries.rs;SessionManager/buildSessionContext 的 SDK 选择待引擎) |
| `lib/rpc-manager.ts`(纯逻辑) | — | 工具合并/cwd 归一化/事件类型/起始计数 | ✅ session::rpc |
| `lib/file-fuzzy.ts` | 189 | @ 文件自动补全 | ✅ file::fuzzy |
| `lib/file-dirent.ts` | 17 | Dirent 判定回退 statSync | ✅ fs::file_dirent |
| `lib/provider-listing-runtime.ts` | 37 | ModelRuntime 适配层 | ✅ models::provider_listing_runtime |
| `lib/session-title.ts` | 253 | LLM 生成会话标题 | ✅ session::title |
| `lib/worktree.ts` | 230 | git worktree 管理 | ✅ git::worktree |
| `lib/git-changes.ts` | 232 | git status/diff | ✅ git::changes |
| `lib/file-access.ts` | 75 | 文件访问白名单 | ✅ fs::file_access |
| `lib/file-upload.ts` | 59 | 上传冲突策略 | ✅ fs::file_upload |
| `lib/model-scope.ts` | 142 | 模型作用域过滤 | ✅ models::scope |
| `lib/models-cache.ts` | 84 | 模型列表缓存 | ✅ models::cache |
| `lib/model-catalog.ts` | 404 | models.dev 目录 | ✅ models::catalog |
| `lib/model-discovery-auth.ts` | 58 | 模型发现认证 | ✅ models::discovery_auth |
| `lib/provider-credential-store.ts` | 114 | provider 凭证存储 | ✅ models::credential_store |
| `lib/provider-listing.ts` | 118 | provider 列表构造 | ✅ models::provider_listing |
| `lib/project-trust.ts` | 48 | 项目信任状态 | 🟡 桩(恒信任,待 SDK 补齐) |
| `lib/skills-service.ts` | 16 | 技能加载 | ✅ skills::skills_service |
| `lib/skill-lock.ts` | 146 | skills.lock.json | ✅ skills::skill_lock |
| `lib/skill-updates.ts` | 263 | 技能更新检测 | ✅ skills::skill_updates |
| `lib/npx.ts` | 62 | npx 调用 | ✅ skills::npx |
| `lib/atomic-file.ts` | 34 | 原子文件写入 | ✅ fs::atomic_file |
| `lib/directory-browser.ts` | 83 | 目录浏览 | ✅ fs::directory_browser |
| `lib/bash-output.ts` | 53 | bash 输出读取 | ✅ fs::bash_output |
| `lib/bounded-form-data.ts` | 49 | 上传大小限制 | ✅ http::bounded_form_data |
| `lib/path-security.ts` | 42 | 路径越界防护 | ✅ fs::path_security |
| `lib/allowed-roots.ts` | 24 | 运行时白名单根 | ✅ fs::allowed_roots |
| `lib/session-path.ts` | 11 | 路径→缓存键 | ✅ session::path |
| `lib/session-file-references.ts` | 29 | 文件被会话引用判定 | ✅ security::session_references |
| `lib/http-dispatcher.ts` | 86 | 全局 HTTP dispatcher | 🟡 纯逻辑(undici 传输层待宿主) |
| `lib/web-auth.ts` | 45 | HTTP Basic 鉴权 | ✅ auth |
| `lib/startup-preferences.ts` | 55 | 启动偏好持久化 | ✅ settings::startup_preferences |
| `lib/request-security.ts` | 118 | SSRF/CSRF 防护 | ✅ security::request_security |
| `lib/streaming-message.ts` | 115 | 流式消息 reducer | ✅ ui::streaming_message |
| `lib/normalize.ts` | 29 | toolCall 字段归一化 | ✅ ui::streaming_message |
| `lib/session-file-references-core.ts` | 87 | 文件引用判定核心 | ✅ security |

**改写总量估算**:~4400 行 TS → ~5000-6000 行 Rust。

> ✅ 已移植 34 个 lib/ 模块(其中 4 个 🟡 部分/桩,待 pi_agent_rust 引擎补齐);
> ❌ 剩余 `lib/rpc-manager.ts` 的 AgentSessionWrapper 引擎绑定层与
> `lib/session-reader.ts` 的 SessionManager 部分(均为引擎绑定),
> 以及全部 `app/api/` 路由层(宿主接线层);`lib/markdown.ts` 等为前端渲染辅助,
> 属前端同步范围(经 sync 保持与上游逐字节一致),不改写。

### 层 3:引擎引用(lib/ 里 import @earendil-works/* 的部分)

后端逻辑里调 TS SDK 的部分,改写为引用 `defims/pi_agent_rust`。
对照 defims/pi_agent_rust 仓库 `docs/sdk-mapping.md` 的 TS SDK ↔ Rust 方法映射。

| TS SDK 调用 | Rust 等价(pi_agent_rust) |
|---|---|
| `createAgentSessionFromServices` | `create_agent_session(SessionOptions)` |
| `inner.prompt(msg)` | `handle.prompt_with_abort(msg, signal, on_event)` |
| `inner.executeBash(cmd)` | `handle.bash(cmd, abort_rx)` |
| `inner.getSessionStats()` | `handle.get_session_stats()` |
| `inner.compact(instructions?)` | `handle.compact_with_instructions(...)` |
| `SessionManager.open/create/listAll` | `pi::session::Session::open` / `SessionIndex::list_sessions` |
| `ModelRuntime.create/getAuth` | `pi::models::ModelRegistry::load` / `pi::auth::AuthStorage` |
| (完整对照见 defims/pi_agent_rust 的 docs/sdk-mapping.md) | |

---

## 消费方 moho-mate 的角色

moho-mate 是 pi-web-rust 的**第一个消费者**。当前 moho-mate 自己实现了后端
(`src/session_scanner.rs` / `files_handler.rs` / `models_handler.rs` 等 ~6400 行 Rust),
pi-web-rust 成熟后这些将被替代。

| moho-mate 现有文件 | 行数 | 对应 pi-web-rust 改写目标 |
|---|---|---|
| `session_scanner.rs` | 1719 | session-reader.ts 的 Rust 版 |
| `files_handler.rs` | 1790 | file-upload/git-changes/directory-browser 的 Rust 版 |
| `models_handler.rs` | 1968 | model-scope/models-cache/model-catalog 的 Rust 版 |
| `ipc_handlers.rs` | 1895 | app/api/ 路由层的 Rust 版 |
| `chat_thread.rs` | 1280 | rpc-manager.ts 的 Rust 版 |
| `ipc_security.rs` | 341 | path-security/allowed-roots 的 Rust 版 |

---

## 追上游流程(在消费方 moho-mate 根目录运行)

```bash
# 1. agegr/pi-web 发新 release(本仓库 submodule 内)
cd pi-web-rust && git fetch --tags && git checkout v0.9.0

# 2. 同步前端
cargo xtask sync-pi-web --dry-run
cargo xtask sync-pi-web

# 3. 检查后端 lib/ 变更(影响 Rust 改写)
git diff v0.8.7..v0.9.0 -- lib/ | grep "^[+-]" | grep -v "^[+-][+-][+-]"

# 4. 检查 app/api/ 变更(影响路由 handler)
git diff v0.8.7..v0.9.0 -- app/api/

# 5. 检查 TS SDK 调用变更(影响引擎引用,见 defims/pi_agent_rust 的 docs/sdk-mapping.md)
git diff v0.8.7..v0.9.0 -- lib/rpc-manager.ts | grep "inner\.\|SessionManager\.\|createAgent"

# 6. Rust 侧对应改写 + 测试
# 7. push pi-web-rust fork
git push origin main
```

## 相关文件

- 引擎对齐:defims/pi_agent_rust 的 `docs/sdk-mapping.md`(↔ earendil-works/pi)
- 消费方 moho-mate 的文档:前端审计 `docs/frontend-fork-audit.md`、后端语义审计 `docs/port-gaps-audit.md`(moho-mate 现有 Rust 实现的语义偏差)、探针笔记 `docs/agegr-probe-notes.md` / `docs/pi-sdk-probe-notes.md`
- 前端同步脚本(消费方):moho-mate 的 `cargo xtask sync-pi-web [--dry-run] [--include-new]`
