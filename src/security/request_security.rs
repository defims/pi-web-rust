//! 对齐 `lib/request-security.ts`。
//!
//! API 请求的 SSRF/CSRF 防护,纯计算、无 IO:
//! - `is_api_request_host_allowed`:Host 头只信任本地名 / IP 字面量 / 显式配置的主机名
//!   (IP 字面量保留 LAN 访问,但浏览器在 Host 头中保留字面地址,无法 DNS rebind)。
//! - `is_api_request_origin_allowed`:拒绝浏览器跨站 API 请求,保留非浏览器客户端。
//!
//! 语义按 Node `new URL()`(WHATWG URL)逐项对齐,包括 quirks:
//! 空端口不写入 origin、端口 0 保留、`http://example.com:` 被解析为 host=example.com。

/// 对齐 `normalizeHostname`。去掉 IPv6 方括号、转小写、去掉尾部点。
fn normalize_hostname(value: &str) -> String {
    let unbracketed = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value);
    unbracketed.to_lowercase().trim_end_matches('.').to_string()
}

/// 对齐 `hostnameFromAuthority`。从 `host[:port]` 形式解析主机名。
/// 含空白 / `@` / `\` 或非空 userinfo / 非 `/` path / query / fragment 时返回 None。
pub fn hostname_from_authority(value: &str) -> Option<String> {
    if value.is_empty() || value.chars().any(|ch| ch.is_whitespace() || matches!(ch, '/' | '@' | '\\')) {
        return None;
    }
    // `/` 已被上面的 regex 拒绝,`?`/`#` 只能落在 query/fragment 上
    // (对齐 TS 对 `parsed.search`/`parsed.hash` 的非空校验)。
    if value.contains(['?', '#']) {
        return None;
    }
    let parsed = parse_authority(value)?;
    if parsed.username.is_some() || parsed.password.is_some() || parsed.pathname != "/" {
        return None;
    }
    Some(normalize_hostname(&parsed.hostname))
}

struct AuthorityParts {
    username: Option<String>,
    password: Option<String>,
    hostname: String,
    pathname: String,
}

/// 对齐 `new URL("http://" + value)` 的主机/路径部分。
///
/// 忠实复刻 Node 的怪癖:带空端口的 `example.com:` 解析为 host=example.com;
/// 端口按第一个冒号切分,须为纯数字且 ≤ 65535(`a:b:c`、`example.com:abc` 非法)。
/// 详见测试。
fn parse_authority(value: &str) -> Option<AuthorityParts> {
    let authority_len = if let Some(rest) = value.strip_prefix("//") {
        // 双斜杠起始:`//authority` —— 按相对 URL 语义,host 落在 path 里。
        // 例:`new URL("http:////x")` → hostname "http",pathname "///x"。
        rest.len()
    } else {
        let slash = value.find('/').unwrap_or(value.len());
        let question = value.find('?').unwrap_or(value.len());
        let hash = value.find('#').unwrap_or(value.len());
        slash.min(question).min(hash)
    };

    let authority = &value[..authority_len];
    let rest = &value[authority_len..];

    let (username, password, authority) = if let Some(at) = authority.rfind('@') {
        let userinfo = &authority[..at];
        let authority = &authority[at + 1..];
        let (user, pass) = match userinfo.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(userinfo.to_string()), None),
        };
        (user, pass, authority)
    } else {
        (None, None, authority)
    };

    let (host, port) = split_host_port(authority)?;
    if let Some(p) = &port {
        if !is_valid_port(p) {
            return None;
        }
    }

    // 无尾随斜杠时 Node 的 pathname 为 "/";有则原样保留(含双斜杠)
    let pathname = if rest.is_empty() || !rest.starts_with('/') {
        "/".to_string()
    } else {
        rest.to_string()
    };

    Some(AuthorityParts { username, password, hostname: host, pathname })
}

