//! Reader for the 2025 `hciindex.dat` + `hci.dat` corpus.
//!
//! The HCI pair is an inverted index into `hamcall.dat`: the index file is a
//! big-endian u32 offset table into `hci.dat`; each HCI entry starts with an
//! encoded search-key header and may contain raw 5-byte postings.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::error::{Error, Result};

/// Open HCI index/data pair.
pub struct HciFile {
    index_path: PathBuf,
    dat_path: PathBuf,
    index_mmap: Mmap,
    dat_mmap: Mmap,
    key_index: Vec<HciKeyRef>,
    publication_years: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HciKeyRef {
    hash: u64,
    index: u32,
}

/// One raw HCI record, addressed by ordinal in `hciindex.dat`.
#[derive(Debug, Clone)]
pub struct RawHciRecord<'a> {
    /// Zero-based ordinal in the HCI offset table.
    pub index: usize,
    /// Byte offset into `hci.dat`.
    pub dat_offset: u64,
    /// Length in bytes.
    pub raw_len: usize,
    /// Borrowed encoded record bytes.
    pub raw_bytes: &'a [u8],
}

/// Decoded HCI record bytes.
#[derive(Debug, Clone)]
pub struct DecodedHciRecord {
    /// Zero-based ordinal in the HCI offset table.
    pub index: usize,
    /// Byte offset into `hci.dat`.
    pub dat_offset: u64,
    /// Decoded bytes, including the in-band terminator when one was found.
    pub bytes: Vec<u8>,
    /// Whether decoding stopped because it saw a decoded byte > `0xB4`.
    pub terminated: bool,
}

/// One posting inside an HCI posting list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HciPosting {
    /// Byte offset into `hamcall.dat`.
    pub dat_offset: u64,
    /// Extra position byte from the 5-byte posting.
    pub position: u8,
}

/// Invariant report for an HCI pair.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    /// Number of u32 offsets in `hciindex.dat`.
    pub entries: usize,
    /// Size of `hci.dat` in bytes.
    pub dat_size: u64,
    /// First offset in the table.
    pub first_offset: Option<u64>,
    /// Last offset in the table.
    pub last_offset: Option<u64>,
    /// Adjacent offsets that are not strictly increasing.
    pub offset_order_violations: usize,
    /// Offsets that point past `hci.dat`.
    pub out_of_bounds_offsets: usize,
    /// Records with zero byte length.
    pub zero_length_records: usize,
    /// Shortest computed record length.
    pub min_record_len: Option<usize>,
    /// Longest computed record length.
    pub max_record_len: Option<usize>,
}

/// Callsign-looking posting counts derived from HCI search keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallsignPostingStatistics {
    /// Total postings under current-looking callsign keys.
    pub current_postings: usize,
    /// Unique `hamcall.dat` offsets under current-looking callsign keys.
    pub current_unique_dat_offsets: usize,
    /// Total 5-byte postings under archive-looking HCI keys.
    pub archive_postings: usize,
    /// Unique `hamcall.dat` offsets referenced by those postings.
    pub archive_unique_dat_offsets: usize,
    /// Per-year posting and unique-offset counts.
    pub archive_years: Vec<ArchivePostingYearStatistics>,
}

/// Archive-posting counts for one HCI publication year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePostingYearStatistics {
    /// Archive publication year.
    pub year: u16,
    /// Total 5-byte postings under archive-looking HCI keys for this year.
    pub postings: usize,
    /// Unique `hamcall.dat` offsets referenced by postings for this year.
    pub unique_dat_offsets: usize,
}

