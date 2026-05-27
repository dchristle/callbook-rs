//! Decoder for 2025 `hamcall.dat` records.

/// One decoded phase candidate for a 2025 `hamcall.dat` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedV2Candidate {
    /// Phase constant used in the position-dependent XOR stream.
    pub phase: u8,
    /// Decoded bytes for the entire raw IDX slice.
    pub bytes: Vec<u8>,
    /// Heuristic score used to rank candidates.
    pub score: i32,
}

/// Decode one `hamcall.dat` record with a specific phase.
#[must_use]
pub fn decode_phase(dat_offset: u64, raw_bytes: &[u8], phase: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw_bytes.len());
    decode_phase_into(dat_offset, raw_bytes, phase, &mut out);
    out
}

/// Decode one `hamcall.dat` record into a reusable buffer.
pub fn decode_phase_into(dat_offset: u64, raw_bytes: &[u8], phase: u8, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(raw_bytes.len());
    let mut key = (1 - dat_offset as i64 + i64::from(phase)).rem_euclid(101) as usize;
    let mut remaining = raw_bytes;
    while !remaining.is_empty() {
        let n = (101 - key).min(remaining.len());
        out.extend(remaining[..n].iter().enumerate().map(|(i, &encoded)| {
            let key = (key + i) as u8;
            (encoded ^ 7) ^ key
        }));
        remaining = &remaining[n..];
        key = 0;
    }
}

/// Return the decode phase for a 2025 `hamcall.dat` slice.
///
/// The executable decodes against VB's one-based absolute file position:
/// `(encoded ^ 7) ^ ((absolute_position + 3) mod 101)`. The crate decodes a
/// slice using one-based position relative to the IDX offset, so the equivalent
/// phase is `(2 * dat_offset + 3) mod 101`.
#[must_use]
pub fn phase_for_dat_offset(dat_offset: u64) -> u8 {
    ((dat_offset % 101) * 2 + 3).rem_euclid(101) as u8
}

/// Rank all 101 decode phases for a record.
#[must_use]
pub fn decode_candidates(
    dat_offset: u64,
    raw_bytes: &[u8],
    idx_key: &[u8],
) -> Vec<DecodedV2Candidate> {
    let mut candidates: Vec<_> = (0..101)
        .map(|phase| {
            let bytes = decode_phase(dat_offset, raw_bytes, phase);
            let score = score_candidate(&bytes, idx_key);
            DecodedV2Candidate {
                phase,
                bytes,
                score,
            }
        })
        .collect();
    candidates.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.phase.cmp(&b.phase)));
    candidates
}

/// Decode with the phase implied by this record's DAT offset.
#[must_use]
pub fn best_candidate(
    dat_offset: u64,
    raw_bytes: &[u8],
    idx_key: &[u8],
) -> Option<DecodedV2Candidate> {
    let phase = phase_for_dat_offset(dat_offset);
    let bytes = decode_phase(dat_offset, raw_bytes, phase);
    let score = score_candidate(&bytes, idx_key);
    Some(DecodedV2Candidate {
        phase,
        bytes,
        score,
    })
}

fn score_candidate(bytes: &[u8], idx_key: &[u8]) -> i32 {
    let mut score = 0i32;
    score += bytes
        .iter()
        .filter(|&&b| b == b'\t' || b == b'\n' || b == b'\r' || (0x20..=0x7e).contains(&b))
        .count() as i32;
    score -= bytes
        .iter()
        .filter(|&&b| b < 0x09 || (0x0d < b && b < 0x20))
        .count() as i32
        * 3;
    score += bytes
        .iter()
        .filter(|&&b| (0xb5..=0xdf).contains(&b))
        .count() as i32
        * 5;

    let upper = uppercase_ascii(bytes);
    let key_upper = uppercase_ascii(idx_key);
    if contains_subslice(&upper, &key_upper) {
        score += 200;
    }
    if let Some((call, suffix)) = split_once_byte(&key_upper, b':') {
        if contains_subslice(&upper, call) {
            score += 100;
        }
        if bytes.starts_with(suffix) || bytes.starts_with(&key_upper[call.len()..]) {
            score += 150;
        }
    }
    score
}

fn uppercase_ascii(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(u8::to_ascii_uppercase).collect()
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn split_once_byte(bytes: &[u8], byte: u8) -> Option<(&[u8], &[u8])> {
    let pos = bytes.iter().position(|b| *b == byte)?;
    Some((&bytes[..pos], &bytes[pos + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(dat_offset: u64, plain: &[u8], phase: u8) -> Vec<u8> {
        decode_phase(dat_offset, plain, phase)
    }

    #[test]
    fn decodes_position_dependent_xor_phase() {
        let dat_offset = 48;
        let plain = b":2010\xb6A\xbaExample\xbc1 Test Way\xbfExampleville\xc100000";
        let encoded = encode(dat_offset, plain, 99);

        assert_eq!(decode_phase(dat_offset, &encoded, 99), plain);
    }

    #[test]
    fn derives_phase_from_dat_offsets() {
        assert_eq!(phase_for_dat_offset(0), 3);
        assert_eq!(phase_for_dat_offset(1), 5);
        assert_eq!(phase_for_dat_offset(48), 99);
        assert_eq!(phase_for_dat_offset(49), 0);
        assert_eq!(phase_for_dat_offset(1_542), 57);
    }

    #[test]
    fn ranks_matching_phase_first() {
        let dat_offset = 48;
        let plain = b":2010\xb6A\xbaExample\xbc1 Test Way\xbfExampleville\xc100000";
        let encoded = encode(dat_offset, plain, 99);

        let best = best_candidate(dat_offset, &encoded, b"N0CALL:2010").unwrap();

        assert_eq!(best.phase, 99);
        assert_eq!(best.bytes, plain);
    }
}
