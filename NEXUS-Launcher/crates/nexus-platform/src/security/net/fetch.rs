//! A small HTTP client with explicit redirect, timeout and size handling.
//!
//! Automatic redirect following is disabled. Every hop is validated and its
//! DNS answers are pinned in the client, so a hostname cannot resolve to a
//! public address for the check and a private address for the connection.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;

use futures_util::StreamExt;
use nexus_core::NexusError;
use reqwest::Response;
use reqwest::header::LOCATION;
use sha2::{Digest, Sha256};
use url::Url;

use super::url::{UrlPolicy, UrlPurpose};
use crate::security::fs::checksum::{FileDigest, Sha256Digest};
use crate::security::fs::{ExpectedFile, SafeFs, SafeRelativePath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchLimits {
    pub max_response_bytes: u64,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_redirects: u8,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: 64 * 1024 * 1024,
            connect_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(120),
            max_redirects: 3,
        }
    }
}

#[derive(Debug)]
pub struct SafeResponse {
    pub final_url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SafeFetcher {
    policy: UrlPolicy,
    limits: FetchLimits,
    user_agent: String,
}

impl SafeFetcher {
    pub fn new(policy: UrlPolicy, limits: FetchLimits) -> Self {
        Self {
            policy,
            limits,
            user_agent: format!("NEXUS-Launcher/{}", env!("CARGO_PKG_VERSION")),
        }
    }

    pub fn policy(&self) -> &UrlPolicy {
        &self.policy
    }

    pub fn limits(&self) -> &FetchLimits {
        &self.limits
    }

    /// Fetches a small response into memory with a hard size limit.
    pub async fn fetch_bytes(
        &self,
        raw_url: &str,
        purpose: UrlPurpose,
    ) -> Result<SafeResponse, NexusError> {
        let (final_url, response) = self.send(raw_url, purpose).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(NexusError::internal(format!(
                "HTTP {} from {}",
                status.as_u16(),
                final_url.host_str().unwrap_or("<unknown>")
            )));
        }

