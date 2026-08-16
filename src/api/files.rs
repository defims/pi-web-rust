//! files — /api/files/[...path] 八态命令(list/read/meta/preview/download/
//! upload-check/upload/watch)。
//!
//! 自 moho files_handler 下沉;访问控制换 lib allowed roots(gate_roots,
//! 含 session cwd 根 —— 等价旧 can_access 的 session 旁路)。
//! Wire B 语义:read 对文本返回 JSON,对二进制返回字节+mime(前端 fetch 与
//! 原生 `<img>` 加载落在同一路由);download 返回字节 + attachment 头。
//! watch:前端 EventSource 拦截保留(计划:file watch 本期不做),路由标 501。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::commands::ExecCtx;
use super::routes::Dispatch;
use super::ApiError;

const TEXT_PREVIEW_MAX_BYTES: usize = 256 * 1024;
const IGNORED_NAMES: &[&str] = &[
    "node_modules", ".git", ".next", "dist", "build", "__pycache__",
];

/// 命令入口:按 args.type 分发(缺省 list;八态提取器)。
pub(crate) async fn files_command(
    ctx: &ExecCtx,
    dispatch: Dispatch,
) -> Result<http::Response<Vec<u8>>, ApiError> {
    let ty = dispatch
        .args
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("list")
        .to_string();
    let raw_path = dispatch
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if raw_path.is_empty() {
        return Err(ApiError::new(400, "path is required"));
    }
    // 通配捕获丢绝对路径的前导斜杠(normalize/trim 所致)—— 前端恒传绝对路径,恢复之
    let raw_path = if raw_path.starts_with('/') { raw_path } else { format!("/{raw_path}") };
    let path = resolve_path(&raw_path);

    match ty.as_str() {
        "list" => {
            // list 不享 session 旁路(上游 route.ts:437-449),先 gate 再验存在
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            list(&path)
        }
        "read" => {
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            read(&path)
        }
        "meta" => {
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            meta(&path).and_then(super::commands::json_response)
        }
        "preview" => {
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            preview(&path)
        }
        "download" => {
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            download(&path)
        }
        "upload-check" => {
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            upload_check(&dispatch)
        }
        "upload" => {
            super::commands::gate_roots(ctx, &path.to_string_lossy()).await?;
            upload(&path, &dispatch)
        }
        "watch" => Err(ApiError::new(501, "file watch stream not implemented (frontend EventSource interception retained)")),
        other => Err(ApiError::new(400, format!("unknown type: {other}"))),
    }
}

// ── 各态实现(lib 根内 + 404/400 语义对齐 route.ts) ────────────────────

fn list(path: &Path) -> Result<http::Response<Vec<u8>>, ApiError> {
    if !path.exists() {
        return Err(ApiError::not_found(format!("path does not exist: {}", path.display())));
    }
    if !path.is_dir() {
        return Err(ApiError::new(400, format!("not a directory: {}", path.display())));
    }
    let mut entries: Vec<(String, bool, u64, String)> = Vec::new();
    let read = std::fs::read_dir(path).map_err(|e| ApiError::internal(format!("read_dir: {e}")))?;
    for entry in read.flatten() {
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if IGNORED_NAMES.contains(&name.as_str()) || name.ends_with(".pyc") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let modified = meta
            .modified()
            .ok()
            .map(|m| {
                let dt: chrono::DateTime<chrono::Utc> = m.into();
                dt.to_rfc3339()
            })
            .unwrap_or_default();
        entries.push((name, is_dir, size, modified));
    }
    // 目录优先,同级大小写不敏感 alpha(对齐上游 localeCompare 与 lib
    // directory_browser 同款;旧 moho 实现为 ASCII 字节序 —— 口径变化清单项)
    entries.sort_by(|a, b| match b.1.cmp(&a.1) {
        std::cmp::Ordering::Equal => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
        other => other,
    });
    let entries_json: Vec<Value> = entries
        .into_iter()
        .map(|(name, is_dir, size, modified)| {
            json!({ "name": name, "isDir": is_dir, "size": size, "modified": modified })
        })
        .collect();
    super::commands::json_response(json!({
        "entries": entries_json,
        "path": path.to_string_lossy(),
    }))
}

