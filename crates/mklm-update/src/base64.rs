//! Strict base64 (RFC 4648, padding required, canonical encodings only): the key IDs of public
//! keys and signatures and the fingerprints are read with it (design m5b C.1, C.2).
//!
//! WP-U implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

/// The bytes of `text`, or `None` unless it is canonical padded base64.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    None // Skeleton (M5b): WP-U
}
