//! URL syntax and SSRF policy.
//!
//! The policy is applied twice: once to the URL as written by the user and
//! again to every redirect target. Before a connection is opened, hostnames are
//! resolved and *all* addresses must pass the policy. The resolved addresses
//! are then pinned in the HTTP client, which prevents DNS rebinding between the
//! check and the request.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use nexus_core::NexusError;
use url::{Host, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlPurpose {
    /// Public provider API (Mojang, Modrinth, ...).
    ExternalApi,
    /// User-configured NEXUS Workspace server.
    NexusSource,
    /// File download URL from an already trusted manifest.
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPolicy {
    /// HTTPS is mandatory unless `allow_insecure_http` was explicitly enabled.
    pub require_https: bool,
    /// Explicit opt-in for plain HTTP. Intended for LAN/self-hosted servers.
    pub allow_insecure_http: bool,
    /// Explicit opt-in for RFC1918/CGNAT/ULA addresses.
    pub allow_private_network: bool,
    /// Explicit opt-in for loopback addresses.
    pub allow_loopback: bool,
    /// `None` means any port is allowed. A list restricts the port.
    pub allowed_ports: Option<Vec<u16>>,
    pub max_url_length: usize,
    pub max_redirects: u8,
    pub dns_timeout: Duration,
}

impl Default for UrlPolicy {
    fn default() -> Self {
        Self::strict()
    }
}

impl UrlPolicy {
    /// Default for external providers: HTTPS only, public addresses only.
    pub fn strict() -> Self {
        Self {
            require_https: true,
            allow_insecure_http: false,
            allow_private_network: false,
            allow_loopback: false,
            allowed_ports: Some(vec![443]),
            max_url_length: 2048,
            max_redirects: 3,
            dns_timeout: Duration::from_secs(10),
        }
    }

    /// Policy for a user-configured NEXUS Workspace source.
    ///
    /// The exceptions are explicit flags, never an implicit consequence of
    /// using a private host name. This keeps self-hosted deployments usable
    /// without silently disabling SSRF protection for every source.
    pub fn nexus_source(
        allow_private_network: bool,
        allow_loopback: bool,
        allow_insecure_http: bool,
    ) -> Self {
        Self {
            require_https: !allow_insecure_http,
            allow_insecure_http,
            allow_private_network,
            allow_loopback,
            allowed_ports: None,
            max_url_length: 2048,
            max_redirects: 3,
            dns_timeout: Duration::from_secs(10),
        }
    }

    /// Builds the NEXUS source policy from explicit bootstrap configuration.
    /// The default configuration stays strict; self-hosted users must opt in.
    pub fn for_nexus_config(config: &nexus_core::config::SecurityConfig) -> Self {
        Self::nexus_source(
            config.allow_local_nexus_sources,
            config.allow_local_nexus_sources,
            config.allow_insecure_http_nexus_sources,
        )
    }

    pub fn validate(&self, raw: &str, purpose: UrlPurpose) -> Result<Url, NexusError> {
        if raw.is_empty() {
            return Err(NexusError::url_invalid("URL is empty"));
        }
        if raw.len() > self.max_url_length {
            return Err(NexusError::url_invalid("URL is too long"));
        }
        if raw != raw.trim() {
            return Err(NexusError::url_invalid(
                "URL has leading or trailing whitespace",
            ));
        }
        if raw.chars().any(char::is_control) {
            return Err(NexusError::url_invalid("URL contains control characters"));
        }

        let url = Url::parse(raw).map_err(|error| NexusError::url_invalid(error.to_string()))?;

        if !url.username().is_empty() || url.password().is_some() {
            return Err(NexusError::url_blocked(
                "credentials in URLs are not allowed",
            ));
        }
        if url.fragment().is_some() {
            return Err(NexusError::url_invalid("URL fragments are not allowed"));
        }

        match url.scheme() {
            "https" => {}
            "http" => {
                if self.require_https || !self.allow_insecure_http {
                    return Err(NexusError::tls_required(format!("{} must use HTTPS", url)));
                }
            }
            other => {
                return Err(NexusError::url_blocked(format!(
                    "URL scheme {other:?} is not supported"
                )));
            }
        }

        let host = url
            .host()
            .ok_or_else(|| NexusError::url_invalid("URL has no host"))?;
        if let Host::Domain(domain) = host {
            if domain.is_empty() || domain.len() > 253 {
                return Err(NexusError::url_invalid("URL host is invalid"));
            }
            if !self.allow_loopback && is_loopback_name(domain) {
                return Err(NexusError::url_blocked(format!(
                    "loopback host {domain:?} is not allowed"
                )));
            }
        }

        if let Some(ports) = &self.allowed_ports {
            let port = url
                .port_or_known_default()
                .ok_or_else(|| NexusError::url_invalid("URL has no usable port"))?;
            if !ports.contains(&port) {
                return Err(NexusError::url_blocked(format!(
                    "port {port} is not allowed by policy"
                )));
            }
        }

        if matches!(purpose, UrlPurpose::NexusSource) {
            if url.query().is_some() {
                return Err(NexusError::url_invalid(
                    "NEXUS source base URL must not contain a query string",
                ));
            }
            if url.path().contains("//")
                || url.path().contains("/../")
                || url.path().ends_with("/..")
            {
                return Err(NexusError::url_invalid(
                    "NEXUS source path is not normalized",
                ));
            }
        }

        if let Some(ip) = literal_ip(&host) {
            self.check_ip(ip)?;
        }

        Ok(url)
    }

    /// Resolves a URL and validates every address before any connection is
    /// attempted. The result is used to pin DNS in the HTTP client.
    pub async fn resolve(
        &self,
        url: &Url,
        _purpose: UrlPurpose,
    ) -> Result<Vec<SocketAddr>, NexusError> {
        let host = url
            .host()
            .ok_or_else(|| NexusError::url_invalid("URL has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| NexusError::url_invalid("URL has no usable port"))?;

        if let Some(ip) = literal_ip(&host) {
            self.check_ip(ip)?;
            return Ok(vec![SocketAddr::new(ip, port)]);
        }

        let host_name = match host {
            Host::Domain(domain) => domain.to_owned(),
            Host::Ipv4(ip) => ip.to_string(),
            Host::Ipv6(ip) => ip.to_string(),
        };

        let lookup = tokio::time::timeout(
            self.dns_timeout,
            tokio::net::lookup_host((host_name.as_str(), port)),
        )
        .await
        .map_err(|_| {
            NexusError::network_timeout(format!("DNS lookup for {host_name:?} timed out"))
        })?
        .map_err(|error| NexusError::url_invalid(format!("DNS lookup failed: {error}")))?;

        let mut addresses = Vec::new();
        for address in lookup {
            self.check_ip(address.ip())?;
            addresses.push(address);
        }
        if addresses.is_empty() {
            return Err(NexusError::url_invalid(format!(
                "host {host_name:?} did not resolve to any address"
            )));
        }

        addresses.sort_unstable();
        addresses.dedup();
        Ok(addresses)
    }

    pub fn check_ip(&self, ip: IpAddr) -> Result<(), NexusError> {
        let ip = canonical_ip(ip);
        let class = classify(ip);

        if class == IpClass::AlwaysBlocked {
            return Err(NexusError::url_blocked(format!(
                "address {ip} is never allowed"
            )));
        }
        if class == IpClass::Loopback && !self.allow_loopback {
            return Err(NexusError::url_blocked(format!(
                "loopback address {ip} requires an explicit exception"
            )));
        }
        if class == IpClass::Private && !self.allow_private_network {
            return Err(NexusError::url_blocked(format!(
                "private address {ip} requires an explicit exception"
            )));
        }
        Ok(())
    }

    pub fn validate_redirect(
        &self,
        from: &Url,
        to: &Url,
        purpose: UrlPurpose,
    ) -> Result<(), NexusError> {
        if from.scheme() == "https" && to.scheme() != "https" {
            return Err(NexusError::redirect_rejected(
                "redirect downgrades HTTPS to an insecure scheme",
            ));
        }
        if to.scheme() != "https" && to.scheme() != "http" {
            return Err(NexusError::redirect_rejected(format!(
                "redirect to scheme {:?} is not allowed",
                to.scheme()
            )));
        }
        if !to.username().is_empty() || to.password().is_some() {
            return Err(NexusError::redirect_rejected(
                "redirect contains credentials",
            ));
        }
        if to.scheme() == "http" && (self.require_https || !self.allow_insecure_http) {
            return Err(NexusError::tls_required("redirect must use HTTPS"));
        }
        if let Some(ports) = &self.allowed_ports {
            let port = to
                .port_or_known_default()
                .ok_or_else(|| NexusError::redirect_rejected("redirect has no usable port"))?;
            if !ports.contains(&port) {
                return Err(NexusError::redirect_rejected(format!(
                    "redirect port {port} is not allowed"
                )));
            }
        }
        let _ = purpose;
        Ok(())
    }
}

/// A normalized NEXUS Workspace base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NexusSourceUrl {
    url: Url,
}

impl NexusSourceUrl {
    pub fn parse(raw: &str, policy: &UrlPolicy) -> Result<Self, NexusError> {
        let mut url = policy.validate(raw, UrlPurpose::NexusSource)?;
        let trimmed = url.path().trim_end_matches('/').to_owned();
        let path = if trimmed.is_empty() {
            "/".to_owned()
        } else {
            trimmed
        };
        url.set_path(&path);
        url.set_query(None);
        url.set_fragment(None);
        Ok(Self { url })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str().trim_end_matches('/')
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn join(&self, relative: &str) -> Result<Url, NexusError> {
        if relative.starts_with('/') || relative.contains("..") {
            return Err(NexusError::url_invalid(
                "NEXUS API path must be relative and normalized",
            ));
        }
        self.url
            .join(relative)
            .map_err(|error| NexusError::url_invalid(error.to_string()))
    }
}

impl std::fmt::Display for NexusSourceUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn literal_ip(host: &Host<&str>) -> Option<IpAddr> {
    match host {
        Host::Ipv4(ip) => Some(IpAddr::V4(*ip)),
        Host::Ipv6(ip) => Some(IpAddr::V6(*ip)),
        Host::Domain(_) => None,
    }
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            let segments = ip.segments();
            // Deprecated IPv4-compatible form `::a.b.c.d` is still routed as
            // IPv4 by some stacks. Treat it as the embedded IPv4 address.
            if segments[..6].iter().all(|segment| *segment == 0)
                && !ip.is_unspecified()
                && !ip.is_loopback()
            {
                let v4 = Ipv4Addr::new(
                    (segments[6] >> 8) as u8,
                    segments[6] as u8,
                    (segments[7] >> 8) as u8,
                    segments[7] as u8,
                );
                return IpAddr::V4(v4);
            }
            IpAddr::V6(ip)
        }
        other => other,
    }
}

fn is_loopback_name(domain: &str) -> bool {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    domain == "localhost" || domain.ends_with(".localhost")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpClass {
    Public,
    Private,
    Loopback,
    AlwaysBlocked,
}

fn classify(ip: IpAddr) -> IpClass {
    match ip {
        IpAddr::V4(ip) => classify_v4(ip),
        IpAddr::V6(ip) => classify_v6(ip),
    }
}

fn classify_v4(ip: Ipv4Addr) -> IpClass {
    let octets = ip.octets();
    if ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_link_local()
        || ip.is_documentation()
        || octets[0] == 0
        || octets[0] >= 240
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
    {
        return IpClass::AlwaysBlocked;
    }
    if ip.is_loopback() {
        return IpClass::Loopback;
    }
    if ip.is_private()
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 169 && octets[1] == 254)
    {
        return IpClass::Private;
    }
    IpClass::Public
}

fn classify_v6(ip: Ipv6Addr) -> IpClass {
    let segments = ip.segments();
    if ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x2001 && segments[1] == 0x0000)
    {
        return IpClass::AlwaysBlocked;
    }
    // fe80::/10 link-local (includes cloud metadata endpoints)
    if segments[0] & 0xffc0 == 0xfe80 {
        return IpClass::AlwaysBlocked;
    }
    if ip.is_loopback() {
        return IpClass::Loopback;
    }
    // fc00::/7 unique local addresses
    if segments[0] & 0xfe00 == 0xfc00 {
        return IpClass::Private;
    }
    // 6to4 (2002::/16) and NAT64 (64:ff9b::/96) embed an IPv4 destination.
    let embedded_v4 = if segments[0] == 0x2002 {
        Some(Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            segments[1] as u8,
            (segments[2] >> 8) as u8,
            segments[2] as u8,
        ))
    } else if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0)
    {
        Some(Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        ))
    } else {
        None
    };
    if let Some(v4) = embedded_v4 {
        return classify_v4(v4);
    }
    IpClass::Public
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_requires_https_and_blocks_private_addresses() {
        let policy = UrlPolicy::strict();
        assert!(
            policy
                .validate("http://example.com/file", UrlPurpose::Download)
                .is_err()
        );
        assert!(
            policy
                .validate("https://example.com/file", UrlPurpose::Download)
                .is_ok()
        );
        assert!(
            policy
                .validate("https://127.0.0.1/file", UrlPurpose::Download)
                .is_err()
        );
        assert!(
            policy
                .validate("https://192.168.1.5/file", UrlPurpose::Download)
                .is_err()
        );
        assert!(
            policy
                .validate(
                    "https://169.254.169.254/latest/meta-data",
                    UrlPurpose::Download
                )
                .is_err()
        );
    }

    #[test]
    fn rejects_dangerous_url_shapes() {
        let policy = UrlPolicy::strict();
        for raw in [
            "",
            "file:///C:/Windows/System32",
            "javascript:alert(1)",
            "https://user:pass@example.com/",
            "https://example.com/#fragment",
            "https://example.com:8443/",
        ] {
            assert!(
                policy.validate(raw, UrlPurpose::ExternalApi).is_err(),
                "{raw:?} must be rejected"
            );
        }
    }

    #[test]
    fn explicit_self_hosted_exception_allows_private_and_http() {
        let policy = UrlPolicy::nexus_source(true, true, true);
        assert!(
            policy
                .validate("http://192.168.1.10:8080/api", UrlPurpose::NexusSource)
                .is_ok()
        );
        assert!(
            policy
                .validate("http://127.0.0.1:3000/api", UrlPurpose::NexusSource)
                .is_ok()
        );
        // Link-local metadata is blocked even with every exception enabled.
        assert!(
            policy
                .validate("http://169.254.169.254/latest", UrlPurpose::NexusSource)
                .is_err()
        );
    }

    #[test]
    fn ipv4_compatible_ipv6_cannot_bypass_ssrf_checks() {
        let policy = UrlPolicy::strict();
        for raw in [
            "https://[::127.0.0.1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://[::ffff:192.168.1.1]/",
            "https://[2002:7f00:0001::]/",
            "https://[64:ff9b::127.0.0.1]/",
        ] {
            assert!(policy.validate(raw, UrlPurpose::Download).is_err(), "{raw}");
        }
    }

    #[test]
    fn private_network_exception_does_not_imply_http() {
        let policy = UrlPolicy::nexus_source(true, false, false);
        assert!(
            policy
                .validate("https://10.0.0.5/api", UrlPurpose::NexusSource)
                .is_ok()
        );
        assert!(
            policy
                .validate("http://10.0.0.5/api", UrlPurpose::NexusSource)
                .is_err()
        );
        assert!(
            policy
                .validate("https://127.0.0.1/api", UrlPurpose::NexusSource)
                .is_err()
        );
    }

    #[test]
    fn nexus_base_url_is_normalized_and_cannot_embed_query() {
        let policy = UrlPolicy::nexus_source(true, true, true);
        let source = NexusSourceUrl::parse("http://192.168.1.4:8080/api/v1/", &policy).unwrap();
        assert_eq!(source.as_str(), "http://192.168.1.4:8080/api/v1");
        assert!(NexusSourceUrl::parse("http://192.168.1.4/api?token=x", &policy).is_err());
        assert!(source.join("../admin").is_err());
    }

    #[test]
    fn redirect_downgrade_is_rejected() {
        let policy = UrlPolicy::nexus_source(true, true, true);
        let from = Url::parse("https://example.com/a").unwrap();
        let to = Url::parse("http://example.com/a").unwrap();
        assert!(
            policy
                .validate_redirect(&from, &to, UrlPurpose::Download)
                .is_err()
        );
    }

    #[tokio::test]
    async fn resolved_metadata_address_is_blocked_even_with_local_exceptions() {
        let policy = UrlPolicy::nexus_source(true, true, true);
        let url = Url::parse("http://169.254.169.254/latest").unwrap();
        assert!(policy.resolve(&url, UrlPurpose::NexusSource).await.is_err());
    }
}
