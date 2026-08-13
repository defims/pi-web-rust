//! 对齐 `lib/session-timing.ts`。从 session log 估算活跃时间。

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TimingEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub timestamp: String,
    pub message: Option<TimingMessage>,
}

#[derive(Debug, Deserialize)]
pub struct TimingMessage {
    pub role: Option<String>,
}

fn is_timing_entry(entry_type: &str) -> bool {
    matches!(
        entry_type,
        "message" | "compaction" | "branch_summary" | "custom_message"
    )
}

fn parse_timestamp_ms(ts: &str) -> Option<i64> {
    // 支持 ISO 8601 和 Unix epoch 毫秒
    if let Ok(n) = ts.parse::<i64>() {
        return Some(n);
    }
    // 简化:尝试用 chrono(如果有)或手写。
    // pi session timestamp 是 ISO 8601 格式。
    // 这里用一个简单的 RFC3339 解析器(避免 chrono 依赖)。
    // 如果解析失败返回 None。
    parse_rfc3339_to_millis(ts)
}

/// RFC3339 / ISO 8601 → epoch ms。
fn parse_rfc3339_to_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .or_else(|_| chrono::DateTime::parse_from_str(s, "%+"))
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 对齐 `computeSessionTotalActiveMs`。
pub fn compute_session_total_active_ms(entries: &[TimingEntry]) -> i64 {
    let mut total_active_ms: i64 = 0;
    let mut previous_timestamp: Option<i64> = None;

    for entry in entries {
        if !is_timing_entry(&entry.entry_type) {
            continue;
        }
        let Some(timestamp) = parse_timestamp_ms(&entry.timestamp) else {
            continue;
        };

        let role = if entry.entry_type == "message" {
            entry.message.as_ref().and_then(|m| m.role.as_deref())
        } else {
            None
        };

        if role == Some("user") || role == Some("bashExecution") {
            previous_timestamp = Some(timestamp);
            continue;
        }

        if let Some(prev) = previous_timestamp {
            if timestamp > prev {
                total_active_ms += timestamp - prev;
            }
        }
        previous_timestamp = Some(timestamp);
    }

    total_active_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        assert_eq!(compute_session_total_active_ms(&[]), 0);
    }

    #[test]
    fn with_epoch_timestamps() {
        let entries = vec![
            TimingEntry {
                entry_type: "message".into(),
                timestamp: "1000".into(),
                message: Some(TimingMessage {
                    role: Some("user".into()),
                }),
            },
            TimingEntry {
                entry_type: "message".into(),
                timestamp: "3000".into(),
                message: Some(TimingMessage {
                    role: Some("assistant".into()),
                }),
            },
            TimingEntry {
                entry_type: "message".into(),
                timestamp: "8000".into(),
                message: Some(TimingMessage {
                    role: Some("user".into()),
                }),
            },
            TimingEntry {
                entry_type: "message".into(),
                timestamp: "9000".into(),
                message: Some(TimingMessage {
                    role: Some("assistant".into()),
                }),
            },
        ];
        // 3000-1000 = 2000, 9000-8000 = 1000 → total 3000
        assert_eq!(compute_session_total_active_ms(&entries), 3000);
    }
}
