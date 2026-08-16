//! file_index — GET /api/file-index?cwd=&q=(@ 文件补全的索引面)。
//!
//! 索引:git ls-files 优先(尊重 .gitignore,经 lib find_repository_root),
//! 失败回退 walk(深度 ≤8、IGNORED_NAMES、MAX_FILES=5000、硬上限 50_000)。
//! 过滤/排序:lib `file::fuzzy`(score/depth/locale,上游 parity —— 替代
//! moho 旧手写 score_file_entry)。
//! 缓存:本批未做(旧实现 10s/20 项;优化项,见口径文档)。

use serde_json::{json, Value};
use std::path::Path;

use super::commands::ExecCtx;
use super::routes::Dispatch;
use super::ApiError;

const MAX_FILES: usize = 5000;
const MAX_WALK_DEPTH: usize = 8;
const WALK_HARD_CAP: usize = 50_000;
const MAX_MATCHES: usize = 20;

pub(crate) async fn file_index_command(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let raw_cwd = dispatch
        .args
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    // 空 cwd(客户端无会话)→ 空集(兼容前端)
    if raw_cwd.is_empty() {
        let has_q = dispatch.args.get("q").and_then(|v| v.as_str()).is_some();
        return super::commands::json_response(if has_q {
            json!({ "matches": [] })
        } else {
            json!({ "files": [], "truncated": false })
        });
    }
    let cwd = super::files::resolve_path_pub(&raw_cwd);
    if !cwd.is_absolute() {
        return Err(ApiError::new(400, "cwd must be an absolute path"));
    }
    if !cwd.exists() {
        return Err(ApiError::not_found(format!("directory does not exist: {}", cwd.display())));
    }
    if !cwd.is_dir() {
        return Err(ApiError::new(400, format!("not a directory: {}", cwd.display())));
    }
    super::commands::gate_roots(ctx, &cwd.to_string_lossy()).await?;

    let q_raw = dispatch.args.get("q").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (files, truncated) =
        super::commands::blocking(ctx, move || index_files(&cwd)).await?;

    if q_raw.is_empty() {
        let files_json: Vec<String> = files.into_iter().map(|(rel, _)| rel).collect();
        return super::commands::json_response(json!({ "files": files_json, "truncated": truncated }));
    }

    // lib fuzzy:entries 构建 + 评分过滤(上限 20)
    let entries = crate::file::fuzzy::build_entries_from_files(
        &files.iter().map(|(rel, _)| rel.clone()).collect::<Vec<_>>(),
    );
    let matches: Vec<Value> = crate::file::fuzzy::filter_file_entries(&entries, &q_raw, MAX_MATCHES)
        .iter()
        .map(|e| json!({ "path": e.path, "isDir": e.is_dir }))
        .collect();
    super::commands::json_response(json!({ "matches": matches }))
}

/// 索引:git ls-files 优先,失败回退 walk。
fn index_files(cwd: &Path) -> (Vec<(String, bool)>, bool) {
    if let Some(root) = futures::executor::block_on(crate::git::changes::find_repository_root(
        &cwd.to_string_lossy(),
    )) {
        if let Some(out) = run_git_ls_files(&root) {
            let mut files: Vec<(String, bool)> =
                out.split('\0').filter(|p| !p.is_empty()).map(|p| (p.to_string(), false)).collect();
            files.sort();
            let truncated = files.len() > MAX_FILES;
            files.truncate(MAX_FILES);
            return (files, truncated);
        }
    }
    walk_index(cwd)
}

fn run_git_ls_files(root: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn walk_index(root: &Path) -> (Vec<(String, bool)>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut visited: usize = 0;
    walk_dir(root, root, 0, &mut out, &mut truncated, &mut visited);
    (out, truncated)
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<(String, bool)>,
    truncated: &mut bool,
    visited: &mut usize,
) {
    if *truncated || *visited >= WALK_HARD_CAP || depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        if *truncated || *visited >= WALK_HARD_CAP {
            return;
        }
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if super::files::IGNORED_NAMES_PUB.contains(&name.as_str()) || name.ends_with(".pyc") {
            continue;
        }
        *visited += 1;
        let Ok(meta) = entry.metadata() else { continue };
        let rel = entry
            .path()
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if meta.is_dir() {
            walk_dir(root, &entry.path(), depth + 1, out, truncated, visited);
        } else {
            if out.len() >= MAX_FILES {
                *truncated = true;
                return;
            }
            out.push((rel, false));
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct TmpRoot(std::path::PathBuf);
    impl HostHooks for TmpRoot {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(std::env::temp_dir().join(format!("no-sess-{:?}", self.0))
                .join("s"))
        }
    }

    fn api_for(tmp: &std::path::Path) -> PiWebApi {
        crate::fs::allowed_roots::allow_file_root(&tmp.to_string_lossy());
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(TmpRoot(tmp.to_path_buf()));
        PiWebApi::new(rt, cfg)
    }

    fn call(api: &PiWebApi, req: http::Request<Vec<u8>>) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(req, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    #[test]
    fn file_index_list_and_query() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/deep")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), b"fn m(){}").unwrap();
        std::fs::write(tmp.path().join("src/deep/util.rs"), b"x").unwrap();
        std::fs::write(tmp.path().join("readme.md"), b"r").unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
        std::fs::write(tmp.path().join("node_modules/pkg/x.js"), b"").unwrap();
        let api = api_for(tmp.path());
        let cwd = url_enc(tmp.path());

        // 无 q:文件列表(walk 兜底;node_modules 过滤;相对路径)
        let resp = call(&api, get(&format!("/api/file-index?cwd={cwd}"))).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let files: Vec<&str> = v["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
        assert_eq!(files, vec!["readme.md", "src/deep/util.rs", "src/main.rs"]);
        assert_eq!(v["truncated"], json!(false));

        // 有 q:lib fuzzy 过滤
        let resp = call(&api, get(&format!("/api/file-index?cwd={cwd}&q=util"))).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let m = v["matches"].as_array().unwrap();
        assert!(!m.is_empty());
        assert!(m.iter().any(|e| e["path"] == json!("src/deep/util.rs")));

        // 空 cwd → 空集
        let resp = call(&api, get("/api/file-index")).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["files"], json!([]));
        // 越权 → 403
        let e = call(&api, get("/api/file-index?cwd=/etc")).unwrap_err();
        assert_eq!(e.status, 403);
    }

    fn get(uri: &str) -> http::Request<Vec<u8>> {
        http::Request::builder().method("GET").uri(uri).body(Vec::new()).unwrap()
    }

    fn url_enc(p: &std::path::Path) -> String {
        p.to_string_lossy()
            .split('/')
            .map(|s| url_encode(s))
            .collect::<Vec<_>>()
            .join("/")
    }

    fn url_encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
}
