//! 对齐 `lib/bounded-form-data.ts`。请求体大小上限约束。
//!
//! 上游语义:`Content-Length` 声明超限直接抛错;否则流式读取并统计字节,
//! 超限时取消读取并抛 [`RequestBodyTooLarge`]。multipart 解析本身留给调用方
//! (此处只做线上字节预算,这是安全关键部分)。

use std::fmt;

use futures::{Stream, StreamExt};

/// 对齐 `RequestBodyTooLargeError`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestBodyTooLarge;

impl fmt::Display for RequestBodyTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Request body exceeds the allowed size")
    }
}

impl std::error::Error for RequestBodyTooLarge {}

/// 对齐 `declaredContentLength`:仅接受纯数字,且为安全整数。
pub fn declared_content_length(value: Option<&str>) -> Option<u64> {
    let value = value?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|length| {
        // 对齐 `Number.isSafeInteger`(u64 在 JS 安全整数范围内)
        *length <= 9_007_199_254_740_991
    })
}

/// 对齐 `parseFormDataWithinLimit` 的声明长度预检:
/// `declared !== null && declared > maxBytes` → 抛错。
pub fn check_declared_content_length(
    content_length: Option<&str>,
    max_bytes: u64,
) -> Result<(), RequestBodyTooLarge> {
    match declared_content_length(content_length) {
        Some(declared) if declared > max_bytes => Err(RequestBodyTooLarge),
        _ => Ok(()),
    }
}

/// 对齐流式字节预算:`size + chunkLen > maxBytes` → 停止并抛错。
///
/// `collect_body_within_limit` 同步版:消费 chunk 迭代器并统计总字节。
/// 返回累计的完整 body(供调用方做 multipart 解析)。
pub fn collect_body_within_limit<I>(
    chunks: I,
    max_bytes: u64,
) -> Result<Vec<u8>, RequestBodyTooLarge>
where
    I: IntoIterator<Item = Vec<u8>>,
{
    let mut body = Vec::new();
    for chunk in chunks {
        if body.len() + chunk.len() > max_bytes as usize {
            return Err(RequestBodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// 对齐 `parseFormDataWithinLimit` 的 async 版。
///
/// `stream` 对应 `request.body.getReader()` 的读取序列;超限时等价
/// `reader.cancel()`(不再消费剩余 chunk)并抛错。
pub async fn parse_form_data_within_limit<S, E>(
    content_length: Option<&str>,
    stream: S,
    max_bytes: u64,
) -> Result<Vec<u8>, BodyLimitError<E>>
where
    S: Stream<Item = Result<Vec<u8>, E>> + Unpin,
    E: std::error::Error + Send + 'static,
{
    check_declared_content_length(content_length, max_bytes).map_err(BodyLimitError::TooLarge)?;

    let mut body = Vec::new();
    let mut too_large = None;
    let mut stream = stream;
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if body.len() + chunk.len() > max_bytes as usize {
                    too_large = Some(RequestBodyTooLarge);
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Some(Err(e)) => return Err(BodyLimitError::Read(e)),
            None => break,
        }
    }
    match too_large {
        Some(too_large) => Err(BodyLimitError::TooLarge(too_large)),
        None => Ok(body),
    }
}

/// async 版的错误:尺寸超限或底层读取错误。
#[derive(Debug)]
pub enum BodyLimitError<E> {
    TooLarge(RequestBodyTooLarge),
    Read(E),
}

impl<E> fmt::Display for BodyLimitError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BodyLimitError::TooLarge(e) => e.fmt(f),
            BodyLimitError::Read(e) => e.fmt(f),
        }
    }
}

impl<E> std::error::Error for BodyLimitError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BodyLimitError::TooLarge(_) => None,
            BodyLimitError::Read(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::iter;

    #[test]
    fn declared_length_parsing() {
        assert_eq!(declared_content_length(Some("1024")), Some(1024));
        assert_eq!(declared_content_length(Some("0")), Some(0));
        assert_eq!(declared_content_length(Some("")), None);
        assert_eq!(declared_content_length(Some("12a")), None);
        assert_eq!(declared_content_length(Some("-1")), None);
        assert_eq!(declared_content_length(Some("1.5")), None);
        assert_eq!(declared_content_length(Some(" 1024")), None);
        assert_eq!(declared_content_length(None), None);
        // 超出 JS safe integer 视为无效
        assert_eq!(declared_content_length(Some("9007199254740992")), None);
        assert_eq!(
            declared_content_length(Some("9007199254740991")),
            Some(9_007_199_254_740_991)
        );
    }

    #[test]
    fn declared_check() {
        assert_eq!(check_declared_content_length(Some("500"), 1000), Ok(()));
        assert_eq!(check_declared_content_length(Some("1000"), 1000), Ok(()));
        assert_eq!(
            check_declared_content_length(Some("1001"), 1000),
            Err(RequestBodyTooLarge)
        );
        // 无效声明 → 不拦截(交由流式预算处理)
        assert_eq!(check_declared_content_length(None, 1000), Ok(()));
        assert_eq!(check_declared_content_length(Some("abc"), 1000), Ok(()));
    }

    #[test]
    fn sync_collect_within_limit() {
        let chunks = vec![b"hello ".to_vec(), b"world".to_vec()];
        let body = collect_body_within_limit(chunks, 100).unwrap();
        assert_eq!(body, b"hello world");

        // 恰好等于上限 → 通过
        let chunks = vec![b"a".to_vec(), b"b".to_vec()];
        assert!(collect_body_within_limit(chunks, 2).is_ok());

        // 超限 → 抛错,不再消费后续 chunk
        let mut consumed = 0usize;
        let chunks = (0..3).map(|i| {
            consumed += 1;
            vec![b'x'; 10]
        });
        assert_eq!(
            collect_body_within_limit(chunks, 25),
            Err(RequestBodyTooLarge)
        );
        assert_eq!(consumed, 3); // 同步版仍消费剩余项(无取消语义)
    }

    #[tokio::test]
    async fn async_collect_within_limit() {
        let stream = iter(vec![
            Ok::<_, std::io::Error>(b"hello ".to_vec()),
            Ok(b"world".to_vec()),
        ]);
        let body = parse_form_data_within_limit(Some("11"), stream, 100)
            .await
            .unwrap();
        assert_eq!(body, b"hello world");

        // 声明长度超限 → 直接抛错(不读流)
        let stream = iter(vec![Ok::<_, std::io::Error>(b"x".to_vec())]);
        let err = parse_form_data_within_limit(Some("200"), stream, 100)
            .await
            .unwrap_err();
        assert!(matches!(err, BodyLimitError::TooLarge(_)));

        // 流式超限(声明缺失)→ 抛错
        let stream = iter(vec![
            Ok::<_, std::io::Error>(b"x".to_vec()),
            Ok(vec![b'y'; 200]),
        ]);
        let err = parse_form_data_within_limit(None, stream, 100)
            .await
            .unwrap_err();
        assert!(matches!(err, BodyLimitError::TooLarge(_)));

        // 读取错误透传
        let stream = iter(vec![Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "boom",
        ))]);
        let err = parse_form_data_within_limit(Some("1"), stream, 100)
            .await
            .unwrap_err();
        assert!(matches!(err, BodyLimitError::Read(_)));
    }
}
