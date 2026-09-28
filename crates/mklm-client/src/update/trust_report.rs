//! The `RecordTrust` a session sends after `Welcome` (design m5b C.4; SECURITY-5).
//!
//! The machine record in HKLM advances only when some helper runs; the user's record advances with
//! every check. When the user knows of a newer manifest (a higher `issued_at` for a key, or a
//! revocation the machine lacks: `TrustState::is_ahead_of`), every helper session — a keyboard
//! change too — carries that manifest and its signature to the helper first, which verifies it for
//! itself and records it. The data is signed: the helper need not trust the sender.

use mklm_ipc::TrustReport;
use mklm_update::TrustState;

use crate::update::cache::UpdateCache;

/// The `RecordTrust` to send after `Welcome`, if any (design m5b C.4): the cached verified
/// manifest and its signature when the user record `is_ahead_of` the machine record.
pub fn pending_trust_report(cache: &UpdateCache, machine: &TrustState) -> Option<TrustReport> {
    let client = cache.load_state();
    if !client.trust.is_ahead_of(machine) {
        return None;
    }
    let (manifest, signature) = cache.load_manifest()?;
    Some(TrustReport {
        manifest: String::from_utf8(manifest).ok()?,
        signature: String::from_utf8(signature).ok()?,
    })
}

/// The reporter of the current user (`session::set_trust_reporter`): the user's cache and the
/// machine record, read when a session starts. Nothing is sent while either cannot be read.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CurrentUser {
    /// Called with the helper's answer (the GUI logs it).
    pub on_answer: Option<fn(&mklm_ipc::UpdateMessage)>,
}

#[cfg(windows)]
impl crate::session::TrustReporter for CurrentUser {
    fn report(&self) -> Option<TrustReport> {
        let dir = mklm_win::user_dirs::update_cache_dir().ok()?;
        let store = mklm_win::update_store::read_update_store().ok()?;
        let machine = match store.trust {
            // A record that cannot be read is empty (as the helper reads it, design m5b C.4).
            Some(text) => TrustState::parse(&text).unwrap_or_default(),
            None => TrustState::default(),
        };
        pending_trust_report(&UpdateCache::new(dir), &machine)
    }

    fn answered(&self, reply: &mklm_ipc::UpdateMessage) {
        if let Some(on_answer) = self.on_answer {
            on_answer(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::cache::ClientState;
    use crate::update::cache::tests::Scratch;

    #[test]
    fn nothing_is_sent_while_the_machine_knows_as_much() {
        let scratch = Scratch::new("trust-report");
        let cache = scratch.cache();
        let machine = TrustState::default();
        // Nothing cached, nothing recorded.
        assert_eq!(pending_trust_report(&cache, &machine), None);
        cache.store_manifest(b"{}", b"sig").unwrap();
        cache.save_state(&ClientState::default()).unwrap();
        // The user's record is not ahead of an equal machine record.
        assert_eq!(pending_trust_report(&cache, &machine), None);
    }

    /// With `TrustState::is_ahead_of` of mklm-update (WP-U): the cached manifest goes when the
    /// user's record is ahead. Skipped while the skeleton's `is_ahead_of` answers false.
    #[test]
    fn the_cached_manifest_is_sent_when_the_user_knows_more() {
        let mut ahead = TrustState::default();
        ahead
            .max_issued_at
            .insert("DE84F116B8548221".into(), 1_792_022_400);
        if !ahead.is_ahead_of(&TrustState::default()) {
            eprintln!("skipped: TrustState::is_ahead_of is the m5b skeleton's");
            return;
        }
        let scratch = Scratch::new("trust-report-ahead");
        let cache = scratch.cache();
        cache.store_manifest(b"{\"m\":1}", b"sig").unwrap();
        cache
            .save_state(&ClientState {
                trust: ahead.clone(),
                ..ClientState::default()
            })
            .unwrap();
        assert_eq!(
            pending_trust_report(&cache, &TrustState::default()),
            Some(TrustReport {
                manifest: "{\"m\":1}".into(),
                signature: "sig".into(),
            })
        );
        // The machine caught up: nothing to send.
        assert_eq!(pending_trust_report(&cache, &ahead), None);
    }
}
