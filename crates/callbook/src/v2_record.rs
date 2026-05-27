//! Parser for decoded 2025 `hamcall.dat` logical records.

use crate::country::CountryTable;
use crate::interest::InterestTable;
use crate::modern::{CallSnapshot, Jurisdiction, SnapshotSource};

/// Split and parse decoded 2025 DAT bytes.
#[cfg(test)]
#[must_use]
pub(crate) fn parse_snapshots(
    decoded: &[u8],
    default_key: Option<&[u8]>,
    source: SnapshotSource,
    countries: Option<&CountryTable>,
    interests: Option<&InterestTable>,
) -> Vec<CallSnapshot> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < decoded.len() {
        let next = decoded[start + usize::from(decoded[start] == 0xb5)..]
            .iter()
            .position(|b| *b == 0xb5)
            .map(|p| start + usize::from(decoded[start] == 0xb5) + p)
            .unwrap_or(decoded.len());
        let segment = &decoded[start..next];
        if let Some(snapshot) = parse_segment(segment, default_key, source, countries, interests) {
            out.push(snapshot);
        }
        start = next;
        if start < decoded.len() && decoded[start] == 0xb5 {
            continue;
        }
    }
    out
}

/// Parse only logical records whose key matches `callsign`.
#[must_use]
pub fn parse_matching_snapshots(
    decoded: &[u8],
    default_key: Option<&[u8]>,
    callsign: &str,
    source: SnapshotSource,
    countries: Option<&CountryTable>,
    interests: Option<&InterestTable>,
) -> Vec<CallSnapshot> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < decoded.len() {
        let next = decoded[start + usize::from(decoded[start] == 0xb5)..]
            .iter()
            .position(|b| *b == 0xb5)
            .map(|p| start + usize::from(decoded[start] == 0xb5) + p)
            .unwrap_or(decoded.len());
        let segment = &decoded[start..next];
        if segment_matches(segment, default_key, callsign) {
            if let Some(snapshot) =
                parse_segment(segment, default_key, source, countries, interests)
            {
                out.push(snapshot);
            }
        }
        start = next;
        if start < decoded.len() && decoded[start] == 0xb5 {
            continue;
        }
    }
    out
}

/// Parse only complete logical records whose key matches `callsign`.
#[must_use]
pub fn parse_complete_matching_snapshots(
    decoded: &[u8],
    default_key: Option<&[u8]>,
    callsign: &str,
    source: SnapshotSource,
    countries: Option<&CountryTable>,
    interests: Option<&InterestTable>,
) -> Vec<CallSnapshot> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < decoded.len() {
        let search_start = start + usize::from(decoded[start] == 0xb5);
        let Some(next) = decoded[search_start..]
            .iter()
            .position(|b| *b == 0xb5)
            .map(|p| search_start + p)
        else {
            break;
        };
        let segment = &decoded[start..next];
        if segment_matches(segment, default_key, callsign) {
            if let Some(snapshot) =
                parse_segment(segment, default_key, source, countries, interests)
            {
                out.push(snapshot);
            }
        }
        start = next;
    }
    out
}

fn parse_segment(
    mut segment: &[u8],
    default_key: Option<&[u8]>,
    source: SnapshotSource,
    countries: Option<&CountryTable>,
    interests: Option<&InterestTable>,
) -> Option<CallSnapshot> {
    if segment.first() == Some(&0xb5) {
        segment = &segment[1..];
    }
    if segment.is_empty() {
        return None;
    }

    let key_end = segment
        .iter()
        .position(|b| (0xb6..=0xdf).contains(b))
        .unwrap_or(segment.len());
    let mut key = clean_ascii(&segment[..key_end])?;
    if key.starts_with(':') {
        let default = clean_ascii(default_key?)?;
        let call = default
            .split_once(':')
            .map_or(default.as_str(), |(call, _)| call);
        key = format!("{call}{key}");
    }
    if !looks_like_key(&key) {
        return None;
    }
    let (callsign, vintage) = split_key(&key)?;
    let mut snapshot = CallSnapshot::new(callsign, vintage, source);
    let mut i = key_end;
    while i < segment.len() {
        let tag = segment[i];
        if !(0xb6..=0xdf).contains(&tag) {
            i += 1;
            continue;
        }
        i += 1;
        let value_start = i;
        while i < segment.len() && !(0xb5..=0xdf).contains(&segment[i]) {
            i += 1;
        }
        let Some(value) = clean_ascii(&segment[value_start..i]) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        apply_tag(&mut snapshot, tag, value);
    }
    apply_country(&mut snapshot, countries);
    apply_interests(&mut snapshot, interests);
    Some(snapshot)
}

