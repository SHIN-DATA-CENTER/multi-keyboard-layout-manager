//! Strict URLs, the URL policy and the release endpoints (design m5b A.5, A.6, A.10).
//!
//! WP-0 implements the production endpoints' URLs; WP-U the parser, the redirect rules and the
//! tag extraction.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use crate::verify::SignatureSlot;
use crate::{MANIFEST_NAME, REPO_URL, Version};

/// Host of every first request in production.
const PRODUCTION_HOST: &str = "github.com";

/// A URL that passed a `UrlPolicy` (design m5b A.6). Only https (development builds: http to
/// 127.0.0.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    text: String,
    https: bool,
    host: String,
    port: u16,
    /// Starts at the `/` after the authority.
    path_and_query: String,
}

impl Url {
    pub fn parse(text: &str, policy: &UrlPolicy) -> Result<Url, UrlError> {
        Err(skeleton(text)) // Skeleton (M5b): WP-U
    }

    /// A `Location` header: absolute, or an absolute path on the same host.
    pub fn resolve(&self, location: &str, policy: &UrlPolicy) -> Result<Url, UrlError> {
        Err(skeleton(location)) // Skeleton (M5b): WP-U
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_https(&self) -> bool {
        self.https
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path_and_query(&self) -> &str {
        &self.path_and_query
    }
}

/// Which rules a [`UrlPolicy`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyKind {
    Production,
    #[cfg(all(debug_assertions, mklm_update_dev))]
    Loopback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPolicy {
    kind: PolicyKind,
}

impl UrlPolicy {
    /// https, port 443, first host github.com, redirects to github.com or *.githubusercontent.com.
    pub fn production() -> UrlPolicy {
        UrlPolicy {
            kind: PolicyKind::Production,
        }
    }

    /// http://127.0.0.1:<any port> only.
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn loopback() -> UrlPolicy {
        UrlPolicy {
            kind: PolicyKind::Loopback,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// `REPO_URL`, or the loopback base standing in for it; no trailing `/`.
    base: String,
    https: bool,
    host: String,
    port: u16,
    /// The path part of `base` (`/SHIN-DATA-CENTER/multi-keyboard-layout-manager`).
    base_path: String,
    policy: UrlPolicy,
}

impl Endpoints {
    pub fn production() -> Endpoints {
        Endpoints {
            base: REPO_URL.to_string(),
            https: true,
            host: PRODUCTION_HOST.to_string(),
            port: 443,
            base_path: REPO_URL
                .strip_prefix("https://github.com")
                .unwrap_or_default()
                .to_string(),
            policy: UrlPolicy::production(),
        }
    }

    /// `base` = `http://127.0.0.1:<port>` standing in for `REPO_URL` (same paths below it).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn loopback(base: &str) -> Result<Endpoints, UrlError> {
        Err(skeleton(base)) // Skeleton (M5b): WP-U
    }

    pub fn policy(&self) -> &UrlPolicy {
        &self.policy
    }

    /// `<base>/releases/latest/download/latest.json`
    pub fn manifest_url(&self) -> Url {
        self.latest_file_url(MANIFEST_NAME)
    }

    /// `<base>/releases/download/<tag>/<slot file>`, or the latest/download one without a tag.
    pub fn signature_url(&self, tag: Option<&str>, slot: SignatureSlot) -> Url {
        match tag {
            Some(tag) => self.url(&format!("/releases/download/{tag}/{}", slot.file_name())),
            None => self.latest_file_url(slot.file_name()),
        }
    }

    /// `<base>/releases/latest/download/<name>` (fetch-smoke: `SHA256SUMS`).
    pub fn latest_file_url(&self, name: &str) -> Url {
        self.url(&format!("/releases/latest/download/{name}"))
    }

    /// `<base>/releases/download/v<version>/<name>`
    pub fn asset_url(&self, version: &Version, name: &str) -> Url {
        self.url(&format!("/releases/download/v{version}/{name}"))
    }

    /// `v<X.Y.Z>` when `location` is `<base>/releases/download/v<X.Y.Z>/<asset_name>`
    /// (OPS-UX-TEST-8: any asset name).
    pub fn tag_from_location(&self, location: &Url, asset_name: &str) -> Option<String> {
        None // Skeleton (M5b): WP-U
    }

    /// `<base><suffix>`; `suffix` starts with `/`.
    fn url(&self, suffix: &str) -> Url {
        Url {
            text: format!("{}{suffix}", self.base),
            https: self.https,
            host: self.host.clone(),
            port: self.port,
            path_and_query: format!("{}{suffix}", self.base_path),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{url:?} is not allowed: {reason}")]
pub struct UrlError {
    pub url: String,
    pub reason: &'static str,
}

/// What the WP-0 skeleton's unimplemented functions return (design m5b G.2).
fn skeleton(url: &str) -> UrlError {
    UrlError {
        url: url.to_string(),
        reason: "not implemented (m5b skeleton)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager";

    #[test]
    fn production_urls() {
        let endpoints = Endpoints::production();
        assert_eq!(endpoints.policy(), &UrlPolicy::production());
        let manifest = endpoints.manifest_url();
        assert_eq!(
            manifest.as_str(),
            format!("{REPO}/releases/latest/download/latest.json")
        );
        assert!(manifest.is_https());
        assert_eq!(manifest.host(), "github.com");
        assert_eq!(manifest.port(), 443);
        assert_eq!(
            manifest.path_and_query(),
            "/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/latest/download/latest.json"
        );
        assert_eq!(
            endpoints
                .signature_url(Some("v0.2.1"), SignatureSlot::Main)
                .as_str(),
            format!("{REPO}/releases/download/v0.2.1/latest.json.minisig")
        );
        assert_eq!(
            endpoints
                .signature_url(Some("v0.2.1"), SignatureSlot::Alt)
                .as_str(),
            format!("{REPO}/releases/download/v0.2.1/latest.json.alt.minisig")
        );
        assert_eq!(
            endpoints.signature_url(None, SignatureSlot::Main).as_str(),
            format!("{REPO}/releases/latest/download/latest.json.minisig")
        );
        assert_eq!(
            endpoints.signature_url(None, SignatureSlot::Alt).as_str(),
            format!("{REPO}/releases/latest/download/latest.json.alt.minisig")
        );
        assert_eq!(
            endpoints.latest_file_url("SHA256SUMS").as_str(),
            format!("{REPO}/releases/latest/download/SHA256SUMS")
        );
        let asset = endpoints.asset_url(&Version::new(0, 2, 1), "MKLM-Setup-0.2.1-x64.exe");
        assert_eq!(
            asset.as_str(),
            format!("{REPO}/releases/download/v0.2.1/MKLM-Setup-0.2.1-x64.exe")
        );
        assert_eq!(
            asset.path_and_query(),
            "/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/download/v0.2.1/MKLM-Setup-0.2.1-x64.exe"
        );
        assert_eq!(asset.host(), "github.com");
    }
}
