//! export — GET /api/sessions/:id/export(上游 pi-web 导出路由的 Rust 移植)。
//!
//! 服务端逻辑移植自 `@earendil-works/pi-coding-agent` 的
//! `dist/core/export-html/index.js`(exportFromFile 路径:不传 themeName /
//! toolRenderer,与 pi-web route.ts 的调用方式一致):
//! SessionData `{header, entries, leafId}` → base64 嵌入模板占位符。
//! 消息渲染与树视图由与上游完全相同的客户端模板承担(export_assets/,
//! 原样 vendor;视觉/交互与上游导出一致)。
//!
//! 深链会话的浏览器栈溢出补丁移植自 pi-web `app/api/sessions/[id]/export/
//! route.ts` 的 `patchExportHtml`(template.js 的 sortChildren/mapNodes/
//! markActive 递归 → 迭代;匹配失败会响亮报错,与上游 replaceRequired 同语义)。

use serde_json::Value;

use super::ApiError;

const TEMPLATE_HTML: &str = include_str!("export_assets/template.html");
const TEMPLATE_CSS: &str = include_str!("export_assets/template.css");
const TEMPLATE_JS: &str = include_str!("export_assets/template.js");
const MARKED_JS: &str = include_str!("export_assets/vendor/marked.min.js");
const HIGHLIGHT_JS: &str = include_str!("export_assets/vendor/highlight.min.js");

/// 默认主题(dark)解析后的 CSS 颜色 —— 上游 `getResolvedThemeColors(undefined)`
/// 的产出(exportFromFile 不传 themeName → 默认主题)。再生成方法见
/// export_assets/README.md;追上游换主题时同步此表。
const DEFAULT_THEME_COLORS: &[(&str, &str)] = &[
    ("accent", "#8abeb7"),
    ("border", "#5f87ff"),
    ("borderAccent", "#00d7ff"),
    ("borderMuted", "#505050"),
    ("success", "#b5bd68"),
    ("error", "#cc6666"),
    ("warning", "#ffff00"),
    ("muted", "#808080"),
    ("dim", "#666666"),
    ("text", "#d4d4d4"),
    ("thinkingText", "#808080"),
    ("selectedBg", "#3a3a4a"),
    ("scrollbarThumb", "#3a3a4a"),
    ("userMessageBg", "#343541"),
    ("userMessageText", "#d4d4d4"),
    ("customMessageBg", "#2d2838"),
    ("customMessageText", "#d4d4d4"),
    ("customMessageLabel", "#9575cd"),
    ("toolPendingBg", "#282832"),
    ("toolSuccessBg", "#283228"),
    ("toolErrorBg", "#3c2828"),
    ("toolTitle", "#d4d4d4"),
    ("toolOutput", "#808080"),
    ("mdHeading", "#f0c674"),
    ("mdLink", "#81a2be"),
    ("mdLinkUrl", "#666666"),
    ("mdCode", "#8abeb7"),
    ("mdCodeBlock", "#b5bd68"),
    ("mdCodeBlockBorder", "#808080"),
    ("mdQuote", "#808080"),
    ("mdQuoteBorder", "#808080"),
    ("mdHr", "#808080"),
    ("mdListBullet", "#8abeb7"),
    ("toolDiffAdded", "#b5bd68"),
    ("toolDiffRemoved", "#cc6666"),
    ("toolDiffContext", "#808080"),
    ("syntaxComment", "#6A9955"),
    ("syntaxKeyword", "#569CD6"),
    ("syntaxFunction", "#DCDCAA"),
    ("syntaxVariable", "#9CDCFE"),
    ("syntaxString", "#CE9178"),
    ("syntaxNumber", "#B5CEA8"),
    ("syntaxType", "#4EC9B0"),
    ("syntaxOperator", "#D4D4D4"),
    ("syntaxPunctuation", "#D4D4D4"),
    ("thinkingOff", "#505050"),
    ("thinkingMinimal", "#6e6e6e"),
    ("thinkingLow", "#5f87af"),
    ("thinkingMedium", "#81a2be"),
    ("thinkingHigh", "#b294bb"),
    ("thinkingXhigh", "#d183e8"),
    ("thinkingMax", "#ff5fff"),
    ("bashMode", "#b5bd68"),
];