impl HciFile {
    /// Open an HCI index/data pair.
    pub fn open(index_path: impl AsRef<Path>, dat_path: impl AsRef<Path>) -> Result<Self> {
        let index_path = index_path.as_ref();
        let dat_path = dat_path.as_ref();
        let index_file = File::open(index_path)?;
        let dat_file = File::open(dat_path)?;
        // SAFETY: read-only maps of stable database files.
        let index_mmap = unsafe { Mmap::map(&index_file)? };
        // SAFETY: read-only maps of stable database files.
        let dat_mmap = unsafe { Mmap::map(&dat_file)? };
        if index_mmap.len() % 4 != 0 {
            return Err(Error::MalformedHeader {
                path: index_path.to_owned(),
                reason: "HCI index size is not a multiple of 4 bytes",
            });
        }
        if index_mmap.is_empty() {
            return Err(Error::MalformedHeader {
                path: index_path.to_owned(),
                reason: "HCI index is empty",
            });
        }
        let mut out = Self {
            index_path: index_path.to_owned(),
            dat_path: dat_path.to_owned(),
            index_mmap,
            dat_mmap,
            key_index: Vec::new(),
            publication_years: Vec::new(),
        };
        let (key_index, publication_years) = build_key_index(&out);
        out.key_index = key_index;
        out.publication_years = publication_years;
        Ok(out)
    }

    /// Number of offsets in `hciindex.dat`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index_mmap.len() / 4
    }

    /// Path to `hciindex.dat`.
    #[must_use]
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    /// Path to `hci.dat`.
    #[must_use]
    pub fn dat_path(&self) -> &Path {
        &self.dat_path
    }

    /// Big-endian offset value at `index`.
    #[must_use]
    pub fn offset(&self, index: usize) -> Option<u64> {
        let start = index.checked_mul(4)?;
        let bytes = self.index_mmap.get(start..start + 4)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?) as u64)
    }

    /// Return one still-encoded HCI record by ordinal.
    #[must_use]
    pub fn raw_record(&self, index: usize) -> Option<RawHciRecord<'_>> {
        let dat_size = self.dat_mmap.len() as u64;
        let start = self.offset(index)?;
        let end = self.offset(index + 1).unwrap_or(dat_size);
        if start > dat_size || end > dat_size || end < start {
            return None;
        }
        let raw_bytes = &self.dat_mmap[start as usize..end as usize];
        Some(RawHciRecord {
            index,
            dat_offset: start,
            raw_len: raw_bytes.len(),
            raw_bytes,
        })
    }

    /// Decode one small HCI record by ordinal.
    ///
    /// Reads at most 100 bytes from `hci.dat`, then decodes with the
    /// position-dependent XOR stream. The decoded byte stream terminates when
    /// a byte greater than `0xB4` is produced.
    #[must_use]
    pub fn decode_record(&self, index: usize) -> Option<DecodedHciRecord> {
        let raw = self.raw_record(index)?;
        Some(decode_raw_record(raw))
    }

    /// Return postings for an exact decoded HCI search key.
    #[must_use]
    pub fn postings_for_key(&self, key: &[u8]) -> Vec<HciPosting> {
        let mut out = Vec::new();
        self.visit_postings_for_key(key, |posting| out.push(posting));
        out
    }

    /// Visit postings for an exact decoded HCI search key without allocating a list.
    pub fn visit_postings_for_key(&self, key: &[u8], mut visit: impl FnMut(HciPosting)) {
        let hash = hash_key(key);
        let start = self.key_index.partition_point(|entry| entry.hash < hash);
        let end = self.key_index.partition_point(|entry| entry.hash <= hash);
        for entry in &self.key_index[start..end] {
            let Some(raw) = self.raw_record(entry.index as usize) else {
                continue;
            };
            let Some(header_len) = decoded_header_matches(raw.dat_offset, raw.raw_bytes, key)
            else {
                continue;
            };
            visit_postings(&raw.raw_bytes[header_len..], &mut visit);
        }
    }

    /// Visit raw HCI records whose decoded header exactly matches `key`.
    pub fn visit_records_for_key(
        &self,
        key: &[u8],
        mut visit: impl FnMut(RawHciRecord<'_>, usize),
    ) {
        let hash = hash_key(key);
        let start = self.key_index.partition_point(|entry| entry.hash < hash);
        let end = self.key_index.partition_point(|entry| entry.hash <= hash);
        for entry in &self.key_index[start..end] {
            let Some(raw) = self.raw_record(entry.index as usize) else {
                continue;
            };
            let Some(header_len) = decoded_header_matches(raw.dat_offset, raw.raw_bytes, key)
            else {
                continue;
            };
            visit(raw, header_len);
        }
    }

    /// Number of exact-key headers indexed at open time.
    #[must_use]
    pub fn indexed_key_count(&self) -> usize {
        self.key_index.len()
    }

    /// Years that appear in decoded HCI archive keys.
    #[must_use]
    pub fn publication_years(&self) -> &[u16] {
        &self.publication_years
    }

    /// Count callsign-looking HCI postings and unique DAT offsets.
    ///
    /// HCI is an inverted index, so postings are search-index hits rather
    /// than records. Unique DAT offsets are the closest cheap estimate of
    /// records represented through callsign-like keys.
    #[must_use]
    pub fn callsign_posting_statistics(&self) -> CallsignPostingStatistics {
        #[derive(Default)]
        struct YearAccumulator {
            postings: usize,
            offsets: BTreeSet<u64>,
        }

        let mut current_postings = 0usize;
        let mut current_offsets = BTreeSet::new();
        let mut archive_years = BTreeMap::<u16, YearAccumulator>::new();
        for index in 0..self.len() {
            let Some(raw) = self.raw_record(index) else {
                continue;
            };
            let Some(header) = decode_header_key(raw.dat_offset, raw.raw_bytes) else {
                continue;
            };
            if !looks_like_callsign_header(&header.key) {
                continue;
            };
            if let Some(year) = header.year {
                if !(1900..=2099).contains(&year) {
                    continue;
                }
                let acc = archive_years.entry(year).or_default();
                visit_postings(&raw.raw_bytes[header.header_len..], |posting| {
                    acc.postings += 1;
                    acc.offsets.insert(posting.dat_offset);
                });
            } else {
                visit_postings(&raw.raw_bytes[header.header_len..], |posting| {
                    current_postings += 1;
                    current_offsets.insert(posting.dat_offset);
                });
            }
        }

        let mut archive_postings = 0usize;
        let mut archive_offsets = BTreeSet::new();
        let archive_years = archive_years
            .into_iter()
            .map(|(year, acc)| {
                archive_postings += acc.postings;
                archive_offsets.extend(acc.offsets.iter().copied());
                ArchivePostingYearStatistics {
                    year,
                    postings: acc.postings,
                    unique_dat_offsets: acc.offsets.len(),
                }
            })
            .collect();
        CallsignPostingStatistics {
            current_postings,
            current_unique_dat_offsets: current_offsets.len(),
            archive_postings,
            archive_unique_dat_offsets: archive_offsets.len(),
            archive_years,
        }
    }

    /// Visit every posting under a callsign-looking HCI key.
    pub fn visit_callsign_postings(&self, mut visit: impl FnMut(&[u8], HciPosting)) {
        for index in 0..self.len() {
            let Some(raw) = self.raw_record(index) else {
                continue;
            };
            let Some(header) = decode_header_key(raw.dat_offset, raw.raw_bytes) else {
                continue;
            };
            if !looks_like_callsign_header(&header.key) {
                continue;
            }
            visit_postings(&raw.raw_bytes[header.header_len..], |posting| {
                visit(&header.key, posting);
            });
        }
    }
}

