//! Strict URLs, the URL policy and the release endpoints (design m5b A.5, A.6, A.10).
//!
//! The URL grammar is deliberately small: `<scheme>://<host>[:<port>]<path>[?<query>]` with the
//! path starting at `/`, printable ASCII only, no userinfo, no fragment, no backslash, no white
//! space. Host names are ASCII labels (letters, digits, `-`), compared in lower case; a host whose
//! last label is all digits (an IPv4 address in any spelling) or that starts with `[` (IPv6) is
//! not a name.
//!
//! - Production (`UrlPolicy::production`): https on port 443 only, to `github.com` or a host
//!   ending in `.githubusercontent.com` (the redirect targets GitHub has used; A.6). The first
//!   request of every fetch goes to `github.com`, fixed by [`Endpoints::production`].
//! - Loopback (development builds only): `http://127.0.0.1:<port>` only.

use crate::verify::SignatureSlot;
use crate::version::parse_release_version;
use crate::{MANIFEST_NAME, REPO_URL, Version};

/// Host of every first request in production.
const PRODUCTION_HOST: &str = "github.com";
/// Redirects may also go to hosts ending with this (A.6).
const PRODUCTION_REDIRECT_SUFFIX: &str = ".githubusercontent.com";

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
        let error = |reason| UrlError {
            url: text.to_string(),
            reason,
        };
        if text.is_empty() || text.len() > MAX_URL_LEN {
            return Err(error("empty or too long"));
        }
        if !text.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
            return Err(error("white space, a control character or non-ASCII"));
        }
        if text.contains('#') {
            return Err(error("a fragment"));
        }
        if text.contains('\\') {
            return Err(error("a backslash"));
        }
        let (https, rest) = if let Some(rest) = text.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = text.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(error("not an http(s) URL"));
        };
        let Some(slash) = rest.find('/') else {
            return Err(error("no path"));
        };
        let (authority, path_and_query) = rest.split_at(slash);
        if authority.contains(['@', '?']) {
            return Err(error("userinfo or a query in the authority"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => {
                let port = parse_port(port).ok_or_else(|| error("a malformed port"))?;
                (host, Some(port))
            }
            None => (authority, None),
        };
        let host = host.to_ascii_lowercase();
        let effective_port = port.unwrap_or(if https { 443 } else { 80 });
        // The text names the port unless it is https's 443, so that it parses again as itself.
        let text = if https && effective_port == 443 {
            format!("https://{host}{path_and_query}")
        } else {
            format!(
                "{}://{host}:{effective_port}{path_and_query}",
                if https { "https" } else { "http" }
            )
        };
        let url = Url {
            text,
            https,
            host,
            port: effective_port,
            path_and_query: path_and_query.to_string(),
        };
        policy.check(&url, port.is_some()).map_err(error)?;
        Ok(url)
    }

    /// A `Location` header: absolute, or an absolute path on the same host.
    pub fn resolve(&self, location: &str, policy: &UrlPolicy) -> Result<Url, UrlError> {
        if location.starts_with('/') && !location.starts_with("//") {
            let base = format!(
                "{}://{}:{}",
                if self.https { "https" } else { "http" },
                self.host,
                self.port
            );
            return Url::parse(&format!("{base}{location}"), policy).map_err(|error| UrlError {
                url: location.to_string(),
                reason: error.reason,
            });
        }
        if location.starts_with("https://") || location.starts_with("http://") {
            return Url::parse(location, policy);
        }
        Err(UrlError {
            url: location.to_string(),
            reason: "neither an absolute URL nor an absolute path",
        })
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

/// Longer URLs are refused (GitHub's signed download URLs are well below 2 KiB).
const MAX_URL_LEN: usize = 4096;

/// 1..=65535, decimal digits, no leading zero.
fn parse_port(text: &str) -> Option<u16> {
    let digits = text.as_bytes();
    if digits.is_empty()
        || digits.len() > 5
        || !digits.iter().all(u8::is_ascii_digit)
        || digits[0] == b'0'
    {
        return None;
    }
    text.parse::<u16>().ok()
}

/// ASCII labels of letters, digits and inner hyphens, 1 to 63 characters each; the last label
/// not all digits (so no IPv4 address in any spelling). Already in lower case.
fn is_host_name(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |label: &&str| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    labels.iter().all(label_ok)
        && labels
            .last()
            .is_some_and(|last| !last.bytes().all(|b| b.is_ascii_digit()))
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

    /// The host and port rules; `explicit_port` tells whether the text named the port (the
    /// loopback policy requires it).
    #[cfg_attr(
        not(all(debug_assertions, mklm_update_dev)),
        allow(
            unused_variables,
            reason = "only the loopback policy reads explicit_port"
        )
    )]
    fn check(&self, url: &Url, explicit_port: bool) -> Result<(), &'static str> {
        match self.kind {
            PolicyKind::Production => {
                if !url.https {
                    return Err("not https");
                }
                if url.port != 443 {
                    return Err("a port other than 443");
                }
                if !is_host_name(&url.host) {
                    return Err("not a host name");
                }
                let allowed = url.host == PRODUCTION_HOST
                    || url
                        .host
                        .strip_suffix(PRODUCTION_REDIRECT_SUFFIX)
                        .is_some_and(|prefix| !prefix.is_empty());
                if !allowed {
                    return Err("a host other than github.com and *.githubusercontent.com");
                }
                Ok(())
            }
            #[cfg(all(debug_assertions, mklm_update_dev))]
            PolicyKind::Loopback => {
                if url.https {
                    return Err("not http to 127.0.0.1");
                }
                if url.host != "127.0.0.1" {
                    return Err("a host other than 127.0.0.1");
                }
                if !explicit_port {
                    return Err("no port");
                }
                Ok(())
            }
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
        let policy = UrlPolicy::loopback();
        let trimmed = base.strip_suffix('/').unwrap_or(base);
        let url = Url::parse(&format!("{trimmed}/"), &policy).map_err(|error| UrlError {
            url: base.to_string(),
            reason: error.reason,
        })?;
        if url.path_and_query != "/" {
            return Err(UrlError {
                url: base.to_string(),
                reason: "a path below the base",
            });
        }
        Ok(Endpoints {
            base: format!("http://127.0.0.1:{}", url.port),
            https: false,
            host: url.host,
            port: url.port,
            base_path: String::new(),
            policy,
        })
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
        if location.https != self.https || location.host != self.host || location.port != self.port
        {
            return None;
        }
        let rest = location
            .path_and_query
            .strip_prefix(&self.base_path)?
            .strip_prefix("/releases/download/v")?;
        let (version, name) = rest.split_once('/')?;
        if name != asset_name {
            return None;
        }
        parse_release_version(version).ok()?;
        Some(format!("v{version}"))
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
        // Every endpoint URL passes the production policy as text too.
        for url in [manifest, asset] {
            assert_eq!(Url::parse(url.as_str(), endpoints.policy()), Ok(url));
        }
    }

    #[test]
    fn production_policy_accepts_github_hosts_over_https() {
        let policy = UrlPolicy::production();
        for (text, host, path) in [
            ("https://github.com/x", "github.com", "/x"),
            (
                "https://github.com:443/x?y=1&z=%2F",
                "github.com",
                "/x?y=1&z=%2F",
            ),
            ("https://GitHub.com/x", "github.com", "/x"),
            (
                "https://release-assets.githubusercontent.com/github-production-release-asset/1?se=2026&sig=a%2Bb",
                "release-assets.githubusercontent.com",
                "/github-production-release-asset/1?se=2026&sig=a%2Bb",
            ),
            (
                "https://objects.githubusercontent.com/a",
                "objects.githubusercontent.com",
                "/a",
            ),
        ] {
            let url = Url::parse(text, &policy).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(url.host(), host);
            assert_eq!(url.port(), 443);
            assert_eq!(url.path_and_query(), path);
            assert!(url.is_https());
        }
        assert_eq!(
            Url::parse("https://github.com:443/x", &policy)
                .unwrap()
                .as_str(),
            "https://github.com/x"
        );
    }

    #[test]
    fn production_policy_refusals() {
        let policy = UrlPolicy::production();
        for text in [
            "http://github.com/x",
            "https://github.com:8443/x",
            "https://github.com:0443/x",
            "https://github.com:/x",
            "https://github.com:65536/x",
            "https://user@github.com/x",
            "https://user:pw@github.com/x",
            "https://github.com/x#frag",
            "https://github.com",
            "https://github.com?x",
            "https://github.com.evil.example/x",
            "https://evilgithub.com/x",
            "https://githubusercontent.com/x",
            "https://.githubusercontent.com/x",
            "https://x.githubusercontent.com.evil/x",
            "https://140.82.112.3/x",
            "https://[::1]/x",
            "https://2130706433/x",
            "https://gith\u{fc}b.com/x",
            "https://xn--github.com/x\u{fc}",
            "https://github.com/a b",
            "https://github.com/a\tb",
            "https://github.com/a\\b",
            "https://github.com./x",
            "https://-github.com/x",
            "https://git_hub.com/x",
            "ftp://github.com/x",
            "HTTPS://github.com/x",
            "//github.com/x",
            "",
        ] {
            assert!(Url::parse(text, &policy).is_err(), "{text:?}");
        }
        let long = format!("https://github.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(Url::parse(&long, &policy).is_err());
    }

    #[test]
    fn locations_resolve_absolute_or_on_the_same_host() {
        let policy = UrlPolicy::production();
        let base = Url::parse(
            "https://github.com/o/r/releases/latest/download/latest.json",
            &policy,
        )
        .unwrap();
        let same = base
            .resolve("/o/r/releases/download/v0.2.1/latest.json", &policy)
            .unwrap();
        assert_eq!(
            same.as_str(),
            "https://github.com/o/r/releases/download/v0.2.1/latest.json"
        );
        let cdn = base
            .resolve(
                "https://release-assets.githubusercontent.com/a/b?sig=1",
                &policy,
            )
            .unwrap();
        assert_eq!(cdn.host(), "release-assets.githubusercontent.com");
        for location in [
            "releases/download/v0.2.1/latest.json",
            "//evil.example/x",
            "https://evil.example/x",
            "http://github.com/x",
            "/x y",
            "",
            "javascript:alert(1)",
        ] {
            assert!(base.resolve(location, &policy).is_err(), "{location:?}");
        }
    }

    #[test]
    fn tags_from_first_redirects() {
        let endpoints = Endpoints::production();
        let policy = endpoints.policy().clone();
        let location = |text: &str| Url::parse(text, &policy).unwrap();
        assert_eq!(
            endpoints.tag_from_location(
                &location(&format!("{REPO}/releases/download/v0.2.1/latest.json")),
                "latest.json"
            ),
            Some("v0.2.1".to_string())
        );
        assert_eq!(
            endpoints.tag_from_location(
                &location(&format!("{REPO}/releases/download/v0.1.0/SHA256SUMS")),
                "SHA256SUMS"
            ),
            Some("v0.1.0".to_string())
        );
        for text in [
            format!("{REPO}/releases/download/v0.2.1/latest.json.minisig"),
            format!("{REPO}/releases/download/v0.2.1/SHA256SUMS"),
            format!("{REPO}/releases/download/0.2.1/latest.json"),
            format!("{REPO}/releases/download/v0.2/latest.json"),
            format!("{REPO}/releases/download/v0.2.1-rc.1/latest.json"),
            format!("{REPO}/releases/download/v00.2.1/latest.json"),
            format!("{REPO}/releases/download/v0.2.1/x/latest.json"),
            format!("{REPO}/releases/download/v0.2.1/latest.json?x=1"),
            format!("{REPO}/releases/latest/download/latest.json"),
            "https://github.com/other/repo/releases/download/v0.2.1/latest.json".to_string(),
            "https://release-assets.githubusercontent.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/download/v0.2.1/latest.json".to_string(),
        ] {
            assert_eq!(
                endpoints.tag_from_location(&location(&text), "latest.json"),
                None,
                "{text}"
            );
        }
    }

    #[test]
    fn host_names() {
        for good in ["github.com", "a.b-c.d", "x1.y2", "github.com"] {
            assert!(is_host_name(good), "{good}");
        }
        for bad in [
            "", "a..b", ".a", "a.", "-a.b", "a-.b", "1.2.3.4", "a.123", "A.b", "a_b.c",
        ] {
            assert!(!is_host_name(bad), "{bad}");
        }
    }

    #[cfg(all(debug_assertions, mklm_update_dev))]
    #[test]
    fn loopback_policy_and_endpoints() {
        let policy = UrlPolicy::loopback();
        let url = Url::parse(
            "http://127.0.0.1:8421/releases/latest/download/latest.json",
            &policy,
        )
        .unwrap();
        assert_eq!(url.port(), 8421);
        assert!(!url.is_https());
        for text in [
            "http://127.0.0.1/x",
            "http://127.0.0.2:8421/x",
            "http://localhost:8421/x",
            "https://127.0.0.1:8421/x",
            "https://github.com/x",
            "http://0x7f000001:8421/x",
        ] {
            assert!(Url::parse(text, &policy).is_err(), "{text}");
        }
        // The production policy never accepts the loopback URL.
        assert!(Url::parse(url.as_str(), &UrlPolicy::production()).is_err());

        let endpoints = Endpoints::loopback("http://127.0.0.1:8421").unwrap();
        assert_eq!(
            Endpoints::loopback("http://127.0.0.1:8421/"),
            Ok(endpoints.clone())
        );
        assert_eq!(
            endpoints.manifest_url().as_str(),
            "http://127.0.0.1:8421/releases/latest/download/latest.json"
        );
        assert_eq!(
            endpoints.manifest_url().path_and_query(),
            "/releases/latest/download/latest.json"
        );
        let location = endpoints
            .manifest_url()
            .resolve("/releases/download/v0.2.1/latest.json", endpoints.policy())
            .unwrap();
        assert_eq!(
            endpoints.tag_from_location(&location, "latest.json"),
            Some("v0.2.1".to_string())
        );
        let other_port = Url::parse(
            "http://127.0.0.1:8422/releases/download/v0.2.1/latest.json",
            &policy,
        )
        .unwrap();
        assert_eq!(
            endpoints.tag_from_location(&other_port, "latest.json"),
            None
        );
        for base in [
            "http://127.0.0.1",
            "http://127.0.0.1:8421/x",
            "http://localhost:8421",
            "https://127.0.0.1:8421",
            "https://github.com",
        ] {
            assert!(Endpoints::loopback(base).is_err(), "{base}");
        }
    }
}