/// 切分 authority 的 host 与端口。IPv6 方括号整体作 host;否则按第一个冒号切分。
///
/// 对齐 WHATWG host 规则:`[`/`]` 只能成对出现在最前的合法 IPv6 字面量
/// (内容须为有效 v6、不含 zone id),`]` 之后只允许 `:port`;
/// 其余任何位置出现 `[`/`]` 都非法。
fn split_host_port(authority: &str) -> Option<(String, Option<String>)> {
    if authority.starts_with('[') {
        let close = authority.find(']')?;
        let v6 = &authority[1..close];
        if !is_ipv6(v6) || v6.contains('%') {
            return None;
        }
        let after = &authority[close + 1..];
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p.to_string()),
            None if after.is_empty() => None,
            None => return None, // `]` 后只能跟 `:port`
        };
        return Some((authority[..=close].to_string(), port));
    }
    if authority.contains(['[', ']']) {
        return None;
    }
    match authority.find(':') {
        Some(idx) => Some((authority[..idx].to_string(), Some(authority[idx + 1..].to_string()))),
        None => Some((authority.to_string(), None)),
    }
}

/// 端口校验:空 = 无端口;否则纯数字且 ≤ 65535。
fn is_valid_port(port: &str) -> bool {
    if port.is_empty() {
        return true;
    }
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    port.parse::<u32>().map(|n| n <= 65535).unwrap_or(false)
}

/// 对齐 `normalizeConfiguredHostname`。IP 字面量直接规范化,否则按 authority 解析。
fn normalize_configured_hostname(value: Option<&str>) -> Option<String> {
    let trimmed = value.map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        return None;
    }
    if is_ip(trimmed) {
        return Some(normalize_hostname(trimmed));
    }
    hostname_from_authority(trimmed)
}

/// 对齐 `isLoopbackHostname`。
fn is_loopback_hostname(hostname: &str) -> bool {
    hostname == "localhost" || hostname.ends_with(".localhost")
}

/// 对齐 `configuredHostnamesFromEnvironment`。
pub fn configured_hostnames_from_environment() -> Vec<String> {
    let mut out = Vec::new();
    for key in ["PI_WEB_HOSTNAME", "PI_WEB_ALLOWED_HOSTS"] {
        if let Ok(value) = std::env::var(key) {
            if key == "PI_WEB_HOSTNAME" {
                if !value.trim().is_empty() {
                    out.push(value);
                }
            } else {
                out.extend(value.split(',').map(str::to_string));
            }
        }
    }
    out
}

/// 对齐 `canonicalOrigin`。返回 `scheme://host[:port]` 形式,失败时 None。
///
/// 按 WHATWG 规则:scheme/host 转小写、保留 IPv6 方括号、空端口省略、
/// 默认端口(80/443/21 等)省略、端口 0 保留、userinfo 剥离、前导零归约。
pub fn canonical_origin(value: &str) -> Option<String> {
    let url = parse_url(value)?;
    let scheme = url.scheme.to_lowercase();
    let host = url.host.to_lowercase();
    let default_port = match scheme.as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    };
    let port_num = url.port.as_deref().filter(|p| !p.is_empty()).and_then(|p| p.parse::<u32>().ok());
    let mut origin = format!("{scheme}://{host}");
    if let Some(p) = port_num {
        if default_port != Some(p) {
            origin.push(':');
            origin.push_str(&p.to_string());
        }
    }
    Some(origin)
}

struct UrlParts {
    scheme: String,
    host: String,
    port: Option<String>,
}