/// Decode a raw HCI record.
#[must_use]
pub fn decode_raw_record(raw: RawHciRecord<'_>) -> DecodedHciRecord {
    let (bytes, _) = decode_header(raw.dat_offset, raw.raw_bytes);
    let terminated = bytes.last().is_some_and(|b| *b > 0xb4);
    DecodedHciRecord {
        index: raw.index,
        dat_offset: raw.dat_offset,
        bytes,
        terminated,
    }
}

fn build_key_index(hci: &HciFile) -> (Vec<HciKeyRef>, Vec<u16>) {
    let mut entries = Vec::new();
    let mut years = Vec::new();
    let dat_size = hci.dat_mmap.len() as u64;
    let mut previous: Option<(usize, u64)> = None;

    for (index, offset_bytes) in hci.index_mmap.chunks_exact(4).enumerate() {
        let current = u32::from_be_bytes(offset_bytes.try_into().expect("chunk len")) as u64;
        if let Some((previous_index, previous_offset)) = previous {
            push_hci_key_ref(
                hci,
                previous_index,
                previous_offset,
                current,
                dat_size,
                &mut entries,
                &mut years,
            );
        }
        previous = Some((index, current));
    }
    if let Some((index, offset)) = previous {
        push_hci_key_ref(
            hci,
            index,
            offset,
            dat_size,
            dat_size,
            &mut entries,
            &mut years,
        );
    }
    entries.sort_unstable_by(|a, b| a.hash.cmp(&b.hash).then_with(|| a.index.cmp(&b.index)));
    years.sort_unstable();
    years.dedup();
    (entries, years)
}

