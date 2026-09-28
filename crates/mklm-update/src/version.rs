//! Version rules (design m5b A.2, C.5).
//!
//! A release version is `X.Y.Z` only: decimal numbers without leading zeros, each at most
//! [`MAX_VERSION_PART`], no `v`, no pre-release, no build metadata. The installed version is the
//! executable's `CARGO_PKG_VERSION`, where a development pre-release (`0.2.0-dev.1`) is allowed.
//! Comparison is SemVer precedence.

use std::cmp::Ordering;

use crate::Version;
use crate::refusal::UpdateRefusal;

/// Largest value of one version part (VERSIONINFO fields are 16-bit).
pub const MAX_VERSION_PART: u64 = 65_535;

fn bad(text: &str) -> UpdateRefusal {
    UpdateRefusal::BadVersion {
        text: text.to_string(),
    }
}

/// One decimal part: `0`, or digits without a leading zero, at most `MAX_VERSION_PART`.
fn part(text: &str) -> Option<u64> {
    let digits = text.as_bytes();
    if digits.is_empty()
        || digits.len() > 5
        || !digits.iter().all(u8::is_ascii_digit)
        || (digits.len() > 1 && digits[0] == b'0')
    {
        return None;
    }
    text.parse::<u64>().ok().filter(|&n| n <= MAX_VERSION_PART)
}

/// `X.Y.Z` only (design m5b A.2). `BadVersion`.
pub fn parse_release_version(text: &str) -> Result<Version, UpdateRefusal> {
    let parts: Vec<&str> = text.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else {
        return Err(bad(text));
    };
    match (part(major), part(minor), part(patch)) {
        (Some(major), Some(minor), Some(patch)) => Ok(Version::new(major, minor, patch)),
        _ => Err(bad(text)),
    }
}

/// `CARGO_PKG_VERSION`: a pre-release is allowed, build metadata is not. `BadVersion`.
pub fn parse_installed_version(text: &str) -> Result<Version, UpdateRefusal> {
    let version = Version::parse(text).map_err(|_| bad(text))?;
    if !version.build.is_empty()
        || version.major > MAX_VERSION_PART
        || version.minor > MAX_VERSION_PART
        || version.patch > MAX_VERSION_PART
    {
        return Err(bad(text));
    }
    Ok(version)
}

/// SemVer precedence: `offered > installed`.
pub fn is_newer(offered: &Version, installed: &Version) -> bool {
    offered.cmp_precedence(installed) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_versions() {
        for (text, expected) in [
            ("0.2.1", Version::new(0, 2, 1)),
            ("0.0.0", Version::new(0, 0, 0)),
            ("10.20.30", Version::new(10, 20, 30)),
            ("65535.65535.65535", Version::new(65_535, 65_535, 65_535)),
        ] {
            assert_eq!(parse_release_version(text), Ok(expected), "{text}");
        }
        for text in [
            "v0.2.1",
            "0.2",
            "0.2.1.0",
            "0.2.1-beta",
            "0.2.1+x",
            "00.2.1",
            "0.02.1",
            "0.2.01",
            "65536.0.0",
            "0.65536.0",
            "0.0.100000",
            "",
            "..",
            "0..1",
            " 0.2.1",
            "0.2.1 ",
            "0.2.-1",
            "0.2.+1",
            "0.2.1\n",
            "０.2.1",
            "99999999999999999999.0.0",
        ] {
            assert_eq!(
                parse_release_version(text),
                Err(UpdateRefusal::BadVersion {
                    text: text.to_string()
                }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn installed_versions() {
        assert_eq!(parse_installed_version("0.2.0"), Ok(Version::new(0, 2, 0)));
        let dev = parse_installed_version("0.2.0-dev.1").unwrap();
        assert_eq!(dev.pre.as_str(), "dev.1");
        for text in [
            "0.2.0+abc",
            "v0.2.0",
            "0.2",
            "00.2.0",
            "65536.0.0",
            "0.2.0-dev.1+x",
            "",
        ] {
            assert!(parse_installed_version(text).is_err(), "{text}");
        }
    }

    #[test]
    fn precedence() {
        let v = |text: &str| parse_installed_version(text).unwrap();
        assert!(is_newer(&v("0.2.1"), &v("0.2.0")));
        assert!(is_newer(&v("1.0.0"), &v("0.99.99")));
        assert!(is_newer(&v("0.2.0"), &v("0.2.0-dev.1")));
        assert!(is_newer(&v("0.2.0-dev.2"), &v("0.2.0-dev.1")));
        assert!(!is_newer(&v("0.2.0"), &v("0.2.0")));
        assert!(!is_newer(&v("0.2.0"), &v("0.2.1")));
        assert!(!is_newer(&v("0.2.0-dev.1"), &v("0.2.0")));
        assert!(is_newer(&v("0.10.0"), &v("0.9.0")));
    }
}