/// 解析绝对 URL 的 scheme://host[:port],host 为空时抛错。
/// 对齐 `new URL(value)` 的解析(host 缺省时整体按 path,origin 取不到)。
fn parse_url(value: &str) -> Option<UrlParts> {
    let value = value.trim();
    let scheme_end = value.find("://")?;
    let scheme = &value[..scheme_end];
    if scheme.is_empty() {
        return None;
    }
    let mut rest = &value[scheme_end + 3..];
    if let Some(slash) = rest.find(['/', '?', '#']) {
        rest = &rest[..slash];
    }
    if rest.is_empty() {
        return None;
    }
    // 剥离 userinfo(Node origin 不含 user:pass@)
    if let Some(at) = rest.rfind('@') {
        rest = &rest[at + 1..];
        if rest.is_empty() {
            return None;
        }
    }
    let (host, port) = split_host_port(rest)?;
    if let Some(p) = &port {
        if !is_valid_port(p) {
            return None;
        }
    }
    Some(UrlParts { scheme: scheme.to_string(), host, port })
}

/// 对齐 `getRequestOrigin`。从请求 URL + Host 头构造规范 origin。
/// 对应上游 `request.url` 与 `request.headers.get("host")`。
pub fn get_request_origin(request_url: &str, host_header: Option<&str>) -> Option<String> {
    let scheme = parse_url_scheme(request_url)?;
    let host = host_header?;
    if host.is_empty() {
        return None;
    }
    canonical_origin(&format!("{scheme}://{host}"))
}

fn parse_url_scheme(value: &str) -> Option<String> {
    let scheme_end = value.find("://")?;
    let scheme = &value[..scheme_end];
    if scheme.is_empty() {
        return None;
    }
    Some(scheme.to_string())
}

/// 对齐 `isUserInitiatedSessionExportNavigation`。浏览器用户主动触发的会话导出导航。
fn is_user_initiated_session_export_navigation(
    method: &str,
    sec_fetch_mode: Option<&str>,
    sec_fetch_dest: Option<&str>,
    sec_fetch_user: Option<&str>,
    pathname: &str,
) -> bool {
    if method != "GET"
        || sec_fetch_mode != Some("navigate")
        || sec_fetch_dest != Some("document")
        || sec_fetch_user != Some("?1")
    {
        return false;
    }
    let Some(rest) = pathname.strip_prefix("/api/sessions/") else {
        return false;
    };
    let (_, tail) = rest.split_once('/').unwrap_or((rest, ""));
    tail == "export"
}

/// 对齐 `isApiRequestHostAllowed`。仅信任本地名、IP 字面量或显式配置的主机名。
pub fn is_api_request_host_allowed(host_header: Option<&str>) -> bool {
    is_api_request_host_allowed_with(host_header, &configured_hostnames_from_environment())
}

/// 带配置注入的版本(便于测试;配置为空等价于未设置环境变量)。
pub fn is_api_request_host_allowed_with(host_header: Option<&str>, configured: &[String]) -> bool {
    let hostname = host_header.and_then(hostname_from_authority);
    let Some(hostname) = hostname else { return false; };
    if is_loopback_hostname(&hostname) || is_ip(&hostname) {
        return true;
    }
    configured
        .iter()
        .any(|configured| normalize_configured_hostname(Some(configured)) == Some(hostname.clone()))
}

/// 对齐 `isApiRequestOriginAllowed`。拒绝浏览器跨站 API 请求,保留非浏览器客户端。
pub fn is_api_request_origin_allowed(
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
    request_url: &str,
    host_header: Option<&str>,
) -> bool {
    if sec_fetch_site == Some("cross-site") {
        return false;
    }
    let Some(origin) = origin else { return true; };
    let request_origin = get_request_origin(request_url, host_header);
    request_origin.is_some() && canonical_origin(origin) == request_origin
}

/// 对齐 `shouldCheckApiRequestOrigin`。无 Origin/Sec-Fetch-Site 头的请求不检查。
pub fn should_check_api_request_origin(origin: Option<&str>, sec_fetch_site: Option<&str>) -> bool {
    origin.is_some() || sec_fetch_site.is_some()
}