fn push_hci_key_ref(
    hci: &HciFile,
    index: usize,
    start: u64,
    end: u64,
    dat_size: u64,
    entries: &mut Vec<HciKeyRef>,
    years: &mut Vec<u16>,
) {
    if start > dat_size || end > dat_size || end < start {
        return;
    }
    let raw_bytes = &hci.dat_mmap[start as usize..end as usize];
    let Some(header) = decode_header_key(start, raw_bytes) else {
        return;
    };
    let Ok(index) = u32::try_from(index) else {
        return;
    };
    entries.push(HciKeyRef {
        hash: header.hash,
        index,
    });
    if let Some(year) = header.year {
        years.push(year);
    }
}

struct HciHeaderKey {
    hash: u64,
    key: Vec<u8>,
    year: Option<u16>,
    header_len: usize,
}

fn decode_header_key(dat_offset: u64, raw_bytes: &[u8]) -> Option<HciHeaderKey> {
    let mut hash = 0xcbf29ce484222325u64;
    let mut key = Vec::new();
    let mut colon_at = None;
    let mut year = 0u16;
    let mut year_digits = 0usize;

    for (zero_based, &encoded) in raw_bytes.iter().take(100).enumerate() {
        let decoded = decode_hci_byte(dat_offset, zero_based, encoded);
        if decoded == 0xb5 {
            if zero_based == 0 {
                return None;
            }
            let year = (year_digits == 4).then_some(year);
            return Some(HciHeaderKey {
                hash,
                key,
                year,
                header_len: zero_based + 1,
            });
        }
        if decoded > 0xb4 || !(decoded.is_ascii_graphic() || decoded == b' ') {
            return None;
        }

        key.push(decoded);
        hash ^= u64::from(decoded);
        hash = hash.wrapping_mul(0x100000001b3);
        if decoded == b':' {
            colon_at = Some(zero_based);
            year = 0;
            year_digits = 0;
        } else if colon_at.is_some() {
            if !decoded.is_ascii_digit() || year_digits >= 4 {
                colon_at = None;
                year_digits = 0;
            } else {
                year = year * 10 + u16::from(decoded - b'0');
                year_digits += 1;
            }
        }
    }
    None
}

fn looks_like_callsign_header(key: &[u8]) -> bool {
    let call = key
        .split(|byte| *byte == b':')
        .next()
        .map_or(key, trim_ascii_whitespace);
    !call.is_empty()
        && call.len() <= 12
        && call.iter().any(u8::is_ascii_alphabetic)
        && call.iter().any(u8::is_ascii_digit)
        && call
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'/')
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn decode_header(dat_offset: u64, raw_bytes: &[u8]) -> (Vec<u8>, usize) {
    let mut bytes = Vec::with_capacity(raw_bytes.len().min(100));
    for (zero_based, &encoded) in raw_bytes.iter().take(100).enumerate() {
        let decoded = decode_hci_byte(dat_offset, zero_based, encoded);
        bytes.push(decoded);
        if decoded > 0xb4 {
            return (bytes, zero_based + 1);
        }
    }
    let len = bytes.len();
    (bytes, len)
}

fn decoded_header_matches(dat_offset: u64, raw_bytes: &[u8], key: &[u8]) -> Option<usize> {
    for (zero_based, &encoded) in raw_bytes.iter().take(100).enumerate() {
        let decoded = decode_hci_byte(dat_offset, zero_based, encoded);
        if zero_based < key.len() {
            if decoded != key[zero_based] {
                return None;
            }
        } else if decoded == 0xb5 {
            return Some(zero_based + 1);
        } else {
            return None;
        }
    }
    None
}

