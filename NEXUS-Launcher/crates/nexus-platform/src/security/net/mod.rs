//! Network security primitives: URL/SSRF policy and a bounded HTTP client.

pub mod fetch;
pub mod url;

pub use fetch::{FetchLimits, SafeFetcher, SafeResponse};
pub use url::{NexusSourceUrl, UrlPolicy, UrlPurpose};