/// 对齐 `isApiRequestAllowed`。Host 校验 + 会话导出豁免 + Origin 校验。
pub fn is_api_request_allowed(
    method: &str,
    host_header: Option<&str>,
    origin: Option<&str>,
    sec_fetch_mode: Option<&str>,
    sec_fetch_dest: Option<&str>,
    sec_fetch_site: Option<&str>,
    sec_fetch_user: Option<&str>,
    request_url: &str,
) -> bool {
    if !is_api_request_host_allowed(host_header) {
        return false;
    }
    let pathname = parse_url_pathname(request_url).unwrap_or_else(|| "/".to_string());
    if is_user_initiated_session_export_navigation(
        method,
        sec_fetch_mode,
        sec_fetch_dest,
        sec_fetch_user,
        pathname.as_str(),
    ) {
        return true;
    }
    !should_check_api_request_origin(origin, sec_fetch_site)
        || is_api_request_origin_allowed(origin, sec_fetch_site, request_url, host_header)
}

fn parse_url_pathname(value: &str) -> Option<String> {
    let mut rest = value.split("://").nth(1)?;
    let query_hash = rest.find(['?', '#']).unwrap_or(rest.len());
    rest = &rest[..query_hash];
    let slash = rest.find('/');
    Some(match slash {
        Some(idx) => rest[idx..].to_string(),
        None => "/".to_string(),
    })
}

/// 对齐 `hasJsonContentType`。`application/json` 或 `application/*+json`。
pub fn has_json_content_type(content_type: Option<&str>) -> bool {
    let Some(media_type) = content_type else { return false; };
    let media_type = media_type.split(';').next().unwrap_or("").trim().to_lowercase();
    media_type == "application/json"
        || (media_type.starts_with("application/") && media_type.ends_with("+json"))
}

/// IP 字面量判定(IPv4 / IPv6 / IPv4-mapped IPv6 等),对齐 `node:net` isIP。
pub fn is_ip(value: &str) -> bool {
    is_ipv4(value) || is_ipv6(value)
}

/// IPv4 判定:4 组 0-255 十进制数,拒绝前导零(对齐 `net.isIP`)。
fn is_ipv4(value: &str) -> bool {
    let mut parts = value.split('.');
    let mut count = 0;
    for _ in 0..4 {
        match parts.next() {
            Some(part) if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) => {
                if part.len() > 1 && part.starts_with('0') {
                    return false;
                }
                match part.parse::<u32>() {
                    Ok(n) if n <= 255 => count += 1,
                    _ => return false,
                }
            }
            _ => return false,
        }
    }
    count == 4 && parts.next().is_none()
}