fn read(path: &Path) -> Result<http::Response<Vec<u8>>, ApiError> {
    if !path.exists() {
        return Err(ApiError::not_found(format!("file not found: {}", path.display())));
    }
    if !path.is_file() {
        return Err(ApiError::new(400, format!("not a file: {}", path.display())));
    }
    // 二进制类:直接字节 + mime(前端 <img>/<audio> 原生消费同一路由)
    if preview_kind(path) != "text" {
        let bytes = std::fs::read(path).map_err(|e| ApiError::internal(format!("read: {e}")))?;
        return Ok(bytes_response(bytes, &mime_for(path), None));
    }
    let size = std::fs::metadata(path).map_err(|e| ApiError::internal(format!("stat: {e}")))?.len();
    if size > TEXT_PREVIEW_MAX_BYTES as u64 {
        return Err(ApiError::new(413, format!("file too large for text preview ({size} > {TEXT_PREVIEW_MAX_BYTES} bytes)")));
    }
    let bytes = std::fs::read(path).map_err(|e| ApiError::internal(format!("read: {e}")))?;
    let content = String::from_utf8(bytes).map_err(|_| {
        ApiError::new(415, "file is not valid UTF-8 (kind=binary); use native byte load")
    })?;
    super::commands::json_response(json!({
        "content": content,
        "language": language_for_path(path),
        "size": size,
    }))
}

fn meta(path: &Path) -> Result<Value, ApiError> {
    if !path.exists() {
        return Err(ApiError::not_found(format!("file not found: {}", path.display())));
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "size": size,
        "language": language_for_path(path),
        "mime": mime_for(path),
        "previewKind": document_preview_kind(path),
    }))
}

fn preview(path: &Path) -> Result<http::Response<Vec<u8>>, ApiError> {
    if extension_of(path) != "docx" {
        return Err(ApiError::new(400, "preview only supported for .docx"));
    }
    // mammoth 为 Node 专属库(与 moho 旧实现同限制;前端有回退)
    Err(ApiError::new(501, "docx preview not supported (mammoth is a Node-only library)"))
}

fn download(path: &Path) -> Result<http::Response<Vec<u8>>, ApiError> {
    if !path.exists() {
        return Err(ApiError::not_found(format!("file not found: {}", path.display())));
    }
    if !path.is_file() {
        return Err(ApiError::new(400, format!("not a file: {}", path.display())));
    }
    let bytes = std::fs::read(path).map_err(|e| ApiError::internal(format!("read: {e}")))?;
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    Ok(bytes_response(
        bytes,
        &mime_for(path),
        Some(&format!("attachment; filename=\"{filename}\"")),
    ))
}

fn upload_check(dispatch: &Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    let names: Vec<String> = dispatch
        .args
        .get("fileNames")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|n| n.as_str().map(String::from)).collect())
        .unwrap_or_default();
    // lib 校验文件名(非法名 → 400)
    if let Some(err) = crate::fs::file_upload::validate_upload_file_names(&names) {
        return Err(ApiError::new(400, err));
    }
    // 恒 200 {conflicts, nonReplaceable}(客户端读 conflicts 弹窗;409 会直炸)
    Ok(json_ok(json!({ "conflicts": [], "nonReplaceable": [] })))
}

fn upload(dir: &Path, dispatch: &Dispatch) -> Result<http::Response<Vec<u8>>, ApiError> {
    if !dir.is_dir() {
        return Err(ApiError::new(400, format!("not a directory: {}", dir.display())));
    }
    let strategy = match dispatch.args.get("conflict").and_then(|v| v.as_str()).unwrap_or("error") {
        "overwrite" => Conflict::Overwrite,
        "skip" => Conflict::Skip,
        "error" => Conflict::Error,
        other => return Err(ApiError::new(400, format!("Invalid conflict strategy: {other}"))),
    };
    let content_type = dispatch
        .content_type
        .as_deref()
        .ok_or_else(|| ApiError::new(400, "Content-Type required for upload"))?;
    let parts = parse_multipart(content_type, &dispatch.body)
        .map_err(|e| ApiError::new(400, format!("multipart parse: {e}")))?;
    if parts.is_empty() {
        return Err(ApiError::new(400, "no file parts"));
    }
    // 冲突检测(已存在 → 按 conflict 策略;error → 409 带 conflicts 列表)
    let mut conflicts = Vec::new();
    let mut skip_count = 0u32;
    let mut written = Vec::new();
    for (filename, data) in &parts {
        let target = dir.join(filename);
        if target.exists() {
            match strategy {
                Conflict::Error => {
                    conflicts.push(filename.clone());
                    continue;
                }
                Conflict::Skip => {
                    skip_count += 1;
                    continue;
                }
                Conflict::Overwrite => {}
            }
        }
        atomic_write(&target, data)
            .map_err(|e| ApiError::internal(format!("write {}: {e}", target.display())))?;
        written.push(filename.clone());
    }
    if !conflicts.is_empty() {
        let body = json!({ "error": "conflict", "conflicts": conflicts, "nonReplaceable": [] });
        let body = serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?;
        return Ok(http::Response::builder()
            .status(409)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body)
            .expect("static builder"));
    }
    Ok(json_ok(json!({ "uploaded": written, "skipped": skip_count })))
}

