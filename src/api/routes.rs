//! routes — 唯一路由映射表(方法+路径 → 命令 + 参数提取 + 超时档)。
//!
//! 消费方:宿主协议 handler(moho-mate 的 `route_api` 对应物)与将来 axum 壳。
//! 双份映射的历史(JS ipc-bridge routeRequest ① + Rust route_invoke ②)在此坍缩为一份。
//!
//! 路径归一化知识(来源:P0 spike 报告,宿主仓库 docs/api-embed-p0-report.md):
//! 页面 `scheme://index.html` 下 `fetch('/api/x')` 到达形态为
//! `scheme://index.html/api/x` —— "index.html" 是 host 段。本层以
//! `http::Uri::path()` 取路径(host 天然分离),对宿主直接透传 wry Request
//! 的接法成立;防御性支持字符串归一(见 [`normalize_path`])。

use serde_json::Value;
use std::time::Duration;

/// 超时档(docs/api-embed-plan.md §二:默认 60s;长命令白名单 300s)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutClass {
    Default,
    Long,
}

/// 路由命中后的派发描述。
#[derive(Debug)]
pub struct Dispatch {
    pub command: &'static str,
    pub args: Value,
    pub timeout_class: TimeoutClass,
    /// 原始请求体(upload multipart 等字节形态命令用)。
    pub body: Vec<u8>,
    /// 请求 Content-Type(multipart boundary 提取用)。
    pub content_type: Option<String>,
}

/// 超时配置(经 ApiConfig 注入;测试可缩短)。
#[derive(Clone, Copy, Debug)]
pub struct TimeoutConfig {
    pub default: Duration,
    pub long: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            default: Duration::from_secs(60),
            long: Duration::from_secs(300),
        }
    }
}

impl TimeoutConfig {
    pub fn for_class(&self, class: TimeoutClass) -> Duration {
        match class {
            TimeoutClass::Default => self.default,
            TimeoutClass::Long => self.long,
        }
    }
}

/// 路由表:(方法, 模式, 命令, 超时档)。模式段 `:name` 捕获路径参数入 args。
const ROUTES: &[(&str, &str, &str, TimeoutClass)] = &[
    ("POST", "/api/agent/new", "agent_new", TimeoutClass::Default),
    ("GET", "/api/agent/running", "agent_running", TimeoutClass::Default),
    ("GET", "/api/agent/:id", "agent_get_state", TimeoutClass::Default),
    ("POST", "/api/agent/:id", "agent_rpc", TimeoutClass::Default),
    ("GET", "/api/home", "home", TimeoutClass::Default),
    ("GET", "/api/sessions", "sessions_list", TimeoutClass::Default),
    ("GET", "/api/sessions/:id", "sessions_get", TimeoutClass::Default),
    ("GET", "/api/sessions/:id/context", "sessions_context", TimeoutClass::Default),
    ("GET", "/api/cwd/browse", "cwd_browse", TimeoutClass::Default),
    ("POST", "/api/cwd/validate", "cwd_validate", TimeoutClass::Default),
    ("POST", "/api/default-cwd", "default_cwd", TimeoutClass::Default),
    ("GET", "/api/file-index", "file_index", TimeoutClass::Default),
    ("GET", "/api/models", "models_list", TimeoutClass::Default),
    ("GET", "/api/models-config", "models_config_get", TimeoutClass::Default),
    ("PUT", "/api/models-config", "models_config_put", TimeoutClass::Default),
    ("GET", "/api/git/status", "git_status", TimeoutClass::Default),
    ("GET", "/api/git/diff", "git_diff", TimeoutClass::Default),
    // files 八态:GET(读侧)+ POST(上传);*path 通配捕获文件路径
    ("GET", "/api/files/*path", "files", TimeoutClass::Default),
    ("POST", "/api/files/*path", "files", TimeoutClass::Default),
];