/// dark 主题 JSON 的 `export` 节(显式导出三色;上游
/// `getThemeExportColors(undefined)` 产出,优先于 deriveExportColors 推导)。
const EXPORT_PAGE_BG: &str = "#18181e";
const EXPORT_CARD_BG: &str = "#1e1e24";
const EXPORT_INFO_BG: &str = "#3c3728";

/// 对齐上游 `generateThemeVars`:每行 `--key: value;`,以 `\n` + 6 空格连接;
/// 末尾追加三个导出背景色(themeExport 显式值优先,与上游同)。
fn theme_vars() -> String {
    let mut lines: Vec<String> = DEFAULT_THEME_COLORS
        .iter()
        .map(|(k, v)| format!("--{k}: {v};"))
        .collect();
    lines.push(format!("--exportPageBg: {EXPORT_PAGE_BG};"));
    lines.push(format!("--exportCardBg: {EXPORT_CARD_BG};"));
    lines.push(format!("--exportInfoBg: {EXPORT_INFO_BG};"));
    lines.join("\n      ")
}

/// JS `String.prototype.replace`(字符串模式)的替换串语义:
/// `$$`→`$`、`$&`→被匹配的搜索串、`` $` ``→匹配前文、`$'`→匹配后文;
/// 其余 `$x` 原样。资产里刻意用了这些序列(template.js 的 `$$`、
/// highlight.min.js 的 `$&`),必须复刻才能与上游输出逐字节一致。
fn js_replace(haystack: &str, search: &str, replacement: &str) -> String {
    let Some(pos) = haystack.find(search) else {
        return haystack.to_string();
    };
    let before = &haystack[..pos];
    let after = &haystack[pos + search.len()..];
    let mut out = String::with_capacity(haystack.len() + replacement.len());
    out.push_str(before);
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('&') => {
                chars.next();
                out.push_str(search);
            }
            Some('`') => {
                chars.next();
                out.push_str(before);
            }
            Some('\'') => {
                chars.next();
                out.push_str(after);
            }
            _ => out.push('$'),
        }
    }
    out.push_str(after);
    out
}

/// 对齐上游 `generateHtml`:占位符拼装(模板内每个占位符恰一次;
/// JS `.replace` 只换首次,Rust 端按同语义实现)。
pub(crate) fn generate_export_html(session_data: &Value) -> Result<String, String> {
    let json = serde_json::to_string(session_data).map_err(|e| e.to_string())?;
    use base64::Engine as _;
    let session_data_b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

    let css = js_replace(TEMPLATE_CSS, "{{THEME_VARS}}", &theme_vars())
        .then_js_replace("{{BODY_BG}}", EXPORT_PAGE_BG)
        .then_js_replace("{{CONTAINER_BG}}", EXPORT_CARD_BG)
        .then_js_replace("{{INFO_BG}}", EXPORT_INFO_BG);
    Ok(js_replace(TEMPLATE_HTML, "{{CSS}}", &css)
        .then_js_replace("{{JS}}", TEMPLATE_JS)
        .then_js_replace("{{SESSION_DATA}}", &session_data_b64)
        .then_js_replace("{{MARKED_JS}}", MARKED_JS)
        .then_js_replace("{{HIGHLIGHT_JS}}", HIGHLIGHT_JS))
}

/// 链式小工具(纯为上文的占位符拼装可读性)。
trait ThenJsReplace {
    fn then_js_replace(self, search: &str, replacement: &str) -> String;
}

impl ThenJsReplace for String {
    fn then_js_replace(self, search: &str, replacement: &str) -> String {
        js_replace(&self, search, replacement)
    }
}

/// 读会话 jsonl → SessionData(对齐上游 exportFromFile 的读取语义:
/// header = 首个 type=session 条目;entries = 其余全部;leafId = 最后一个
/// 非 header 条目的 id,无条目 → null)。
///
/// 与上游的已知差异(有意,更安全):上游 `SessionManager.open` 在加载时会把
/// jsonl 规范化后**写回源文件**(entry id 再生成、parentId 重挂、header 补
/// version —— 生成 tests/export_fixture/reference.html 时实测发生);本移植
/// 是纯读 passthrough,不改会话文件。entry 以文件原样进入 SESSION_DATA,
/// 树形/高亮渲染不受影响(id 链自洽即可)。坏行容忍跳过(上游严格解析)。
pub(crate) fn session_data_from_jsonl(content: &str) -> Value {
    let mut header = Value::Null;
    let mut entries: Vec<Value> = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches(['\r']);
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("session") && header.is_null() {
            header = v;
        } else {
            entries.push(v);
        }
    }
    let leaf_id = entries
        .iter()
        .rev()
        .find_map(|e| e.get("id").and_then(|i| i.as_str()).map(String::from));
    serde_json::json!({ "header": header, "entries": entries, "leafId": leaf_id })
}