enum Conflict {
    Error,
    Overwrite,
    Skip,
}

/// 临时文件 + rename 原子写。
fn atomic_write(target: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = target.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, target)
}

// ── multipart 解析(最小实现:boundary 分割 + 每段 Content-Disposition) ──

/// 解析 multipart/form-data,返回 [(filename, bytes)](仅文件段)。
/// Wire B 上传走 fetch + ArrayBuffer 手工组 multipart(P0 补测结论:
/// 浏览器序列化的 FormData 体不被 WKURLSchemeHandler 转发)。
fn parse_multipart(content_type: &str, body: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let boundary = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
        .ok_or("missing boundary")?
        .trim_matches('"')
        .to_string();
    let delim = format!("--{boundary}");
    let delim_b = delim.as_bytes();

    let mut parts = Vec::new();
    let mut pos = 0usize;
    // 定位首边界
    pos = find(body, delim_b, pos).ok_or("boundary not found")?;
    pos += delim_b.len();
    loop {
        // 边界后: "--" = 结束;否则 CRLF 后是段头
        if body[pos..].starts_with(b"--") {
            break;
        }
        if body[pos..].starts_with(b"\r\n") {
            pos += 2;
        }
        // 段头到空行
        let head_end = find(&body[pos..], b"\r\n\r\n", 0).ok_or("malformed part header")? + pos;
        let headers = String::from_utf8_lossy(&body[pos..head_end]).to_string();
        let body_start = head_end + 4;
        // 段体到下一边界(其前的 CRLF 属于边界)
        let next = find(&body[body_start..], delim_b, 0)
            .ok_or("unterminated part")?
            + body_start;
        let mut part_end = next;
        if part_end >= 2 && &body[part_end - 2..part_end] == b"\r\n" {
            part_end -= 2;
        }
        // 提取 filename(仅文件段;普通字段跳过)
        let filename = headers.lines().find_map(|h| {
            let h = h.to_ascii_lowercase();
            if h.starts_with("content-disposition:") && h.contains("filename=") {
                let orig = headers.lines().find(|l| l.to_ascii_lowercase().starts_with("content-disposition:"))?;
                let idx = orig.to_ascii_lowercase().find("filename=")? + "filename=".len();
                let v = orig[idx..].trim();
                Some(v.trim_matches('"').to_string())
            } else {
                None
            }
        });
        if let Some(name) = filename {
            if !name.is_empty() {
                parts.push((name, body[body_start..part_end].to_vec()));
            }
        }
        pos = next + delim_b.len();
    }
    Ok(parts)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

// ── 类型/语言助手(moho files_handler 下沉) ─────────────────────────────

fn resolve_path(raw: &str) -> PathBuf {
    let expanded = if raw == "~" {
        crate::paths::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        format!(
            "{}/{}",
            crate::paths::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default(),
            rest
        )
    } else {
        raw.to_string()
    };
    PathBuf::from(crate::paths::resolve(&expanded))
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

fn preview_kind(path: &Path) -> &'static str {
    let ext = extension_of(path);
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" | "avif" => "image",
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "opus" => "audio",
        "mp4" | "webm" | "mov" | "mkv" | "avi" => "video",
        "pdf" => "pdf",
        "docx" => "docx",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "archive",
        _ if is_text_extension(&ext) => "text",
        _ => "binary",
    }
}

