//! Checks both sides make before the first request, and the session timeouts.
//!
//! Order (section E.3 of the design doc):
//! 1. Caller: pipe created with `FILE_FLAG_FIRST_PIPE_INSTANCE`; helper launched; the connecting
//!    client's PID (`GetNamedPipeClientProcessId`) must be the launched helper's PID.
//! 2. Helper: the server's PID (`GetNamedPipeServerProcessId`) must be `--caller-pid`, and that
//!    process's image must sit in the helper's own directory; then it sends [`Hello`].
//! 3. Caller: [`verify_hello`]; sends [`Welcome`].
//! 4. Helper: [`verify_welcome`].

use std::time::Duration;

use crate::PROTOCOL_VERSION;
use crate::args::Nonce;
use crate::message::{Hello, Welcome};

/// Caller: how long to wait for the helper to connect (covers the UAC prompt).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
/// Both: how long each handshake frame may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Helper: how long to wait for the next request (or `Bye`) before exiting.
pub const REQUEST_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Caller: longest silence while a request runs. The helper's writer thread sends
/// [`crate::Event::Heartbeat`] every [`HEARTBEAT_INTERVAL`] whatever the engine is doing, so only a
/// wedged helper process stays silent this long (design review S7); a helper process that exits
/// ends the wait at once (the caller holds its process handle).
pub const MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Helper: interval of the writer thread's heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// Helper: longest wait for a [`crate::Decision`] outside a countdown (reconnect path).
pub const DECISION_TIMEOUT: Duration = Duration::from_secs(600);

/// A handshake check failed; the session ends without any request being read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    #[error("protocol version {found}, expected {expected}")]
    Version { found: u32, expected: u32 },
    #[error("the helper did not present the expected nonce")]
    Nonce,
    #[error("peer PID {found}, expected {expected}")]
    Pid { expected: u32, found: u32 },
    #[error("peer build {found:?}, expected {expected:?}; reinstall or rebuild both executables")]
    Build { expected: String, found: String },
}

/// Caller side: version, build ID (exact), nonce (constant time) and `helper_pid` = the PID the
/// caller launched and saw connect.
pub fn verify_hello(
    hello: &Hello,
    nonce: &Nonce,
    launched_pid: u32,
    build_id: &str,
) -> Result<(), HandshakeError> {
    check_version(hello.protocol)?;
    check_build(&hello.build_id, build_id)?;
    // A malformed nonce fails like a wrong one. Only its format decides how early, never how much
    // of the secret it shares.
    let presented = Nonce::parse_hex(&hello.nonce_hex).map_err(|_| HandshakeError::Nonce)?;
    if !presented.matches(nonce) {
        return Err(HandshakeError::Nonce);
    }
    check_pid(hello.helper_pid, launched_pid)
}

/// Helper side: version, build ID (exact) and `caller_pid` = `--caller-pid`.
pub fn verify_welcome(
    welcome: &Welcome,
    caller_pid: u32,
    build_id: &str,
) -> Result<(), HandshakeError> {
    check_version(welcome.protocol)?;
    check_build(&welcome.build_id, build_id)?;
    check_pid(welcome.caller_pid, caller_pid)
}

fn check_version(found: u32) -> Result<(), HandshakeError> {
    if found == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(HandshakeError::Version {
            found,
            expected: PROTOCOL_VERSION,
        })
    }
}

/// Exact match. An empty ID never matches, not even an empty one: it would mean a build without
/// the ID (design review S11), and two such builds must not pass for a matching pair.
fn check_build(found: &str, expected: &str) -> Result<(), HandshakeError> {
    if !expected.is_empty() && found == expected {
        Ok(())
    } else {
        Err(HandshakeError::Build {
            expected: expected.to_string(),
            found: found.to_string(),
        })
    }
}