fn decode_hci_byte(dat_offset: u64, zero_based: usize, encoded: u8) -> u8 {
    let stream_key = ((dat_offset + zero_based as u64 + 4) % 101) as u8;
    (encoded ^ 7) ^ stream_key
}

fn visit_postings(bytes: &[u8], mut visit: impl FnMut(HciPosting)) {
    let usable = bytes.len().saturating_sub(1);
    for chunk in bytes[..usable].chunks_exact(5) {
        let offset = u32::from_be_bytes(chunk[0..4].try_into().expect("chunk len")) as u64;
        visit(HciPosting {
            dat_offset: offset,
            position: chunk[4],
        });
    }
}

fn hash_key(key: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in key {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Verify the offset-table invariants for an HCI pair.
#[must_use]
pub fn verify(hci: &HciFile) -> VerifyReport {
    let entries = hci.len();
    let dat_size = hci.dat_mmap.len() as u64;
    let first_offset = hci.offset(0);
    let last_offset = entries.checked_sub(1).and_then(|i| hci.offset(i));
    let mut offset_order_violations = 0usize;
    let mut out_of_bounds_offsets = 0usize;
    let mut zero_length_records = 0usize;
    let mut min_record_len: Option<usize> = None;
    let mut max_record_len: Option<usize> = None;

    let mut prev = None;
    for i in 0..entries {
        let Some(off) = hci.offset(i) else {
            out_of_bounds_offsets += 1;
            continue;
        };
        if off > dat_size {
            out_of_bounds_offsets += 1;
        }
        if let Some(prev_off) = prev {
            if off <= prev_off {
                offset_order_violations += 1;
            }
            let len = off.saturating_sub(prev_off) as usize;
            if len == 0 {
                zero_length_records += 1;
            }
            min_record_len = Some(min_record_len.map_or(len, |m| m.min(len)));
            max_record_len = Some(max_record_len.map_or(len, |m| m.max(len)));
        }
        prev = Some(off);
    }

    if let Some(last) = last_offset {
        let len = dat_size.saturating_sub(last) as usize;
        if len == 0 {
            zero_length_records += 1;
        }
        min_record_len = Some(min_record_len.map_or(len, |m| m.min(len)));
        max_record_len = Some(max_record_len.map_or(len, |m| m.max(len)));
    }

    VerifyReport {
        entries,
        dat_size,
        first_offset,
        last_offset,
        offset_order_violations,
        out_of_bounds_offsets,
        zero_length_records,
        min_record_len,
        max_record_len,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn write_file(bytes: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    fn encode_header(offset: u64, plain: &[u8]) -> Vec<u8> {
        plain
            .iter()
            .enumerate()
            .map(|(zero_based, &decoded)| {
                let key = ((offset + zero_based as u64 + 4) % 101) as u8;
                (decoded ^ key) ^ 7
            })
            .collect()
    }

    fn posting(offset: u32) -> [u8; 5] {
        let mut out = [0; 5];
        out[..4].copy_from_slice(&offset.to_be_bytes());
        out
    }

    #[test]
    fn reads_big_endian_offsets_and_raw_records() {
        let idx = write_file(&[
            0, 0, 0, 1, //
            0, 0, 0, 4, //
            0, 0, 0, 8,
        ]);
        let dat = write_file(b"#aaabbbbcc");
        let hci = HciFile::open(idx.path(), dat.path()).unwrap();

        assert_eq!(hci.len(), 3);
        assert_eq!(hci.offset(0), Some(1));
        assert_eq!(hci.offset(2), Some(8));
        assert_eq!(hci.raw_record(0).unwrap().raw_bytes, b"aaa");
        assert_eq!(hci.raw_record(1).unwrap().raw_bytes, b"bbbb");
        assert_eq!(hci.raw_record(2).unwrap().raw_bytes, b"cc");
    }

    #[test]
    fn verify_reports_offset_invariants() {
        let idx = write_file(&[
            0, 0, 0, 1, //
            0, 0, 0, 4, //
            0, 0, 0, 8,
        ]);
        let dat = write_file(b"#aaabbbbcc");
        let hci = HciFile::open(idx.path(), dat.path()).unwrap();
        let rep = verify(&hci);

        assert_eq!(rep.entries, 3);
        assert_eq!(rep.first_offset, Some(1));
        assert_eq!(rep.last_offset, Some(8));
        assert_eq!(rep.offset_order_violations, 0);
        assert_eq!(rep.out_of_bounds_offsets, 0);
        assert_eq!(rep.min_record_len, Some(2));
        assert_eq!(rep.max_record_len, Some(4));
    }

    #[test]
    fn decodes_position_and_offset_keyed_record() {
        let idx = write_file(&[
            0, 0, 0, 1, //
            0, 0, 0, 5,
        ]);
        let plain = [b'A', b'B', 0xB5];
        let offset = 1i64;
        let encoded: Vec<u8> = plain
            .iter()
            .enumerate()
            .map(|(zero_based, &decoded)| {
                let one_based = zero_based as i64 + 1;
                let key = (offset + one_based + 3).rem_euclid(101) as u8;
                (decoded ^ key) ^ 7
            })
            .collect();
        let mut body = vec![b'#'];
        body.extend_from_slice(&encoded);
        body.push(b'x');
        let dat = write_file(&body);
        let hci = HciFile::open(idx.path(), dat.path()).unwrap();

        let rec = hci.decode_record(0).unwrap();
        assert_eq!(rec.bytes, plain);
        assert!(rec.terminated);
    }

    #[test]
    fn indexes_decoded_header_keys_and_publication_years() {
        let idx = write_file(&[
            0, 0, 0, 1, //
            0, 0, 0, 10,
        ]);
        let plain = *b"AB:2020\xB5";
        let offset = 1u64;
        let encoded: Vec<u8> = plain
            .iter()
            .enumerate()
            .map(|(zero_based, &decoded)| {
                let key = ((offset + zero_based as u64 + 4) % 101) as u8;
                (decoded ^ key) ^ 7
            })
            .collect();
        let mut body = vec![b'#'];
        body.extend_from_slice(&encoded);
        body.push(b'x');
        let dat = write_file(&body);
        let hci = HciFile::open(idx.path(), dat.path()).unwrap();

        assert_eq!(hci.indexed_key_count(), 1);
        assert_eq!(hci.publication_years(), &[2020]);
    }

    #[test]
    fn callsign_posting_statistics_counts_current_and_archive_offsets() {
        let mut body = vec![b'#'];
        let mut offsets = Vec::new();

        offsets.push(body.len() as u32);
        body.extend_from_slice(&encode_header(body.len() as u64, b"K1ABC\xB5"));
        body.extend_from_slice(&posting(100));
        body.extend_from_slice(&posting(200));
        body.push(b'x');

        offsets.push(body.len() as u32);
        body.extend_from_slice(&encode_header(body.len() as u64, b"K1ABC:2015\xB5"));
        body.extend_from_slice(&posting(100));
        body.extend_from_slice(&posting(300));
        body.push(b'x');

        offsets.push(body.len() as u32);
        body.extend_from_slice(&encode_header(body.len() as u64, b"DAVID\xB5"));
        body.extend_from_slice(&posting(400));
        body.push(b'x');

        let mut idx_bytes = Vec::new();
        for offset in offsets {
            idx_bytes.extend_from_slice(&offset.to_be_bytes());
        }
        let idx = write_file(&idx_bytes);
        let dat = write_file(&body);
        let hci = HciFile::open(idx.path(), dat.path()).unwrap();

        let stats = hci.callsign_posting_statistics();
        assert_eq!(stats.current_postings, 2);
        assert_eq!(stats.current_unique_dat_offsets, 2);
        assert_eq!(stats.archive_postings, 2);
        assert_eq!(stats.archive_unique_dat_offsets, 2);
        assert_eq!(stats.archive_years.len(), 1);
        assert_eq!(stats.archive_years[0].year, 2015);
        assert_eq!(stats.archive_years[0].unique_dat_offsets, 2);
    }
}