/// 组装 + 打补丁(exportFromFile + pi-web route patchExportHtml 的完整路径)。
pub(crate) fn export_session_html(jsonl_content: &str) -> Result<String, String> {
    let data = session_data_from_jsonl(jsonl_content);
    patch_export_html(&generate_export_html(&data)?)
}

/// 对齐 pi-web route.ts `patchExportHtml`:template.js 三处递归函数替换为
/// 迭代版(深链会话在浏览器栈溢出的修复)。每处搜索串必须恰命中一次,
/// 否则报错(与上游 replaceRequired 同语义 —— 模板换版时响亮失败)。
fn patch_export_html(html: &str) -> Result<String, String> {
    // 行尾归一(route.ts 为 CRLF,模板为 LF;归一后匹配)
    let html = html.replace("\r\n", "\n");
    let html = replace_required(
        html,
        "sortChildren",
        "        function sortChildren(node) {\n          node.children.sort((a, b) =>\n            new Date(a.entry.timestamp).getTime() - new Date(b.entry.timestamp).getTime()\n          );\n          node.children.forEach(sortChildren);\n        }",
        "        function sortChildren(root) {\n          const stack = [root];\n          while (stack.length) {\n            const node = stack.pop();\n            node.children.sort((a, b) =>\n              new Date(a.entry.timestamp).getTime() - new Date(b.entry.timestamp).getTime()\n            );\n            for (let i = node.children.length - 1; i >= 0; i--) {\n              stack.push(node.children[i]);\n            }\n          }\n        }",
    )?;
    let html = replace_required(
        html,
        "mapNodes",
        "          function mapNodes(node) {\n            treeNodeMap.set(node.entry.id, node);\n            node.children.forEach(mapNodes);\n          }\n          tree.forEach(mapNodes);",
        "          const stack = [...tree].reverse();\n          while (stack.length) {\n            const node = stack.pop();\n            treeNodeMap.set(node.entry.id, node);\n            for (let i = node.children.length - 1; i >= 0; i--) {\n              stack.push(node.children[i]);\n            }\n          }",
    )?;
    replace_required(
        html,
        "markActive",
        "        function markActive(node) {\n          let has = activePathIds.has(node.entry.id);\n          for (const child of node.children) {\n            if (markActive(child)) has = true;\n          }\n          containsActive.set(node, has);\n          return has;\n        }",
        "        function markActive(root) {\n          // Post-order traversal using two stacks\n          const stack1 = [root];\n          const stack2 = [];\n          while (stack1.length) {\n            const node = stack1.pop();\n            stack2.push(node);\n            for (const child of node.children) {\n              stack1.push(child);\n            }\n          }\n          while (stack2.length) {\n            const node = stack2.pop();\n            let has = activePathIds.has(node.entry.id);\n            for (const child of node.children) {\n              if (containsActive.get(child)) has = true;\n            }\n            containsActive.set(node, has);\n          }\n        }",
    )
}

fn replace_required(source: String, name: &str, search: &str, replacement: &str) -> Result<String, String> {
    let matches = source.matches(search).count();
    if matches != 1 {
        return Err(format!(
            "Failed to patch exported HTML: {name} expected 1 match, found {matches}"
        ));
    }
    Ok(source.replacen(search, replacement, 1))
}

/// 对齐 route.ts `getContentDisposition`/`encodeHeaderValue`:
/// ASCII 回退名 + RFC 5987 `filename*`(encodeURIComponent 再把 !'()* 转大写十六进制)。
pub(crate) fn content_disposition(file_name: &str, inline: bool) -> String {
    let fallback: String = file_name
        .chars()
        .map(|c| {
            let keep = matches!(c, '\x20'..='\x7E') && !"\"\\;\r\n".contains(c);
            if keep { c } else { '_' }
        })
        .collect();
    let fallback = if fallback.is_empty() { "session.html".to_string() } else { fallback };
    let disposition = if inline { "inline" } else { "attachment" };
    format!(
        "{disposition}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        encode_header_value(file_name)
    )
}

