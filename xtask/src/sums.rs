//! `SHA256SUMS` as `sha256sum` (release.yml) and `build-installer.ps1` write it: one
//! `<64 lower-case hex digits><two spaces or space + '*'><file name>` per line, LF or CRLF.

use std::collections::BTreeMap;

use anyhow::{Context, bail};
use mklm_update::Sha256Digest;

/// File name → digest. Refuses empty files, malformed lines and repeated names.
pub fn parse(text: &str) -> anyhow::Result<BTreeMap<String, Sha256Digest>> {
    let mut sums = BTreeMap::new();
    let body = text.strip_suffix('\n').unwrap_or(text);
    for (index, line) in body.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let number = index + 1;
        let (hex, rest) = line
            .split_at_checked(64)
            .with_context(|| format!("SHA256SUMS line {number} is too short"))?;
        let digest = Sha256Digest::parse_hex(hex)
            .with_context(|| format!("SHA256SUMS line {number}: not a lower-case SHA-256"))?;
        let name = rest
            .strip_prefix("  ")
            .or_else(|| rest.strip_prefix(" *"))
            .with_context(|| format!("SHA256SUMS line {number}: no separator after the hash"))?;
        if name.is_empty() || name.contains(['/', '\\', '\r', '\t']) || name.trim() != name {
            bail!("SHA256SUMS line {number}: {name:?} is not a plain file name");
        }
        if sums.insert(name.to_string(), digest).is_some() {
            bail!("SHA256SUMS names {name} twice");
        }
    }
    if sums.is_empty() {
        bail!("SHA256SUMS is empty");
    }
    Ok(sums)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b";
    const B: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    #[test]
    fn both_writers() {
        let text = format!("{A}  MKLM-Setup-0.2.1-arm64.exe\n{B}  MKLM-Setup-0.2.1-x64.exe\n");
        let sums = parse(&text).unwrap();
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["MKLM-Setup-0.2.1-x64.exe"].to_hex(), B);
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(parse(&crlf).unwrap(), sums);
        let binary = format!("{A} *MKLM-Setup-0.2.1-arm64.exe");
        assert_eq!(parse(&binary).unwrap().len(), 1);
    }

    #[test]
    fn refusals() {
        for text in [
            String::new(),
            "\n".to_string(),
            format!("{A}  x\n\n"),
            format!("{A} x"),
            format!("{A}  "),
            format!("{}  x", A.to_uppercase()),
            format!("{}  x", &A[1..]),
            format!("{A}  x\n{B}  x"),
            format!("{A}  dir/x"),
            format!("{A}  x "),
        ] {
            assert!(parse(&text).is_err(), "{text:?}");
        }
    }
}
