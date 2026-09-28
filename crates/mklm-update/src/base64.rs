//! Strict base64 (RFC 4648, padding required, canonical encodings only): the key IDs of public
//! keys and signatures and the fingerprints are read with it (design m5b C.1, C.2).
//!
//! One byte string has exactly one accepted text: the standard alphabet only, the length a multiple
//! of 4, `=` only as the one or two last characters of the text, and the unused bits of the last
//! character zero. No whitespace, no line breaks, no URL-safe alphabet.

/// The value of one character of the standard alphabet.
fn value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The bytes of `text`, or `None` unless it is canonical padded base64.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let (quads, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(quads.len() * 3);
    for (index, quad) in quads.iter().enumerate() {
        let last = index + 1 == quads.len();
        let padding = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let mut values = [0u8; 4];
        for (slot, &c) in values.iter_mut().zip(&quad[..4 - padding]) {
            *slot = value(c)?;
        }
        let word = (u32::from(values[0]) << 18)
            | (u32::from(values[1]) << 12)
            | (u32::from(values[2]) << 6)
            | u32::from(values[3]);
        let [_, b0, b1, b2] = word.to_be_bytes();
        match padding {
            0 => out.extend_from_slice(&[b0, b1, b2]),
            1 => {
                // Three characters carry 18 bits for 2 bytes: the last 2 bits must be zero.
                if values[2] & 0b11 != 0 {
                    return None;
                }
                out.extend_from_slice(&[b0, b1]);
            }
            _ => {
                // Two characters carry 12 bits for 1 byte: the last 4 bits must be zero.
                if values[1] & 0b1111 != 0 {
                    return None;
                }
                out.push(b0);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_test_vectors() {
        for (text, bytes) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmE=", b"fooba"),
            ("Zm9vYmFy", b"foobar"),
        ] {
            assert_eq!(decode(text).as_deref(), Some(bytes), "{text:?}");
        }
        assert_eq!(decode("+/+/").unwrap(), vec![0xFB, 0xFF, 0xBF]);
        assert_eq!(decode("AAAA").unwrap(), vec![0, 0, 0]);
        assert_eq!(decode("////").unwrap(), vec![0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn missing_or_extra_padding_is_refused() {
        for text in [
            "Zg", "Zg=", "Zm8", "Zm9vYg", "Zm9vYmE", "Zg===", "Z===", "====", "Zm9v====",
            "Zg==Zg==", "Zm=v", "=Zm9",
        ] {
            assert_eq!(decode(text), None, "{text:?}");
        }
    }

    #[test]
    fn non_canonical_last_characters_are_refused() {
        // "Zh==" and "Zm9=" decode to the same bytes as "Zg==" and "Zm8=" in lenient decoders.
        for text in ["Zh==", "Zv==", "Zm9=", "Zm+=", "Zm9vYh==", "Zm9vYmF="] {
            assert_eq!(decode(text), None, "{text:?}");
        }
    }

    #[test]
    fn other_alphabets_and_whitespace_are_refused() {
        for text in [
            "Zm9v\n", " Zm9v", "Zm9v ", "Zm 9v", "Zm-v", "Zm_v", "Zm9v\r\n", "Zm9v\0", "Ｚm9v",
        ] {
            assert_eq!(decode(text), None, "{text:?}");
        }
    }
}
