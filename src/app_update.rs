//! 对齐 `lib/app-update.ts`。Pi Web 版本比较 + release URL 构造。纯逻辑,无 IO。

use regex::Regex;
use std::sync::LazyLock;

/// 对齐 `STABLE_VERSION_PATTERN = /^(\d+)\.(\d+)\.(\d+)$/`。
static STABLE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)\.(\d+)\.(\d+)$").expect("valid stable version regex"));

/// JS `Number.MAX_SAFE_INTEGER` = 2^53 - 1。
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// 对齐 `parseStableVersion`。
///
/// 三个捕获组各自经 `Number()` 解析后必须通过 `Number.isSafeInteger`
/// (有限、无小数部分、绝对值 ≤ 2^53-1),否则返回 `None`。
/// 返回 `i64`(安全整数范围内精确可表)。
fn parse_stable_version(version: &str) -> Option<[i64; 3]> {
    let caps = STABLE_VERSION_RE.captures(version)?;
    let mut parts = [0i64; 3];
    for (i, grp) in [1usize, 2, 3].iter().enumerate() {
        let raw = caps.get(*grp)?.as_str();
        // 对齐 `match.slice(1).map(Number)`:纯数字串 → f64
        let n: f64 = raw.parse::<f64>().ok()?;
        // 对齐 `Number.isSafeInteger`(版本号非负,abs 即 n)
        if !n.is_finite() || n.fract() != 0.0 || n > MAX_SAFE_INTEGER {
            return None;
        }
        parts[i] = n as i64;
    }
    Some(parts)
}

/// 对齐 `isNewerStableVersion(candidate, current)`。
///
/// 任一版本非稳定格式返回 `false`;否则按 major→minor→patch 逐段比较,
/// 第一处不同决定新旧,全部相同返回 `false`(不把相等当更新)。
pub fn is_newer_stable_version(candidate: &str, current: &str) -> bool {
    let (Some(candidate_parts), Some(current_parts)) = (
        parse_stable_version(candidate),
        parse_stable_version(current),
    ) else {
        return false;
    };

    for index in 0..candidate_parts.len() {
        if candidate_parts[index] != current_parts[index] {
            return candidate_parts[index] > current_parts[index];
        }
    }
    false
}

/// 对齐 `getPiWebReleaseUrl(version)`。
///
/// 仅稳定版本返回 `https://github.com/agegr/pi-web/releases/tag/v<version>`;
/// 非稳定(含预发布/非法)返回 `None`。URL 用原始字符串,不经过解析值。
pub fn get_pi_web_release_url(version: &str) -> Option<String> {
    parse_stable_version(version)?;
    Some(format!(
        "https://github.com/agegr/pi-web/releases/tag/v{version}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_newer_stable_pi_web_versions() {
        assert!(is_newer_stable_version("0.8.8", "0.8.7"));
        assert!(is_newer_stable_version("0.9.0", "0.8.7"));
        assert!(is_newer_stable_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn does_not_report_equal_older_or_unsupported_as_updates() {
        assert!(!is_newer_stable_version("0.8.7", "0.8.7"));
        assert!(!is_newer_stable_version("0.8.6", "0.8.7"));
        assert!(!is_newer_stable_version("0.8.8-beta.1", "0.8.7"));
        assert!(!is_newer_stable_version("invalid", "0.8.7"));
    }

    #[test]
    fn builds_a_release_url_only_for_stable_versions() {
        assert_eq!(
            get_pi_web_release_url("0.8.8").as_deref(),
            Some("https://github.com/agegr/pi-web/releases/tag/v0.8.8")
        );
        assert_eq!(get_pi_web_release_url("0.8.8-beta.1"), None);
    }
}