fn segment_matches(mut segment: &[u8], default_key: Option<&[u8]>, callsign: &str) -> bool {
    if segment.first() == Some(&0xb5) {
        segment = &segment[1..];
    }
    if segment.is_empty() {
        return false;
    }

    let key_end = segment
        .iter()
        .position(|b| (0xb6..=0xdf).contains(b))
        .unwrap_or(segment.len());
    let key = trim_ascii(&segment[..key_end]);
    if !key.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return false;
    }
    if let Some(rest) = key.strip_prefix(b":") {
        if rest.is_empty() {
            return false;
        }
        let Some(default) = default_key.map(trim_ascii) else {
            return false;
        };
        let call = split_once_byte(default, b':').map_or(default, |(call, _)| call);
        return ascii_eq_ignore_case(call, callsign.as_bytes());
    }
    let call = split_once_byte(key, b':').map_or(key, |(call, _)| call);
    ascii_eq_ignore_case(call, callsign.as_bytes())
}

fn apply_tag(snapshot: &mut CallSnapshot, tag: u8, value: String) {
    match tag {
        0xb6 => snapshot.license_class = Some(value),
        0xb7 => snapshot.record_code = Some(value),
        0xb8 => snapshot.first_name = Some(value),
        0xb9 => snapshot.middle_name = Some(value),
        0xba => snapshot.last_name = Some(value),
        0xbb => snapshot.suffix = Some(value),
        0xbc..=0xbe => snapshot.address = Some(value),
        0xbf => snapshot.city = Some(value),
        0xc0 => snapshot.state_or_province = Some(value),
        0xc1 => snapshot.postal_code = Some(value),
        0xc2 => snapshot.birth_date = Some(value),
        0xc3 => snapshot.first_issued = Some(value),
        0xc4 => snapshot.expires = Some(value),
        0xc5 => snapshot.last_changed = Some(value),
        0xc6 => snapshot.county = Some(value),
        0xc7 => snapshot.gmt_offset = Some(value),
        0xc8 => snapshot.latitude = Some(value),
        0xc9 => snapshot.longitude = Some(value),
        0xca => snapshot.grid = Some(value),
        0xcb => snapshot.area_code = Some(value),
        0xcc => snapshot.previous_call = Some(value),
        0xcd => snapshot.previous_class = Some(value),
        0xce => snapshot.fcc_transaction_type = Some(value),
        0xcf => snapshot.email = Some(value),
        0xd0 => snapshot.qsl = Some(value),
        0xd1 => snapshot.country = Some(value),
        0xd2 => snapshot.url = Some(value),
        0xd4 => snapshot.fax_number = Some(value),
        0xd5 => snapshot.interest_codes_raw = Some(value),
        0xd7 => snapshot.license_id = Some(value),
        0xd9 => snapshot.frn = Some(value),
        0xda => snapshot.iota = Some(value),
        0xde => snapshot.numeric_id = Some(value),
        _ => {
            snapshot.raw_tags.insert(tag, value);
        }
    }
}

fn apply_interests(snapshot: &mut CallSnapshot, interests: Option<&InterestTable>) {
    let Some(raw) = &snapshot.interest_codes_raw else {
        return;
    };
    snapshot.interest_codes = InterestTable::codes(raw);
    if let Some(table) = interests {
        snapshot.interests = table.resolve_raw(raw);
    }
}

fn apply_country(snapshot: &mut CallSnapshot, countries: Option<&CountryTable>) {
    if snapshot.country.is_none() {
        if let Some(country) = countries.and_then(|table| table.lookup(&snapshot.callsign)) {
            snapshot.country = Some(country.name);
            snapshot.jurisdiction = country.jurisdiction;
            return;
        }
    }
    if matches!(snapshot.jurisdiction, Jurisdiction::Unknown) {
        snapshot.jurisdiction = match snapshot.country.as_deref() {
            Some("United States" | "UNITED STATES") => Jurisdiction::UnitedStates,
            Some("Canada" | "CANADA") => Jurisdiction::Canada,
            Some(_) => Jurisdiction::International,
            None if snapshot.state_or_province.is_some() && snapshot.postal_code.is_some() => {
                Jurisdiction::UnitedStates
            }
            None => Jurisdiction::Unknown,
        };
    }
}