/// 路由解析:http 方言请求 → 派发描述;未命中 → None(调用方回 404)。
pub(crate) fn resolve(req: &http::Request<Vec<u8>>) -> Option<Dispatch> {
    let method = req.method();
    let path = normalize_path(req.uri().path());
    let query = req.uri().query().unwrap_or("");
    let mut args = query_to_args(query);
    // POST/PUT/PATCH:JSON body 字段并入 args(body 优先 —— 上游 route.ts 从 body 读参数)
    if matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(req.body()) {
            if let Some(args_map) = args.as_object_mut() {
                for (k, v) in map {
                    args_map.insert(k, v);
                }
            }
        }
    }

    for (m, pattern, command, class) in ROUTES {
        if method.as_str() != *m {
            continue;
        }
        if let Some(params) = match_pattern(pattern, &path) {
            // :id 保留字(前端契约:字面量 new/running 走专属路由,不作会话 id)
            if let Some(id) = params.get("id") {
                if id == "new" || id == "running" {
                    return None;
                }
            }
            if let (Some(args_map), Some(param_map)) = (args.as_object_mut(), Some(params)) {
                for (k, v) in param_map {
                    args_map.insert(k, v);
                }
            }
            let content_type = req
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            return Some(Dispatch {
                command,
                args,
                timeout_class: *class,
                body: req.body().clone(),
                content_type,
            });
        }
    }
    #[cfg(test)]
    if let Some(d) = resolve_test(method.as_str(), &path, args) {
        return Some(d);
    }
    None
}

/// 模式匹配:`:name` 捕获单段;`*name` 捕获剩余多段(逐段解码后以 / 重连,
/// 保留路径内斜杠);字面段精确相等。命中返回参数表。
fn match_pattern(pattern: &str, path: &str) -> Option<serde_json::Map<String, Value>> {
    let p_segs: Vec<&str> = pattern.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let mut params = serde_json::Map::new();
    let mut si = 0usize;
    for (pi, p) in p_segs.iter().enumerate() {
        if let Some(name) = p.strip_prefix('*') {
            // 通配:剩余所有段(逐段解码,防 %2F 被误展开)
            let rest: Vec<String> = segs[si..].iter().map(|s| url_decode(s)).collect();
            if rest.is_empty() && pi != p_segs.len() - 1 {
                return None;
            }
            params.insert(name.to_string(), Value::from(rest.join("/")));
            return Some(params);
        }
        let Some(seg) = segs.get(si) else {
            return None;
        };
        if let Some(name) = p.strip_prefix(':') {
            params.insert(name.to_string(), Value::from(url_decode(seg)));
        } else if p != seg {
            return None;
        }
        si += 1;
    }
    if si != segs.len() {
        return None;
    }
    Some(params)
}

#[cfg(test)]
fn resolve_test(method: &str, path: &str, args: Value) -> Option<Dispatch> {
    let table: &[(&str, &str, &str, TimeoutClass)] = &[
        ("GET", "/api/test-sleep", "test_sleep", TimeoutClass::Default),
        ("GET", "/api/test-panic", "test_panic", TimeoutClass::Default),
        ("GET", "/api/test-bytes", "test_bytes", TimeoutClass::Default),
    ];
    for (m, pat, command, class) in table {
        if method == *m && match_pattern(pat, path).is_some() {
            return Some(Dispatch {
                command,
                args,
                timeout_class: *class,
                body: Vec::new(),
                content_type: None,
            });
        }
    }
    None
}

/// 路径归一:去尾部斜杠(根除外);空路径视为根。
pub fn normalize_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return "/".to_string();
    }
    if p.len() > 1 && p.ends_with('/') {
        p[..p.len() - 1].to_string()
    } else {
        p.to_string()
    }
}

/// query string → 命令参数对象(覆盖式;同名后者胜,量级小不做重复收集)。
fn query_to_args(query: &str) -> Value {
    let mut map = serde_json::Map::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let k = url_decode(k);
        let v = url_decode(v);
        // 数字形态尽量还原为数值,其余保持字符串(对齐前端 query 用法)
        if let Ok(n) = v.parse::<i64>() {
            map.insert(k, Value::from(n));
        } else if let Ok(b) = v.parse::<bool>() {
            map.insert(k, Value::from(b));
        } else {
            map.insert(k, Value::from(v));
        }
    }
    Value::Object(map)
}

/// 最小百分号解码(query 用;UTF-8 百分号序列)。
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 1 && i + 2 < bytes.len() + 1 {
            if let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