/// 单个 IPv6 十六进制组:1-4 位 hex。
fn is_hex_group(value: &str) -> bool {
    (1..=4).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// IPv6 判定(对齐 `net.isIP`):允许尾部 zone id(`fe80::1%eth0`)、
/// 至多一个 `::`、尾部可嵌 IPv4 字面量(`::ffff:192.168.1.1` 算 2 组)。
fn is_ipv6(value: &str) -> bool {
    // zone id:`%` 后必须有非空内容
    let value = if let Some(idx) = value.find('%') {
        if value[idx + 1..].is_empty() {
            return false;
        }
        &value[..idx]
    } else {
        value
    };
    if value.is_empty() {
        return false;
    }

    // 至多一个 "::"
    let mut parts = value.split("::");
    let head = parts.next().unwrap_or("");
    let tail = match parts.next() {
        Some(t) => t,
        None => return is_ipv6_full_form(head),
    };
    if parts.next().is_some() {
        return false;
    }

    // 空串 = 零组(合法);非空但有非法分隔(空组)时返回 None
    let (Some(head_groups), Some(tail_groups)) = (
        if head.is_empty() { Some(Vec::new()) } else { split_groups(head) },
        if tail.is_empty() { Some(Vec::new()) } else { split_groups(tail) },
    ) else {
        return false;
    };
    let head_units = count_groups(&head_groups, false);
    let tail_units = count_groups(&tail_groups, true);
    head_units + tail_units <= 7
}

/// 无 `::` 的全长 IPv6:恰好 8 组(末组可为 IPv4,按 2 组计)。
fn is_ipv6_full_form(value: &str) -> bool {
    if value.starts_with(':') || value.ends_with(':') {
        return false;
    }
    let groups = split_groups(value);
    let Some(groups) = groups else { return false; };
    count_groups(&groups, true) == 8
}

fn split_groups(value: &str) -> Option<Vec<&str>> {
    let groups: Vec<&str> = value.split(':').collect();
    if groups.iter().any(|g| g.is_empty()) {
        return None;
    }
    Some(groups)
}

/// 统计组数;IPv4 组按 2 组计。`allow_tail_v4` 仅对末位生效。
fn count_groups(groups: &[&str], allow_tail_v4: bool) -> usize {
    let mut units = 0;
    let n = groups.len();
    for (i, g) in groups.iter().enumerate() {
        if i == n - 1 && allow_tail_v4 && is_ipv4(g) {
            units += 2;
        } else if is_hex_group(g) {
            units += 1;
        } else {
            return usize::MAX;
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_hostname_cases() {
        assert_eq!(normalize_hostname("Example.COM."), "example.com");
        assert_eq!(normalize_hostname("[::1]"), "::1");
        assert_eq!(normalize_hostname("localhost."), "localhost");
    }

    #[test]
    fn hostname_from_authority_cases() {
        assert_eq!(hostname_from_authority("example.com"), Some("example.com".into()));
        assert_eq!(hostname_from_authority("example.com:8080"), Some("example.com".into()));
        // 空端口怪癖:整体按 host 解析
        assert_eq!(hostname_from_authority("example.com:"), Some("example.com".into()));
        assert_eq!(hostname_from_authority("[::1]:8080"), Some("::1".into()));
        // 含 `/` 的值在解析前被 regex 拒绝(对齐 TS 的 /[\s/@\\]/ 先行校验)
        assert_eq!(hostname_from_authority("example.com:/x"), None);
        assert_eq!(hostname_from_authority("Example.COM."), Some("example.com".into()));

        assert_eq!(hostname_from_authority(""), None);
        assert_eq!(hostname_from_authority("host/"), None);
        assert_eq!(hostname_from_authority("user@host"), None);
        assert_eq!(hostname_from_authority("a\\b"), None);
        assert_eq!(hostname_from_authority("exa mple.com"), None);
        assert_eq!(hostname_from_authority("example.com?x"), None);
        assert_eq!(hostname_from_authority("example.com#f"), None);
        // 端口按第一个冒号切分,须为纯数字 ≤ 65535
        assert_eq!(hostname_from_authority("a:b:c"), None);
        assert_eq!(hostname_from_authority("foo[bar"), None);
        assert_eq!(hostname_from_authority("example.com["), None);
        assert_eq!(hostname_from_authority("[gg]"), None);
        assert_eq!(hostname_from_authority("[::1]z:80"), None);
        assert_eq!(hostname_from_authority("[fe80::1%eth0]"), None);
        // 非法端口
        assert_eq!(hostname_from_authority("example.com:99999"), None);
        assert_eq!(hostname_from_authority("example.com:65536"), None);
        assert_eq!(hostname_from_authority("example.com:abc"), None);
        assert_eq!(hostname_from_authority("example.com:65535"), Some("example.com".into()));
        assert_eq!(hostname_from_authority("example.com:0"), Some("example.com".into()));
        assert_eq!(hostname_from_authority("example.com:8080:9090"), None);
    }

    #[test]
    fn canonical_origin_cases() {
        assert_eq!(canonical_origin("http://example.com:80"), Some("http://example.com".into()));
        assert_eq!(canonical_origin("http://example.com"), Some("http://example.com".into()));
        assert_eq!(canonical_origin("https://example.com:443"), Some("https://example.com".into()));
        assert_eq!(canonical_origin("http://user:pass@example.com/x"), Some("http://example.com".into()));
        assert_eq!(canonical_origin("http://[::1]:8080"), Some("http://[::1]:8080".into()));
        assert_eq!(canonical_origin("http://example.com:8080/path"), Some("http://example.com:8080".into()));
        assert_eq!(canonical_origin("HTTPS://EXAMPLE.com:8443"), Some("https://example.com:8443".into()));
        assert_eq!(canonical_origin("http://[::1]"), Some("http://[::1]".into()));
        // 空端口省略(对齐 Node)
        assert_eq!(canonical_origin("http://example.com:"), Some("http://example.com".into()));
        // 端口 0 保留
        assert_eq!(canonical_origin("http://example.com:0"), Some("http://example.com:0".into()));
        // 前导零归约后与默认端口一致 → 省略
        assert_eq!(canonical_origin("http://example.com:00080"), Some("http://example.com".into()));
        assert_eq!(canonical_origin("http://example.com:00"), Some("http://example.com:0".into()));
        assert_eq!(canonical_origin("http://example.com:65535"), Some("http://example.com:65535".into()));
        // 非默认 https 端口保留
        assert_eq!(canonical_origin("https://example.com:8443"), Some("https://example.com:8443".into()));
        // 无 host → None
        assert_eq!(canonical_origin("http:///x"), None);
        assert_eq!(canonical_origin("not a url"), None);
    }

    #[test]
    fn is_ip_cases() {
        assert!(is_ip("127.0.0.1"));
        assert!(is_ip("192.168.1.1"));
        assert!(is_ip("0.0.0.0"));
        assert!(is_ip("255.255.255.255"));
        assert!(is_ip("1.2.3.4"));
        assert!(!is_ip("256.0.0.1"));
        assert!(!is_ip("1.2.3"));
        assert!(!is_ip("1.2.3.4.5"));
        assert!(!is_ip(""));
        assert!(!is_ip("a.b.c.d"));
        assert!(!is_ip("1.2.3.04"));

        assert!(is_ip("::1"));
        assert!(is_ip("::"));
        assert!(is_ip("2001:db8::ff00:42:8329"));
        assert!(is_ip("fe80::1"));
        assert!(is_ip("1:2:3:4:5:6:7:8"));
        assert!(!is_ip("1:2:3:4:5:6:7:8:9"));
        assert!(!is_ip(":::" ));
        assert!(!is_ip("g:2:3:4:5:6:7:8"));
        assert!(!is_ip("::1:2:3:4:5:6:7:8:9"));
    }

    #[test]
    fn export_navigation_detection() {
        assert!(is_user_initiated_session_export_navigation(
            "GET",
            Some("navigate"),
            Some("document"),
            Some("?1"),
            "/api/sessions/abc123/export",
        ));
        assert!(!is_user_initiated_session_export_navigation(
            "GET",
            Some("navigate"),
            Some("document"),
            Some("?1"),
            "/api/sessions/abc123/messages",
        ));
        assert!(!is_user_initiated_session_export_navigation(
            "POST",
            Some("navigate"),
            Some("document"),
            Some("?1"),
            "/api/sessions/abc123/export",
        ));
        assert!(!is_user_initiated_session_export_navigation(
            "GET",
            None,
            Some("document"),
            Some("?1"),
            "/api/sessions/abc123/export",
        ));
    }

    #[test]
    fn host_allowed_with_config() {
        let empty: Vec<String> = vec![];
        assert!(is_api_request_host_allowed_with(Some("localhost"), &empty));
        assert!(is_api_request_host_allowed_with(Some("api.localhost"), &empty));
        assert!(is_api_request_host_allowed_with(Some("127.0.0.1"), &empty));
        assert!(is_api_request_host_allowed_with(Some("192.168.1.5"), &empty));
        assert!(is_api_request_host_allowed_with(Some("[::1]"), &empty));
        assert!(!is_api_request_host_allowed_with(Some("example.com"), &empty));
        assert!(!is_api_request_host_allowed_with(None, &empty));
        assert!(!is_api_request_host_allowed_with(Some(""), &empty));
        assert!(!is_api_request_host_allowed_with(Some("evil.com"), &empty));

        let cfg = vec!["pi.example.com".to_string()];
        assert!(is_api_request_host_allowed_with(Some("pi.example.com"), &cfg));
        assert!(is_api_request_host_allowed_with(Some("pi.example.com:8080"), &cfg));
        assert!(!is_api_request_host_allowed_with(Some("sub.pi.example.com"), &cfg));
    }

    #[test]
    fn origin_allowed_cases() {
        let url = "http://example.com/api/foo";
        let host = Some("example.com");

        // 无 origin:浏览器同站/非浏览器放行(除非 cross-site)
        assert!(is_api_request_origin_allowed(None, Some("same-origin"), url, host));
        assert!(is_api_request_origin_allowed(None, Some("same-site"), url, host));
        assert!(is_api_request_origin_allowed(None, None, url, host));
        assert!(!is_api_request_origin_allowed(None, Some("cross-site"), url, host));

        // 同 origin
        assert!(is_api_request_origin_allowed(Some("http://example.com"), None, url, host));
        // 不同 origin → 拒绝
        assert!(!is_api_request_origin_allowed(Some("http://evil.com"), None, url, host));
        // 仅端口不同 → 拒绝
        assert!(!is_api_request_origin_allowed(Some("http://example.com:8080"), None, url, host));
        // 非法 origin → 拒绝
        assert!(!is_api_request_origin_allowed(Some("not a url"), None, url, host));
        // Host 头带端口时 origin 按端口比较
        assert!(is_api_request_origin_allowed(
            Some("http://example.com:8080"),
            None,
            "http://example.com/api/foo",
            Some("example.com:8080"),
        ));
        // Host 头缺失 → requestOrigin 为空 → 拒绝
        assert!(!is_api_request_origin_allowed(Some("http://example.com"), None, url, None));
    }

    #[test]
    fn request_allowed_integration() {
        let url = "http://example.com/api/foo";
        let host = Some("localhost");
        // Host 未过 → 拒绝
        assert!(!is_api_request_allowed(
            "GET", Some("example.com"), Some("http://localhost"), Some("navigate"),
            Some("document"), None, Some("?1"), url,
        ));
        // Host 过 + 无 origin 头 → 放行
        assert!(is_api_request_allowed(
            "GET", host, None, None, None, None, None, url,
        ));
        // Host 过 + 非法跨站 origin → 拒绝
        assert!(!is_api_request_allowed(
            "GET", host, Some("http://evil.com"), None, None, Some("cross-site"), None, url,
        ));
        // 会话导出导航豁免(即使有 origin 头)
        assert!(is_api_request_allowed(
            "GET", host, Some("http://evil.com"), Some("navigate"),
            Some("document"), None, Some("?1"),
            "http://example.com/api/sessions/abc/export",
        ));
    }

    #[test]
    fn json_content_type_cases() {
        assert!(has_json_content_type(Some("application/json")));
        assert!(has_json_content_type(Some("application/problem+json")));
        assert!(has_json_content_type(Some(" Application/JSON; charset=utf-8 ")));
        assert!(has_json_content_type(Some("application/json; charset=utf-8")));
        assert!(!has_json_content_type(Some("text/plain")));
        assert!(!has_json_content_type(Some("application/jsonx")));
        assert!(!has_json_content_type(Some("application/x-json")));
        assert!(!has_json_content_type(None));
    }

    #[test]
    fn environment_hosts() {
        // 未设置时为空(测试环境不依赖真实环境变量)
        let configured = configured_hostnames_from_environment();
        let envs = ["PI_WEB_HOSTNAME", "PI_WEB_ALLOWED_HOSTS"];
        let unset = envs
            .iter()
            .all(|k| std::env::var(k).is_err() || std::env::var(k).unwrap().is_empty());
        if unset {
            assert!(configured.is_empty());
        }
        let _ = configured;
    }
}