fn split_key(key: &str) -> Option<(String, Option<u16>)> {
    if let Some((call, vintage)) = key.split_once(':') {
        let year = vintage.parse::<u16>().ok()?;
        Some((call.to_owned(), Some(year)))
    } else {
        Some((key.to_owned(), None))
    }
}

fn looks_like_key(key: &str) -> bool {
    let call = key.split_once(':').map_or(key, |(call, _)| call);
    !call.is_empty()
        && call.len() <= 12
        && call
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/')
}

fn clean_ascii(bytes: &[u8]) -> Option<String> {
    let trimmed = trim_ascii(bytes);
    if trimmed.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        Some(String::from_utf8_lossy(trimmed).into_owned())
    } else {
        None
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace() && *b != 0)
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace() && *b != 0)
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn split_once_byte(bytes: &[u8], byte: u8) -> Option<(&[u8], &[u8])> {
    let pos = bytes.iter().position(|b| *b == byte)?;
    Some((&bytes[..pos], &bytes[pos + 1..]))
}

fn ascii_eq_ignore_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(&left, &right)| left.eq_ignore_ascii_case(&right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decoded_tagged_record() {
        let bytes =
            b"\xb5K0ABC:2015\xb6G\xb8Jane\xb9Q\xbaExample\xbc123 Test St\xbfSpringfield\xc0IL\xc162704";
        let snapshots = parse_snapshots(bytes, None, SnapshotSource::HamCallHci, None, None);
        assert_eq!(snapshots.len(), 1);
        let rec = &snapshots[0];
        assert_eq!(rec.callsign, "K0ABC");
        assert_eq!(rec.vintage, Some(2015));
        assert_eq!(rec.first_name.as_deref(), Some("Jane"));
        assert_eq!(rec.city.as_deref(), Some("Springfield"));
    }

    #[test]
    fn reconstructs_leading_suffix_key() {
        let bytes = b":2010\xb6A\xbaExample";
        let snapshots = parse_snapshots(
            bytes,
            Some(b"S51DX:2010"),
            SnapshotSource::HamCallDatIdx,
            None,
            None,
        );
        assert_eq!(snapshots[0].callsign, "S51DX");
        assert_eq!(snapshots[0].vintage, Some(2010));
    }

    #[test]
    fn matching_parser_skips_neighbor_records() {
        let bytes = b"\xb5K0AB\xb8Neighbor\xb5K0ABC:2015\xb8Jane\xbfSpringfield";
        let snapshots =
            parse_matching_snapshots(bytes, None, "K0ABC", SnapshotSource::HamCallHci, None, None);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].callsign, "K0ABC");
        assert_eq!(snapshots[0].first_name.as_deref(), Some("Jane"));
    }

    #[test]
    fn resolves_interest_codes_from_catalog() {
        let table = InterestTable::parse(
            "---Bands\n\
             0010*160 meters\n\
             0020*80 meters\n",
        );
        let bytes = b"\xb5K0AB\xb8Example\xd5001000209999";

        let snapshots = parse_snapshots(
            bytes,
            None,
            SnapshotSource::HamCallDatIdx,
            None,
            Some(&table),
        );

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].interest_codes, vec!["0010", "0020", "9999"]);
        assert_eq!(
            snapshots[0]
                .interests
                .iter()
                .map(|interest| (interest.code.as_str(), interest.label.as_str()))
                .collect::<Vec<_>>(),
            vec![("0010", "160 meters"), ("0020", "80 meters")]
        );
    }

    #[test]
    fn complete_matching_parser_ignores_trailing_partial_record() {
        let bytes = b"\xb5K0ABC:2015\xb8Jane";
        let snapshots = parse_complete_matching_snapshots(
            bytes,
            None,
            "K0ABC",
            SnapshotSource::HamCallHci,
            None,
            None,
        );
        assert!(snapshots.is_empty());

        let bytes = b"\xb5K0ABC:2015\xb8Jane\xb5K0AB\xbfDenver";
        let snapshots = parse_complete_matching_snapshots(
            bytes,
            None,
            "K0ABC",
            SnapshotSource::HamCallHci,
            None,
            None,
        );
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].first_name.as_deref(), Some("Jane"));
    }
}