fn is_text_extension(ext: &str) -> bool {
    if ext.is_empty() {
        return true;
    }
    !matches!(
        ext,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif"
            | "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "opus"
            | "mp4" | "webm" | "mov" | "mkv" | "avi"
            | "pdf"
            | "docx" | "doc"
            | "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar"
            | "class" | "jar" | "pyc" | "pyo" | "o" | "so" | "dll" | "dylib"
            | "exe" | "bin" | "wasm" | "ttf" | "otf" | "woff" | "woff2" | "eot"
    )
}

fn mime_for(path: &Path) -> String {
    match extension_of(path).as_str() {
        "png" => "image/png".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "bmp" => "image/bmp".into(),
        "ico" => "image/x-icon".into(),
        "svg" => "image/svg+xml".into(),
        "avif" => "image/avif".into(),
        "mp3" => "audio/mpeg".into(),
        "wav" => "audio/wav".into(),
        "ogg" => "audio/ogg".into(),
        "flac" => "audio/flac".into(),
        "m4a" | "aac" => "audio/mp4".into(),
        "opus" => "audio/opus".into(),
        "mp4" => "video/mp4".into(),
        "webm" => "video/webm".into(),
        "mov" => "video/quicktime".into(),
        "mkv" => "video/x-matroska".into(),
        "avi" => "video/x-msvideo".into(),
        "pdf" => "application/pdf".into(),
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
        _ => "text/plain".into(),
    }
}

fn document_preview_kind(path: &Path) -> Option<&'static str> {
    match extension_of(path).as_str() {
        "pdf" => Some("pdf"),
        "docx" => Some("docx"),
        _ => None,
    }
}

fn language_for_path(path: &Path) -> &'static str {
    let base = path
        .file_name()
        .map(|b| b.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if base == "dockerfile" || base.starts_with("dockerfile.") {
        return "dockerfile";
    }
    if base == ".env" || base.starts_with(".env.") {
        return "bash";
    }
    if base == "makefile" || base == "gnumakefile" {
        return "makefile";
    }
    language_for_extension(&extension_of(path))
}

fn language_for_extension(ext: &str) -> &'static str {
    match ext {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "rs" => "rust",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" => "c",
        "cpp" | "cc" | "cxx" => "cpp",
        "h" => "c",
        "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "json" | "jsonl" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "md" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" | "fish" => "bash",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "tf" | "hcl" => "hcl",
        "txt" => "text",
        _ => "text",
    }
}

fn bytes_response(bytes: Vec<u8>, mime: &str, disposition: Option<&str>) -> http::Response<Vec<u8>> {
    let mut builder = http::Response::builder()
        .header(http::header::CONTENT_TYPE, mime)
        .header(http::header::CACHE_CONTROL, "no-store");
    if let Some(d) = disposition {
        builder = builder.header(http::header::CONTENT_DISPOSITION, d);
    }
    builder.body(bytes).expect("static builder")
}

fn json_ok(v: Value) -> http::Response<Vec<u8>> {
    super::commands::json_response(v).expect("json response")
}