        if let Some(length) = response.content_length()
            && length > self.limits.max_response_bytes
        {
            return Err(NexusError::response_too_large(format!(
                "Content-Length {length} exceeds limit {}",
                self.limits.max_response_bytes
            )));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);

        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                if error.is_timeout() {
                    NexusError::network_timeout(format!("response body timed out: {error}"))
                } else {
                    NexusError::internal(format!("response body failed: {error}"))
                }
            })?;
            if body.len() as u64 + chunk.len() as u64 > self.limits.max_response_bytes {
                return Err(NexusError::response_too_large(format!(
                    "response exceeds limit {}",
                    self.limits.max_response_bytes
                )));
            }
            body.extend_from_slice(&chunk);
        }

        Ok(SafeResponse {
            final_url,
            status: status.as_u16(),
            content_type,
            body,
        })
    }

    /// Downloads into a staging file, verifies it against `expected` (if any),
    /// then atomically installs it at `destination`.
    #[allow(clippy::too_many_arguments)]
    pub async fn download_to(
        &self,
        raw_url: &str,
        purpose: UrlPurpose,
        destination_root: &SafeFs,
        destination: &SafeRelativePath,
        max_size: u64,
        expected: Option<&ExpectedFile>,
        overwrite: bool,
    ) -> Result<FileDigest, NexusError> {
        let limit = max_size.min(self.limits.max_response_bytes);
        let (final_url, response) = self.send(raw_url, purpose).await?;
        if !response.status().is_success() {
            return Err(NexusError::internal(format!(
                "HTTP {} from {}",
                response.status().as_u16(),
                final_url.host_str().unwrap_or("<unknown>")
            )));
        }
        if let Some(length) = response.content_length()
            && length > limit
        {
            return Err(NexusError::file_too_large(format!(
                "Content-Length {length} exceeds limit {limit}"
            )));
        }

        let staging = destination_root.create_staging("download")?;
        let staging_fs = staging.filesystem()?;
        let staged = SafeRelativePath::parse("payload.bin")?;
        let staged_path = staging_fs.resolve(&staged)?.absolute;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_path)
            .map_err(|error| {
                NexusError::internal(format!(
                    "cannot create staged download {}: {error}",
                    staged_path.display()
                ))
            })?;

        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                if error.is_timeout() {
                    NexusError::network_timeout(format!("download timed out: {error}"))
                } else {
                    NexusError::internal(format!("download failed: {error}"))
                }
            })?;
            total = total
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| NexusError::file_too_large("download size overflow"))?;
            if total > limit {
                return Err(NexusError::file_too_large(format!(
                    "download exceeds limit {limit} bytes"
                )));
            }
            file.write_all(&chunk)
                .map_err(|error| NexusError::internal(format!("cannot write download: {error}")))?;
            hasher.update(&chunk);
        }
        file.sync_all()
            .map_err(|error| NexusError::internal(format!("cannot flush download: {error}")))?;
        drop(file);

        let digest = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        let staged_digest = FileDigest {
            sha256: Sha256Digest::from_bytes(bytes),
            size: total,
        };

        if let Some(expected) = expected {
            staged_digest.verify(expected)?;
        }

        let expected_installed = ExpectedFile::new(staged_digest.sha256, staged_digest.size);
        staging.commit_file(
            &staged,
            destination_root,
            destination,
            &expected_installed,
            limit,
            overwrite,
        )
    }

    async fn send(
        &self,
        raw_url: &str,
        purpose: UrlPurpose,
    ) -> Result<(Url, Response), NexusError> {
        let mut current = self.policy.validate(raw_url, purpose)?;
        let mut redirects = 0u8;

        loop {
            let response = self.send_once(&current, purpose).await?;
            let status = response.status();

            if !status.is_redirection() {
                return Ok((current, response));
            }

            if redirects >= self.limits.max_redirects.min(self.policy.max_redirects) {
                return Err(NexusError::redirect_rejected(format!(
                    "redirect limit exceeded at {}",
                    current.host_str().unwrap_or("<unknown>")
                )));
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    NexusError::redirect_rejected("redirect response has no Location header")
                })?;
            let mut next = current
                .join(location)
                .map_err(|error| NexusError::redirect_rejected(error.to_string()))?;
            next.set_fragment(None);
            self.policy.validate_redirect(&current, &next, purpose)?;
            // Apply the same syntax checks (credentials, port, scheme, ...).
            self.policy.validate(next.as_str(), purpose)?;

            current = next;
            redirects += 1;
        }
    }

    async fn send_once(&self, url: &Url, purpose: UrlPurpose) -> Result<Response, NexusError> {
        let host = url
            .host_str()
            .ok_or_else(|| NexusError::url_invalid("URL has no host"))?;
        let addresses = self.policy.resolve(url, purpose).await?;

        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(self.limits.connect_timeout)
            .timeout(self.limits.request_timeout)
            .user_agent(self.user_agent.clone())
            .danger_accept_invalid_certs(false);

        if !url
            .host()
            .is_some_and(|host| matches!(host, url::Host::Domain(_)))
        {
            if let Some(first) = addresses.first() {
                builder = builder.resolve(host, *first);
            }
        } else {
            builder = builder.resolve_to_addrs(host, &addresses);
        }

        let client = builder
            .build()
            .map_err(|error| NexusError::internal(format!("cannot build HTTP client: {error}")))?;

        client.get(url.clone()).send().await.map_err(|error| {
            if error.is_timeout() {
                NexusError::network_timeout(format!(
                    "request to {} timed out",
                    url.host_str().unwrap_or("<unknown>")
                ))
            } else {
                NexusError::internal(format!(
                    "request to {} failed: {error}",
                    url.host_str().unwrap_or("<unknown>")
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn local_policy() -> UrlPolicy {
        UrlPolicy::nexus_source(true, true, true)
    }

    async fn serve(responses: Vec<String>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 4096];
                let _ = socket.read(&mut buffer).await;
                socket.write_all(response.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            }
        });
        (address, handle)
    }

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn follows_redirect_after_revalidating_it() {
        let (address, server) = serve(vec![
            response("302 Found", "Location: /final\r\n", ""),
            response("200 OK", "", "payload"),
        ])
        .await;
        let fetcher = SafeFetcher::new(local_policy(), FetchLimits::default());
        let result = fetcher
            .fetch_bytes(&format!("http://{address}/start"), UrlPurpose::Download)
            .await
            .unwrap();
        assert_eq!(result.body, b"payload");
        assert_eq!(result.final_url.path(), "/final");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_redirect_loop_at_limit() {
        let (address, server) = serve(vec![
            response("302 Found", "Location: /again\r\n", ""),
            response("302 Found", "Location: /again\r\n", ""),
        ])
        .await;
        let limits = FetchLimits {
            max_redirects: 1,
            ..FetchLimits::default()
        };
        let fetcher = SafeFetcher::new(local_policy(), limits);
        let error = fetcher
            .fetch_bytes(&format!("http://{address}/start"), UrlPurpose::Download)
            .await
            .unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::RedirectRejected);
        server.abort();
    }

    #[tokio::test]
    async fn enforces_response_size_limit_while_streaming() {
        let (address, server) = serve(vec![response("200 OK", "", &"x".repeat(4096))]).await;
        let limits = FetchLimits {
            max_response_bytes: 10,
            ..FetchLimits::default()
        };
        let fetcher = SafeFetcher::new(local_policy(), limits);
        let error = fetcher
            .fetch_bytes(&format!("http://{address}/big"), UrlPurpose::Download)
            .await
            .unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::ResponseTooLarge);
        server.abort();
    }

    #[tokio::test]
    async fn default_policy_refuses_plain_http_before_connecting() {
        let fetcher = SafeFetcher::new(UrlPolicy::strict(), FetchLimits::default());
        let error = fetcher
            .fetch_bytes("http://example.com/", UrlPurpose::ExternalApi)
            .await
            .unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::TlsRequired);
    }

    #[tokio::test]
    async fn blocks_cloud_metadata_address() {
        let fetcher = SafeFetcher::new(local_policy(), FetchLimits::default());
        let error = fetcher
            .fetch_bytes(
                "http://169.254.169.254/latest/meta-data",
                UrlPurpose::Download,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::UrlBlocked);
    }
}
