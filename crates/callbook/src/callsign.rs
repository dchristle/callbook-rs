//! Callsign normalization and storage.
//!
//! HamCall stores keys as 26-byte ASCII, uppercase, right-padded with spaces.
//! This module exposes a fixed-size stack-resident type that performs the
//! normalization once and is then used as the search key.

use crate::error::{Error, Result};
use crate::format::KEY_LEN;

/// A normalized callsign in HamCall key form: uppercase ASCII, right-padded
/// with spaces to 26 bytes.
///
/// Construction is the only place where validation happens; afterwards the
/// key is just bytes and can be passed around `Copy`-cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Callsign([u8; KEY_LEN]);

impl Callsign {
    /// Build a [`Callsign`] from a string slice. Lowercase letters are
    /// uppercased; the buffer is right-padded with spaces. Returns
    /// [`Error::InvalidCallsign`] for empty input, oversized input, or
    /// characters outside `[A-Z0-9/]` (after uppercasing).
    pub fn parse(input: &str) -> Result<Self> {
        let bytes = input.as_bytes();
        let trimmed = strip_ascii_whitespace(bytes);
        if trimmed.is_empty() {
            return Err(Error::InvalidCallsign(input.to_owned()));
        }
        if trimmed.len() > KEY_LEN {
            return Err(Error::InvalidCallsign(input.to_owned()));
        }
        let mut out = [b' '; KEY_LEN];
        for (dst, &b) in out.iter_mut().zip(trimmed.iter()) {
            let c = ascii_upper(b);
            if !is_callsign_char(c) {
                return Err(Error::InvalidCallsign(input.to_owned()));
            }
            *dst = c;
        }
        Ok(Self(out))
    }

    /// Borrow the 26-byte key.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Render the callsign as a string slice, trimmed of trailing spaces.
    /// Always valid ASCII.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        let end = self.0.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
        // SAFETY: every byte is validated ASCII at construction time.
        unsafe { std::str::from_utf8_unchecked(&self.0[..end]) }
    }
}

impl std::fmt::Display for Callsign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[inline]
fn ascii_upper(b: u8) -> u8 {
    if b.is_ascii_lowercase() {
        b & !0x20
    } else {
        b
    }
}

#[inline]
fn is_callsign_char(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/'
}

#[inline]
fn strip_ascii_whitespace(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = s {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uppercase() {
        let c = Callsign::parse("W1AW").unwrap();
        assert_eq!(c.as_str(), "W1AW");
        assert_eq!(&c.as_bytes()[..4], b"W1AW");
        assert!(c.as_bytes()[4..].iter().all(|&b| b == b' '));
    }

    #[test]
    fn parses_lowercase() {
        let c = Callsign::parse("w1aw").unwrap();
        assert_eq!(c.as_str(), "W1AW");
    }

    #[test]
    fn allows_slash() {
        let c = Callsign::parse("KH6/W1AW").unwrap();
        assert_eq!(c.as_str(), "KH6/W1AW");
    }

    #[test]
    fn pads_to_26() {
        let c = Callsign::parse("K1A").unwrap();
        assert_eq!(c.as_bytes().len(), KEY_LEN);
        assert_eq!(&c.as_bytes()[..3], b"K1A");
    }

    #[test]
    fn trims_whitespace() {
        let c = Callsign::parse("  w1aw  ").unwrap();
        assert_eq!(c.as_str(), "W1AW");
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(
            Callsign::parse(""),
            Err(Error::InvalidCallsign(_))
        ));
        assert!(matches!(
            Callsign::parse("   "),
            Err(Error::InvalidCallsign(_))
        ));
    }

    #[test]
    fn rejects_oversize() {
        let too_long = "A".repeat(KEY_LEN + 1);
        assert!(matches!(
            Callsign::parse(&too_long),
            Err(Error::InvalidCallsign(_))
        ));
    }

    #[test]
    fn rejects_bad_chars() {
        for s in ["w1aw!", "k-1a", "abc def", "n4@"] {
            assert!(
                matches!(Callsign::parse(s), Err(Error::InvalidCallsign(_))),
                "{s:?} should be rejected",
            );
        }
    }

    #[test]
    fn ord_matches_byte_order() {
        let a = Callsign::parse("AA1A").unwrap();
        let b = Callsign::parse("ZZ9Z").unwrap();
        assert!(a < b);
        let c = Callsign::parse("W1AW").unwrap();
        let d = Callsign::parse("W1AX").unwrap();
        assert!(c < d);
    }
}