// ============================================================================
// 测试:八态覆盖(tempdir + 自播种 roots)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct TmpRoot(std::path::PathBuf);
    impl HostHooks for TmpRoot {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            // 放在被列目录之外(引擎 SessionIndex 刷新会创建根目录,落 tmp 内会混入列表)
            Some(std::env::temp_dir().join(format!("no-sessions-{}", self.0.to_string_lossy().hash_str())))
        }
    }

    trait HashStr {
        fn hash_str(&self) -> String;
    }
    impl HashStr for str {
        fn hash_str(&self) -> String {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.hash(&mut h);
            format!("{:016x}", h.finish())
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

    fn call(
        api: &PiWebApi,
        req: http::Request<Vec<u8>>,
    ) -> Result<http::Response<Vec<u8>>, crate::api::ApiError> {
        let (tx, rx) = std::sync::mpsc::channel();
        api.handle(
            req,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        rx.recv_timeout(std::time::Duration::from_secs(30)).expect("responder called")
    }

    fn get_url(path: &str, query: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("GET")
            .uri(format!("{path}?{query}"))
            .body(Vec::new())
            .unwrap()
    }

    #[test]
    fn files_list_shape_and_filter() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        std::fs::create_dir_all(tmp.path().join("Zdir")).unwrap();
        std::fs::create_dir_all(tmp.path().join("adir")).unwrap();
        std::fs::write(tmp.path().join("x.pyc"), b"x").unwrap();
        std::fs::write(tmp.path().join("note.txt"), b"hello").unwrap();
        let api = api_for(tmp.path());
        let resp = call(&api, get_url(&format!("/api/files{}", url_enc_path(tmp.path())), "type=list"))
            .expect("ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        let names: Vec<&str> =
            v["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["adir", "Zdir", "note.txt"]);
    }

    #[test]
    fn files_read_text_json_binary_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), b"fn main() {}").unwrap();
        std::fs::write(tmp.path().join("i.png"), vec![0x89u8, 0x50, 0x4E, 0x47]).unwrap();
        let api = api_for(tmp.path());
        let resp = call(
            &api,
            get_url(&format!("/api/files{}", url_enc_path(&tmp.path().join("a.rs"))), "type=read"),
        )
        .expect("ok");
        assert_eq!(resp.status(), 200);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["content"], json!("fn main() {}"));
        assert_eq!(v["language"], json!("rust"));
        let resp = call(
            &api,
            get_url(&format!("/api/files{}", url_enc_path(&tmp.path().join("i.png"))), "type=read"),
        )
        .expect("ok");
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers().get(http::header::CONTENT_TYPE).unwrap(), "image/png");
        assert_eq!(resp.body(), &[0x89u8, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn files_meta_download_watch() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("d.pdf"), vec![1u8, 2, 3]).unwrap();
        let api = api_for(tmp.path());
        let base = format!("/api/files{}", url_enc_path(&tmp.path().join("d.pdf")));
        let resp = call(&api, get_url(&base, "type=meta")).expect("ok");
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["size"], json!(3));
        assert_eq!(v["previewKind"], json!("pdf"));
        assert_eq!(v["mime"], json!("application/pdf"));
        let resp = call(&api, get_url(&base, "type=download")).expect("ok");
        assert_eq!(resp.status(), 200);
        assert!(resp
            .headers()
            .get(http::header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("attachment"));
        assert_eq!(resp.body(), &[1u8, 2, 3]);
        let e = call(&api, get_url(&base, "type=watch")).unwrap_err();
        assert_eq!(e.status, 501);
    }

    #[test]
    fn files_upload_multipart_conflict_modes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("old.txt"), b"old").unwrap();
        let api = api_for(tmp.path());
        let base = format!("/api/files{}", url_enc_path(tmp.path()));
        let body =
            multipart_body("----b42", &[("new.txt", b"new-content"), ("old.txt", b"overwritten")]);

        let resp = call(&api, upload_req(&base, "conflict=error", &body)).expect("ok");
        assert_eq!(resp.status(), 409);
        let v: Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["conflicts"], json!(["old.txt"]));
        assert_eq!(std::fs::read(tmp.path().join("old.txt")).unwrap(), b"old");

        let resp = call(&api, upload_req(&base, "conflict=overwrite", &body)).expect("ok");
        assert_eq!(resp.status(), 200);
        assert_eq!(std::fs::read(tmp.path().join("new.txt")).unwrap(), b"new-content");
        assert_eq!(std::fs::read(tmp.path().join("old.txt")).unwrap(), b"overwritten");

        std::fs::write(tmp.path().join("old.txt"), b"sentinel").unwrap();
        let resp = call(&api, upload_req(&base, "conflict=skip", &body)).expect("ok");
        assert_eq!(resp.status(), 200);
        assert_eq!(std::fs::read(tmp.path().join("old.txt")).unwrap(), b"sentinel");
    }

    fn upload_req(base: &str, query: &str, body: &[u8]) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method("POST")
            .uri(format!("{base}?type=upload&{query}"))
            .header(http::header::CONTENT_TYPE, "multipart/form-data; boundary=----b42")
            .body(body.to_vec())
            .unwrap()
    }

    fn multipart_body(boundary: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in files {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            out.extend_from_slice(data);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    fn url_enc_path(p: &std::path::Path) -> String {
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