/// Exact match; PID 0 (the idle process) is never a peer.
fn check_pid(found: u32, expected: u32) -> Result<(), HandshakeError> {
    if found == expected && found != 0 {
        Ok(())
    } else {
        Err(HandshakeError::Pid { expected, found })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::NONCE_LEN;

    const BUILD: &str = "0.1.0+0123456789abcdef";
    const HELPER_PID: u32 = 5150;
    const CALLER_PID: u32 = 4242;

    fn nonce_bytes() -> [u8; NONCE_LEN] {
        let mut bytes = [0u8; NONCE_LEN];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::try_from(index)
                .unwrap()
                .wrapping_mul(37)
                .wrapping_add(11);
        }
        bytes
    }

    fn nonce() -> Nonce {
        Nonce::from_bytes(nonce_bytes())
    }

    fn hello() -> Hello {
        Hello::new(&nonce(), HELPER_PID, "0.1.0", BUILD)
    }

    fn welcome() -> Welcome {
        Welcome::new(CALLER_PID, BUILD)
    }

    #[test]
    fn matching_peers_pass() {
        let hello = hello();
        assert_eq!(hello.protocol, PROTOCOL_VERSION);
        assert_eq!(hello.nonce_hex, nonce().to_hex());
        assert_eq!(hello.helper_version, "0.1.0");
        assert_eq!(verify_hello(&hello, &nonce(), HELPER_PID, BUILD), Ok(()));
        let welcome = welcome();
        assert_eq!(welcome.protocol, PROTOCOL_VERSION);
        assert_eq!(verify_welcome(&welcome, CALLER_PID, BUILD), Ok(()));
    }

    #[test]
    fn nonce_mismatch() {
        for index in [0, NONCE_LEN / 2, NONCE_LEN - 1] {
            let mut other = nonce_bytes();
            other[index] ^= 0x80;
            let hello = Hello::new(&Nonce::from_bytes(other), HELPER_PID, "0.1.0", BUILD);
            assert_eq!(
                verify_hello(&hello, &nonce(), HELPER_PID, BUILD),
                Err(HandshakeError::Nonce)
            );
        }
        let good = nonce().to_hex();
        for nonce_hex in [
            String::new(),
            good.to_uppercase(),
            good[..62].to_string(),
            format!("{good}00"),
            format!(" {}", &good[1..]),
        ] {
            let hello = Hello {
                nonce_hex,
                ..hello()
            };
            assert_eq!(
                verify_hello(&hello, &nonce(), HELPER_PID, BUILD),
                Err(HandshakeError::Nonce)
            );
        }
    }

    #[test]
    fn pid_mismatch() {
        assert_eq!(
            verify_hello(&hello(), &nonce(), HELPER_PID + 4, BUILD),
            Err(HandshakeError::Pid {
                expected: HELPER_PID + 4,
                found: HELPER_PID
            })
        );
        assert_eq!(
            verify_welcome(&welcome(), CALLER_PID + 4, BUILD),
            Err(HandshakeError::Pid {
                expected: CALLER_PID + 4,
                found: CALLER_PID
            })
        );
        // PID 0 is nobody, even when "expected".
        let zero = Hello::new(&nonce(), 0, "0.1.0", BUILD);
        assert!(matches!(
            verify_hello(&zero, &nonce(), 0, BUILD),
            Err(HandshakeError::Pid { .. })
        ));
        assert!(matches!(
            verify_welcome(&Welcome::new(0, BUILD), 0, BUILD),
            Err(HandshakeError::Pid { .. })
        ));
    }

    #[test]
    fn build_mismatch() {
        let other = "0.1.0+fedcba9876543210";
        assert_eq!(
            verify_hello(&hello(), &nonce(), HELPER_PID, other),
            Err(HandshakeError::Build {
                expected: other.to_string(),
                found: BUILD.to_string()
            })
        );
        assert_eq!(
            verify_welcome(&welcome(), CALLER_PID, other),
            Err(HandshakeError::Build {
                expected: other.to_string(),
                found: BUILD.to_string()
            })
        );
        // Exact: no case folding, no trimming, no prefix match.
        for found in [
            BUILD.to_uppercase(),
            format!("{BUILD} "),
            BUILD[..BUILD.len() - 1].to_string(),
            String::new(),
        ] {
            let hello = Hello {
                build_id: found.clone(),
                ..hello()
            };
            assert!(
                matches!(
                    verify_hello(&hello, &nonce(), HELPER_PID, BUILD),
                    Err(HandshakeError::Build { .. })
                ),
                "{found:?}"
            );
            let welcome = Welcome {
                build_id: found.clone(),
                ..welcome()
            };
            assert!(
                matches!(
                    verify_welcome(&welcome, CALLER_PID, BUILD),
                    Err(HandshakeError::Build { .. })
                ),
                "{found:?}"
            );
        }
        // Two builds without an ID do not match each other.
        let hello = Hello {
            build_id: String::new(),
            ..hello()
        };
        assert!(matches!(
            verify_hello(&hello, &nonce(), HELPER_PID, ""),
            Err(HandshakeError::Build { .. })
        ));
        assert!(matches!(
            verify_welcome(&Welcome::new(CALLER_PID, ""), CALLER_PID, ""),
            Err(HandshakeError::Build { .. })
        ));
    }

    #[test]
    fn version_mismatch() {
        for protocol in [0, PROTOCOL_VERSION + 1, u32::MAX] {
            let hello = Hello {
                protocol,
                ..hello()
            };
            assert_eq!(
                verify_hello(&hello, &nonce(), HELPER_PID, BUILD),
                Err(HandshakeError::Version {
                    found: protocol,
                    expected: PROTOCOL_VERSION
                })
            );
            let welcome = Welcome {
                protocol,
                ..welcome()
            };
            assert_eq!(
                verify_welcome(&welcome, CALLER_PID, BUILD),
                Err(HandshakeError::Version {
                    found: protocol,
                    expected: PROTOCOL_VERSION
                })
            );
        }
    }

    /// The version is checked first, so a peer of another version is reported as such even when
    /// everything else differs too.
    #[test]
    fn version_is_reported_first() {
        let hello = Hello {
            protocol: PROTOCOL_VERSION + 1,
            nonce_hex: "zz".to_string(),
            helper_pid: 1,
            helper_version: String::new(),
            build_id: "other".to_string(),
        };
        assert!(matches!(
            verify_hello(&hello, &nonce(), HELPER_PID, BUILD),
            Err(HandshakeError::Version { .. })
        ));
    }
}
