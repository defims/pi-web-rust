# defims/picrab-web ↔ agegr/pi-web Alignment

> **English (default)** · 中文版: [pi-web-rust-mapping.zh-CN.md](./pi-web-rust-mapping.zh-CN.md)
>
> `defims/picrab-web` is the Rust version of [agegr/pi-web](https://github.com/agegr/pi-web),
> continuously tracking upstream.
>
> agegr/pi-web is the Next.js web UI for the pi engine (frontend + Node.js backend).
> defims/picrab-web's goal: **keep the frontend identical to upstream** (synced via sync)
> and **progressively rewrite the backend from TypeScript to Rust**, referencing
> `defims/picrab` as the engine.
>
> - **Alignment target**: agegr/pi-web's frontend (components/hooks/lib client code) + backend semantics (lib/ Node-only files + app/api/ routes)
> - **Aligned object**: defims/picrab-web (frontend = upstream original; backend = Rust rewrite in progress)
> - **Engine dependency**: defims/picrab (aligned with earendil-works/pi; see `docs/sdk-mapping.md` in the [defims/picrab](https://github.com/defims/picrab) repo)

## Core rules

- **Docs default to English**: the canonical copy is English (`X.md`); the Chinese version lives at `X.zh-CN.md`, cross-linked from the file header; update both copies together on any change.
- **Commits in English**: git commit messages in this repository are always written in English.

## Architecture comparison

| | agegr/pi-web (TS original) | defims/picrab-web (Rust version) |
|---|---|---|
| Frontend | Next.js + React (browser) | **same** (synced, byte-identical) |
| Backend | Next.js API routes (Node.js) | **Rust crate** (referencing pi_agent_rust) |
| Engine | @earendil-works/pi-coding-agent (TS SDK) | pi_agent_rust (Rust, aligned with the TS SDK) |
| Transport | HTTP fetch + EventSource | **in-process API layer (`api` feature, docs in consumer moho-mate's `docs/api-embed-plan.md`)** — `PiWebApi::handle(http::Request, Responder)` with host-injected asupersync runtime; host transports: wry async custom protocol (Wire B) + invoke forward bridge. SSE binding = event sink callbacks (no connection; frontend EventSource shim retains the wire shape). |
| Session storage | SessionManager (TS) | pi_agent_rust Session (Rust) |
| Model registry | ModelRuntime (TS) | pi_agent_rust ModelRegistry (Rust) |

## Frontend sync

Frontend code (components/hooks/lib client files) is **byte-identical** to upstream, synced
via `cargo xtask sync-pi-web` (the consumer moho-mate's xtask, run from its repo root):

```bash
# track an upstream release (this repository)
git fetch --tags && git checkout v0.9.0

# sync the frontend into the consumer (overwrites untouched files; override files only print a diff; run from the moho-mate root)
cargo xtask sync-pi-web --dry-run
cargo xtask sync-pi-web
```

### Override list (consumer moho-mate's `scripts/pi-web-overrides.txt`)

Frontend files the consumer has modified (not auto-overwritten during sync; only a diff is
printed for manual review):

| file | nature of change |
|---|---|
| `lib/agent-event-wire.ts` | shim-ified (no dependency on TS SDK union types) |

### Type shims (consumer moho-mate's `frontend/src/pi-shim/`)

The upstream frontend imports `@earendil-works/pi-coding-agent` / `@earendil-works/pi-agent-core`
(TS SDK types); the Rust version has no such packages. The consumer points vite/tsconfig
aliases at local shims:

| shim file | replaced TS package | exports |
|---|---|---|
| `pi-shim/pi-coding-agent.ts` | @earendil-works/pi-coding-agent | ResourceDiagnostic / AgentSessionEvent / JsonAgentSessionEvent / SessionManager / SettingsManager / SlashCommandInfo / Theme |
| `pi-shim/pi-agent-core.ts` | @earendil-works/pi-agent-core | AgentMessage |

See the consumer moho-mate's `docs/frontend-fork-audit.md`.

---

## Backend rewrite (TS → Rust)

The agegr/pi-web backend has three layers; the Rust rewrite advances layer by layer:

### Layer 1: `app/api/` route layer (40 route.ts → Rust handlers)

| upstream route | method | core logic | Rust rewrite status |
|---|---|---|---|
| `/api/agent/[id]` | POST | rpc-manager command dispatch (25 cases) | ✅ `src/api/session_runtime.rs` (prompt/steer/follow_up/abort/clear_queue/set_model/set_thinking_level/set_session_name/compact/set_tools/get_stats/get_last_text; fork/navigate_tree/reload/extension_* await host wiring) |
| `/api/agent/[id]/events` | GET | SSE event stream | ✅ **event-sink binding** (`src/api/events.rs` ApiEvent; engine on_event → `to_client_event` wire filter → EventSink; synthesized prompt_done/prompt_error/agent_settled/queue_update/compaction_*) |
| `/api/agent/new` | POST | create session | ✅ `src/api/session_runtime.rs` (capacity + close-old-open-new) |
| `/api/sessions` | GET | listAllSessions | ✅ `src/api/commands.rs` (lib chain + runningSessionIds) |
| `/api/sessions/[id]` | GET/PATCH/DELETE | session CRUD | ✅ get/context via `src/api/sessions.rs`; rename/delete via host proxy (route-level file ops, moho parity) |
| `/api/sessions/[id]/context` | GET | buildSessionContext | ✅ `src/api/sessions.rs` (lib engine chain) |
| `/api/sessions/[id]/state` | GET | running check + state | ✅ `agent_get_state` alias route (da9b9ea; useAgentSession loadSession consumer) |
| `/api/sessions/[id]/export` | GET | HTML export | ✅ **full port** of upstream export-html (9749436): vendored 0.84.1 template assets under `src/api/export_assets/` + server-side port in `src/api/export.rs` + deep-chain patch; byte-parity test against upstream-generated reference |
| `/api/models` | GET | model list + scopes | ✅ `src/api/models.rs` |
| `/api/models-config` | GET/PUT | models.json read/write | ✅ `src/api/models.rs` (lib models_config_store) |
| `/api/files/[...path]` | GET/POST | file read/write/upload | ✅ `src/api/files.rs` (eight states incl. multipart upload; size limits 25MB/100MB → 413) |
| `/api/git/status` `/api/git/diff` | GET | git operations | ✅ `src/api/commands.rs` (lib git::changes) |
| `/api/cwd/browse` | GET | directory browsing | ✅ `src/api/commands.rs` (lib directory_browser) |
| ... | | | (40 total; coverage locked by `routes.rs upstream_route_surface_covered` — every upstream route ∈ ROUTES ∪ documented exceptions: auth login/logout/api-key, skills install/update, worktrees, app-update) |

### Layer 2: `lib/` server logic (28 Node-only .ts → Rust modules)

| upstream file | lines | purpose | Rust rewrite status |
|---|---|---|---|
| `lib/rpc-manager.ts` | 1292 | agent session registry + RPC command dispatcher | ✅ session::rpc(纯逻辑)+ `src/api/session_runtime.rs`(AgentSessionWrapper 等价:注册表/邮箱/turn 循环/idle 清扫/扩展 UI 通道;356 测试) |
| `lib/session-reader.ts` | 350 | jsonl session reading + cache | ✅ reader.rs + entries.rs + `build_session_context_from_json`(经引擎 pi::sdk::build_session_context);SessionManager 侧 = `list_all_sessions`(引擎 SessionIndex 增量刷新) |
| `lib/rpc-manager.ts` (pure logic) | — | tool merging / cwd normalization / event types / start counters | ✅ session::rpc |
| `lib/file-fuzzy.ts` | 189 | @ file autocomplete | ✅ file::fuzzy |
| `lib/file-dirent.ts` | 17 | Dirent detection falling back to statSync | ✅ fs::file_dirent |
| `lib/provider-listing-runtime.ts` | 37 | ModelRuntime adapter layer | ✅ models::provider_listing_runtime |
| `lib/session-title.ts` | 253 | LLM-generated session titles | ✅ session::title |
| `lib/worktree.ts` | 230 | git worktree management | ✅ git::worktree |
| `lib/git-changes.ts` | 232 | git status/diff | ✅ git::changes |
| `lib/file-access.ts` | 75 | file-access allowlist | ✅ fs::file_access |
| `lib/file-upload.ts` | 59 | upload conflict strategy | ✅ fs::file_upload |
| `lib/model-scope.ts` | 142 | model scope filtering | ✅ models::scope |
| `lib/models-cache.ts` | 84 | model list cache | ✅ models::cache |
| `lib/model-catalog.ts` | 404 | models.dev catalog | ✅ models::catalog |
| `lib/model-discovery-auth.ts` | 58 | model discovery auth | ✅ models::discovery_auth |
| `lib/provider-credential-store.ts` | 114 | provider credential storage | ✅ models::credential_store |
| `lib/provider-listing.ts` | 118 | provider listing construction | ✅ models::provider_listing |
| `lib/project-trust.ts` | 48 | project trust state | 🟡 stub (always trusted, awaiting SDK completion) |
| `lib/skills-service.ts` | 16 | skills loading | ✅ skills::skills_service |
| `lib/skill-lock.ts` | 146 | skills.lock.json | ✅ skills::skill_lock |
| `lib/skill-updates.ts` | 263 | skills update detection | ✅ skills::skill_updates |
| `lib/npx.ts` | 62 | npx invocation | ✅ skills::npx |
| `lib/atomic-file.ts` | 34 | atomic file writes | ✅ fs::atomic_file |
| `lib/directory-browser.ts` | 83 | directory browsing | ✅ fs::directory_browser |
| `lib/bash-output.ts` | 53 | bash output reading | ✅ fs::bash_output |
| `lib/bounded-form-data.ts` | 49 | upload size limits | ✅ http::bounded_form_data |
| `lib/path-security.ts` | 42 | path-traversal protection | ✅ fs::path_security |
| `lib/allowed-roots.ts` | 24 | runtime allowlist roots | ✅ fs::allowed_roots |
| `lib/session-path.ts` | 11 | path → cache key | ✅ session::path |
| `lib/session-file-references.ts` | 29 | file-referenced-by-session check | ✅ security::session_references |
| `lib/http-dispatcher.ts` | 86 | global HTTP dispatcher | 🟡 pure logic (undici transport layer awaits a host) |
| `lib/web-auth.ts` | 45 | HTTP Basic auth | ✅ auth |
| `lib/startup-preferences.ts` | 55 | startup preference persistence | ✅ settings::startup_preferences |
| `lib/request-security.ts` | 118 | SSRF/CSRF protection | ✅ security::request_security |
| `lib/streaming-message.ts` | 115 | streaming message reducer | ✅ ui::streaming_message |
| `lib/normalize.ts` | 29 | toolCall field normalization | ✅ ui::streaming_message |
| `lib/session-file-references-core.ts` | 87 | file reference check core | ✅ security |

**Estimated rewrite volume**: ~4400 lines TS → ~5000-6000 lines Rust. 

> ✅ lib/ modules ported —— 引擎绑定层(AgentSessionWrapper / SessionManager)已于
> 2026-08 落地(`src/api/session_runtime.rs` + 引擎 SessionIndex);project-trust 仍为
> 恒信任 stub(无信任门需求即当前语义)。
> ✅ `app/api/` 路由层 = `src/api/`(50+ 命令全路由:agent RPC 25 case/sessions
> 含 auto-name LLM 生成/files 八态/models-config discover+test/skills 真实现/
> plugins 列表+五动作/cwd),由 moho-mate 经 Wire B 直连消费。
> `lib/markdown.ts` 等前端渲染件走 frontend sync(与上游字节一致),不重写。
> 测试:cargo(api)356 + 前端(node --test)444。

### Layer 3: engine references (the parts of lib/ importing @earendil-works/*)

Parts of the server logic calling the TS SDK are rewritten to reference `defims/picrab`.
Cross-check the TS SDK ↔ Rust method mapping in `docs/sdk-mapping.md` of the defims/picrab repo.

| TS SDK call | Rust equivalent (pi_agent_rust) |
|---|---|
| `createAgentSessionFromServices` | `create_agent_session(SessionOptions)` |
| `inner.prompt(msg)` | `handle.prompt_with_abort(msg, signal, on_event)` |
| `inner.executeBash(cmd)` | `handle.bash(cmd, abort_rx)` |
| `inner.getSessionStats()` | `handle.get_session_stats()` |
| `inner.compact(instructions?)` | `handle.compact_with_instructions(...)` |
| `SessionManager.open/create/listAll` | `pi::session::Session::open` / `SessionIndex::list_sessions` |
| `ModelRuntime.create/getAuth` | `pi::models::ModelRegistry::load` / `pi::auth::AuthStorage` |
| (full mapping in defims/picrab's docs/sdk-mapping.md) | |

---

## The consumer moho-mate's role

moho-mate is the **first consumer** of pi-web-rust. It currently implements its own backend
(~6400 lines of Rust across `src/session_scanner.rs` / `files_handler.rs` / `models_handler.rs`
etc.), to be replaced as pi-web-rust matures.

| moho-mate file | lines | pi-web-rust rewrite target |
|---|---|---|
| `session_scanner.rs` | 1719 | Rust version of session-reader.ts |
| `files_handler.rs` | 1790 | Rust version of file-upload/git-changes/directory-browser |
| `models_handler.rs` | 1968 | Rust version of model-scope/models-cache/model-catalog |
| `ipc_handlers.rs` | 1895 | Rust version of the app/api/ route layer |
| `chat_thread.rs` | 1280 | Rust version of rpc-manager.ts |
| `ipc_security.rs` | 341 | Rust version of path-security/allowed-roots |

---

## Upstream tracking flow (run from the consumer moho-mate's root)

```bash
# 1. agegr/pi-web cuts a new release (inside this repo's submodule checkout)
cd pi-web-rust && git fetch --tags && git checkout v0.9.0

# 2. sync the frontend
cargo xtask sync-pi-web --dry-run
cargo xtask sync-pi-web

# 3. inspect backend lib/ changes (affect the Rust rewrite)
git diff v0.8.7..v0.9.0 -- lib/ | grep "^[+-]" | grep -v "^[+-][+-][+-]"

# 4. inspect app/api/ changes (affect route handlers)
git diff v0.8.7..v0.9.0 -- app/api/

# 5. inspect TS SDK call changes (affect engine references; see defims/picrab's docs/sdk-mapping.md)
git diff v0.8.7..v0.9.0 -- lib/rpc-manager.ts | grep "inner\.\|SessionManager\.\|createAgent"

# 6. corresponding Rust rewrites + tests
# 7. push the pi-web-rust fork
git push origin main
```

## Related files

- Engine alignment: `docs/sdk-mapping.md` in defims/picrab (↔ earendil-works/pi)
- Consumer moho-mate docs: frontend audit `docs/frontend-fork-audit.md`, backend semantics audit `docs/port-gaps-audit.md` (semantic deviations of moho-mate's existing Rust code), probe notes `docs/agegr-probe-notes.md` / `docs/pi-sdk-probe-notes.md`
- Frontend sync script (consumer): moho-mate's `cargo xtask sync-pi-web [--dry-run] [--include-new]`
- 中文版: [pi-web-rust-mapping.zh-CN.md](./pi-web-rust-mapping.zh-CN.md)