fn encode_header_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 导出文件名(对齐 route.ts:`pi-session-<basename 去掉 .jsonl>.html`)。
pub(crate) fn export_file_name(session_path: &std::path::Path) -> String {
    let base = session_path.file_name().and_then(|n| n.to_str()).unwrap_or("session");
    let stem = base.strip_suffix(".jsonl").unwrap_or(base);
    format!("pi-session-{stem}.html")
}

/// 响应组装(headers 对齐 route.ts:266-274)。
pub(crate) fn html_response(html: String, file_name: &str, inline: bool) -> Result<http::Response<Vec<u8>>, ApiError> {
    Ok(http::Response::builder()
        .status(200)
        .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(http::header::CONTENT_DISPOSITION, content_disposition(file_name, inline))
        .header(http::header::CACHE_CONTROL, "no-cache")
        .header("Content-Security-Policy", "frame-ancestors 'none'")
        .header("X-Content-Type-Options", "nosniff")
        .header("X-Frame-Options", "DENY")
        .body(html.into_bytes())
        .map_err(|e| ApiError::internal(e.to_string()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_JSONL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/export_fixture/session.jsonl"
    ));
    /// 上游 0.84.1 exportFromFile 对同一 fixture 的真实输出(未打补丁)。
    const REFERENCE_HTML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/export_fixture/reference.html"
    ));

    #[test]
    fn js_replace_semantics() {
        assert_eq!(js_replace("a{{X}}b", "{{X}}", "1$$2"), "a1$2b");
        assert_eq!(js_replace("a{{X}}b", "{{X}}", "[$&]"), "a[{{X}}]b");
        assert_eq!(js_replace("a{{X}}b", "{{X}}", "$`|$'"), "aa|bb");
        // 其余 $x 原样;无命中原样返回
        assert_eq!(js_replace("a{{X}}b", "{{X}}", "$z"), "a$zb");
        assert_eq!(js_replace("abc", "{{X}}", "$$"), "abc");
    }

    #[test]
    fn session_data_shape() {
        let data = session_data_from_jsonl(FIXTURE_JSONL);
        assert_eq!(data["header"]["id"], "fix-uuid-1");
        // leafId = 最后一个非 header 条目(上游 getLeafId 语义,非 header.leafId)。
        // 注意:fixture 由上游 SessionManager.open 规范化过(id 再生成 + 写回,
        // 见下方"与上游的已知差异")—— 062c2793 是规范化后的末条 id。
        assert_eq!(data["leafId"], "062c2793");
        let entries = data["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0]["id"], "70554a38");
    }

    /// 与上游真实输出做逐字节对拍:模板拼装(JS 替换语义)+ 三处补丁后,
    /// 除 SESSION_DATA(JSON 序列化次序差异)外必须与
    /// patchExportHtml(上游输出)完全一致。
    #[test]
    fn byte_parity_with_upstream_output() {
        let ours = export_session_html(FIXTURE_JSONL).expect("export");
        let patched_reference = patch_export_html(REFERENCE_HTML.replace("\r\n", "\n").as_str())
            .expect("patch applies to upstream output");

        let marker = "<script id=\"session-data\" type=\"application/json\">";
        fn split<'a>(html: &'a str, marker: &str) -> (&'a str, &'a str, &'a str) {
            let pos = html.find(marker).expect("session data marker");
            let start = pos + marker.len();
            let end = html[start..].find("</script>").expect("script close") + start;
            (&html[..pos], &html[start..end], &html[end..])
        }
        let (our_pre, our_data, our_post) = split(&ours, marker);
        let (ref_pre, ref_data, ref_post) = split(&patched_reference, marker);
        assert_eq!(our_pre, ref_pre, "prefix (head+css) must be byte-identical");
        assert_eq!(our_post, ref_post, "suffix (marked+highlight+patched js) must be byte-identical");

        // SESSION_DATA:JSON 键序可能不同(TS 插入序 vs serde 序),语义比对
        use base64::Engine as _;
        let decode = |b64: &str| {
            serde_json::from_slice::<Value>(
                &base64::engine::general_purpose::STANDARD.decode(b64.trim()).expect("b64"),
            )
            .expect("json")
        };
        assert_eq!(decode(our_data), decode(ref_data));

        // 补丁生效:迭代版标记在,递归版不在
        assert!(ours.contains("const stack = [root];"));
        assert!(!ours.contains("node.children.forEach(sortChildren);"));
    }

    #[test]
    fn dollar_patterns_actually_exercise() {
        // 资产内确有 $$ / $& —— 防止换版资产后 js_replace 变成死代码的哨兵
        assert!(TEMPLATE_JS.contains("$$"));
        assert!(HIGHLIGHT_JS.contains("$&"));
    }

    #[test]
    fn content_disposition_shape() {
        let d = content_disposition("pi-session-2026-a1b2c3d4.html", true);
        assert_eq!(
            d,
            "inline; filename=\"pi-session-2026-a1b2c3d4.html\"; filename*=UTF-8''pi-session-2026-a1b2c3d4.html"
        );
        let d = content_disposition("café.html", false);
        assert!(d.starts_with("attachment; filename=\"caf_.html\""));
        assert!(d.ends_with("filename*=UTF-8''caf%C3%A9.html"));
    }

    // ── 路由级集成(GET /api/sessions/:id/export)──────────────────────────

    use super::super::{ApiConfig, EventSink, HostHooks, PiWebApi};
    use std::sync::Arc;

    struct SessionsRoot(std::path::PathBuf);
    impl HostHooks for SessionsRoot {
        fn sessions_root(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
    }

    fn api_with_sessions_root(root: &std::path::Path) -> PiWebApi {
        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let rt = Arc::new(
            asupersync::runtime::RuntimeBuilder::multi_thread()
                .blocking_threads(1, 2)
                .with_reactor(reactor)
                .build()
                .unwrap(),
        );
        let mut cfg = ApiConfig::new(Arc::new(|_: crate::api::ApiEvent| {}) as EventSink);
        cfg.hooks = Arc::new(SessionsRoot(root.to_path_buf()));
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

    #[test]
    fn export_route_serves_patched_html() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let day = root.join("2026-08-21");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("2026-08-21T00-00-00.000Z_deadbeef.jsonl");
        std::fs::write(&path, FIXTURE_JSONL).unwrap();
        let api = api_with_sessions_root(&root);

        // inline=1 → text/html + inline disposition + 安全头 + 补丁生效
        let resp = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri("/api/sessions/fix-uuid-1/export?inline=1")
                .body(Vec::new())
                .unwrap(),
        )
        .expect("export ok");
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get(http::header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let cd = resp.headers().get(http::header::CONTENT_DISPOSITION).unwrap().to_str().unwrap();
        assert!(cd.starts_with("inline; filename=\"pi-session-2026-08-21T00-00-00.000Z_deadbeef.html\""));
        assert_eq!(resp.headers().get(http::header::CACHE_CONTROL).unwrap(), "no-cache");
        assert_eq!(resp.headers().get("X-Frame-Options").unwrap(), "DENY");
        let body = String::from_utf8(resp.body().clone()).unwrap();
        assert!(body.contains("const stack = [root];"), "deep-chain patch applied");
        assert!(!body.contains("node.children.forEach(sortChildren);"));

        // 无 inline → attachment
        let resp = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri("/api/sessions/fix-uuid-1/export")
                .body(Vec::new())
                .unwrap(),
        )
        .expect("export ok");
        let cd = resp.headers().get(http::header::CONTENT_DISPOSITION).unwrap().to_str().unwrap();
        assert!(cd.starts_with("attachment;"));

        // 不存在 → 404(上游同文案)
        let e = call(
            &api,
            http::Request::builder()
                .method("GET")
                .uri("/api/sessions/nope/export?inline=1")
                .body(Vec::new())
                .unwrap(),
        )
        .unwrap_err();
        assert_eq!(e.status, 404);
        assert_eq!(e.message, "Session not found");

        // 导出是纯读:源文件不被改动(上游 SessionManager.open 会规范化写回,见模块注释)
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, FIXTURE_JSONL);
    }
}
