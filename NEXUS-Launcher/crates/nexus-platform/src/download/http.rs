//! HTTP transport for downloads.
//!
//! Redirects are followed manually so every hop is re-validated by the same
//! [`UrlPolicy`] that protects the rest of the launcher. DNS answers are pinned
//! in the per-hop reqwest client, mirroring `SafeFetcher`.

use std::time::Duration;

use nexus_core::NexusError;
use reqwest::Response;
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_RANGE, ETAG, HeaderMap, HeaderName, IF_RANGE, LAST_MODIFIED, LOCATION,
    RANGE, RETRY_AFTER,
};

use crate::security::net::UrlPolicy;
use crate::security::net::url::UrlPurpose;

#[derive(Debug, Clone)]
pub(crate) struct DownloadHttpConfig {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub max_redirects: u8,
    pub user_agent: String,
}

#[derive(Debug)]
pub(crate) struct RequestError {
    pub error: NexusError,
    pub retryable: bool,
}

impl RequestError {
    fn permanent(error: NexusError) -> Self {
        Self {
            error,
            retryable: false,
        }
    }

    fn transient(error: NexusError) -> Self {
        Self {
            error,
            retryable: true,
        }
    }
}

pub(crate) async fn send_download_request(
    policy: &UrlPolicy,
    config: &DownloadHttpConfig,
    raw_url: &str,
    offset: u64,
    if_range: Option<&str>,
) -> Result<Response, RequestError> {
    let mut current = policy
        .validate(raw_url, UrlPurpose::Download)
        .map_err(RequestError::permanent)?;
    let mut redirects = 0u8;

    loop {
        let addresses = policy
            .resolve(&current, UrlPurpose::Download)
            .await
            .map_err(RequestError::permanent)?;
        let host = current
            .host_str()
            .ok_or_else(|| RequestError::permanent(NexusError::url_invalid("URL has no host")))?
            .to_owned();

        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .user_agent(config.user_agent.clone())
            .danger_accept_invalid_certs(false);

        if current
            .host()
            .is_some_and(|host| matches!(host, url::Host::Domain(_)))
        {
            builder = builder.resolve_to_addrs(&host, &addresses);
        } else if let Some(first) = addresses.first() {
            builder = builder.resolve(&host, *first);
        }

        let client = builder.build().map_err(|error| {
            RequestError::permanent(NexusError::internal(format!(
                "cannot build HTTP client: {error}"
            )))
        })?;

        let mut request = client
            .get(current.clone())
            .header(ACCEPT_ENCODING, "identity");
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        if let Some(validator) = if_range {
            request = request.header(IF_RANGE, validator);
        }

        let response = request.send().await.map_err(map_reqwest_error)?;
        if !response.status().is_redirection() {
            return Ok(response);
        }

        if redirects >= config.max_redirects.min(policy.max_redirects) {
            return Err(RequestError::permanent(NexusError::redirect_rejected(
                "redirect limit exceeded",
            )));
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                RequestError::permanent(NexusError::redirect_rejected(
                    "redirect response has no Location header",
                ))
            })?;
        let mut next = current.join(location).map_err(|error| {
            RequestError::permanent(NexusError::redirect_rejected(error.to_string()))
        })?;
        next.set_fragment(None);
        policy
            .validate_redirect(&current, &next, UrlPurpose::Download)
            .map_err(RequestError::permanent)?;
        policy
            .validate(next.as_str(), UrlPurpose::Download)
            .map_err(RequestError::permanent)?;
        current = next;
        redirects += 1;
    }
}

fn map_reqwest_error(error: reqwest::Error) -> RequestError {
    if error.is_timeout() {
        return RequestError::transient(NexusError::network_timeout(format!(
            "download request timed out: {error}"
        )));
    }
    if error.is_connect() || error.is_request() || error.is_body() {
        return RequestError::transient(NexusError::internal(format!(
            "download request failed: {error}"
        )));
    }
    RequestError::permanent(NexusError::internal(format!(
        "download request failed: {error}"
    )))
}

pub(crate) fn header_text(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

pub(crate) fn etag(response: &Response) -> Option<String> {
    header_text(response.headers(), ETAG)
}

pub(crate) fn last_modified(response: &Response) -> Option<String> {
    header_text(response.headers(), LAST_MODIFIED)
}

pub(crate) fn content_length(response: &Response) -> Option<u64> {
    response.content_length()
}

pub(crate) fn retry_after(response: &Response) -> Option<Duration> {
    let raw = header_text(response.headers(), RETRY_AFTER)?;
    if let Ok(seconds) = raw.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(300)));
    }
    None
}

/// Chooses a validator suitable for `If-Range`.
///
/// Weak ETags (`W/"..."`) are not valid for byte-range conditional requests, so
/// `Last-Modified` is preferred in that case.
pub(crate) fn if_range_value(etag: Option<&str>, last_modified: Option<&str>) -> Option<String> {
    match etag {
        Some(value) if !value.trim_start().starts_with("W/") => Some(value.to_owned()),
        _ => last_modified.map(ToOwned::to_owned),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total: Option<u64>,
}

/// Parses `Content-Range: bytes START-END/TOTAL`. `TOTAL` may be `*`.
pub(crate) fn parse_content_range(raw: &str) -> Option<ContentRange> {
    let (unit, value) = raw.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim().parse::<u64>().ok()?;
    let end = end.trim().parse::<u64>().ok()?;
    if end < start {
        return None;
    }
    let total = match total.trim() {
        "*" => None,
        raw => Some(raw.parse::<u64>().ok()?),
    };
    if total.is_some_and(|total| end >= total) {
        return None;
    }
    Some(ContentRange { start, end, total })
}

pub(crate) fn content_range(response: &Response) -> Option<ContentRange> {
    header_text(response.headers(), CONTENT_RANGE)
        .as_deref()
        .and_then(parse_content_range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_ranges() {
        assert_eq!(
            parse_content_range("bytes 0-499/1234"),
            Some(ContentRange {
                start: 0,
                end: 499,
                total: Some(1234)
            })
        );
        assert_eq!(
            parse_content_range("bytes 500-999/*"),
            Some(ContentRange {
                start: 500,
                end: 999,
                total: None
            })
        );
        assert_eq!(parse_content_range("items 0-1/2"), None);
        assert_eq!(parse_content_range("bytes 10-5/20"), None);
        assert_eq!(parse_content_range("garbage"), None);
    }

    #[test]
    fn weak_etag_falls_back_to_last_modified() {
        assert_eq!(
            if_range_value(Some("\"abc\""), Some("yesterday")).as_deref(),
            Some("\"abc\"")
        );
        assert_eq!(
            if_range_value(Some("W/\"abc\""), Some("yesterday")).as_deref(),
            Some("yesterday")
        );
        assert_eq!(if_range_value(None, None), None);
    }
}
