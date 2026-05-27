//! Index file (`hamcall.idx`) parser and search — **2025 DVD format**.
//!
//! This is the supported 2025 `ham0/hamcall.idx` format.
//! [`crate::db`] auto-detects the layout.
//!
//! # On-disk model
//!
//! ```text
//! <key><padding spaces><dat_offset><single trailing space>\r\n
//! <key><padding spaces><dat_offset><single trailing space>\r\n
//! ...
//! ```
//!
//! - File is plain ASCII; every byte is in `{0x09, 0x0A, 0x0D, 0x20..=0x7E}`.
//! - `<key>` is the callsign, optionally followed by `:<YYYY>` license-year.
//! - `<dat_offset>` is an ASCII decimal byte offset into `hamcall.dat`.
//! - Production 2025 lines are 28 bytes including `\r\n`, except for the
//!   trailing `ZZZZZZZZ` sentinel. The parser still accepts variable line
//!   widths.
//! - Entries are sorted ascending by ASCII byte order on `<key>`.
//! - DAT offsets are strictly monotonically increasing.
//! - First entry is the sentinel `!!!`; last is `ZZZZZZZZ` with offset
//!   `file_size + 5`.
//!
//! # Search strategy
//!
//! On open we scan the file once to build a sparse byte-offset directory
//! at fixed line-count stride (every Nth line). Lookup binary-searches
//! the directory to land in a small chunk, then linearly walks the chunk.
//! All search primitives in this module funnel through the parsed-entry
//! binary search helpers.

use std::fs::File;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::format::v2;

/// One opened, mmapped 2025-format IDX file.
pub struct TextIdxFile {
    path: PathBuf,
    mmap: Mmap,
    entries: Vec<TextIndexEntry>,
    /// Byte offsets in this file at which every `STRIDE`-th line begins.
    /// `directory[i]` is the start of line `i * STRIDE`. Always starts
    /// with `0` and ends with the offset of the last line.
    directory: Vec<u32>,
    /// Total number of lines in the file.
    line_count: usize,
    /// Sample stride used when building the directory.
    stride: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextIndexEntry {
    key_start: u32,
    key_len: u16,
    line_start: u32,
    dat_offset: u64,
}

/// One parsed IDX entry, borrowed from the mmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEntry<'a> {
    /// Key bytes — callsign optionally followed by `:YYYY`. Borrowed from
    /// the mmap so no allocation.
    pub key: &'a [u8],
    /// Decimal offset into `hamcall.dat` where this record starts.
    pub dat_offset: u64,
}

/// Default sample stride for the sparse directory. Tuned so the directory
/// for the production-size IDX (~1.3 M entries) takes ~512 KB.
const DEFAULT_DIRECTORY_STRIDE: usize = 256;

impl TextIdxFile {
    /// Memory-map an IDX file and build the sparse directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_stride(path, DEFAULT_DIRECTORY_STRIDE)
    }

    /// Memory-map an IDX file with a custom directory stride. Smaller
    /// stride → more memory, faster lookup. The default is fine for
    /// nearly all uses.
    pub fn open_with_stride(path: impl AsRef<Path>, stride: usize) -> Result<Self> {
        let path = path.as_ref().to_owned();
        if stride < 1 {
            return Err(Error::MalformedHeader {
                path,
                reason: "stride must be >= 1",
            });
        }
        let file = File::open(&path)?;
        // SAFETY: read-only view of a stable on-disk database.
        let mmap = unsafe { Mmap::map(&file)? };
        let (directory, line_count, entries) = build_index(&mmap, stride, &path)?;
        Ok(Self {
            path,
            mmap,
            entries,
            directory,
            line_count,
            stride,
        })
    }

    /// File path (for diagnostics).
    #[inline]
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Total number of lines (entries) in the file.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.line_count
    }

    /// Whether the IDX is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.line_count == 0
    }

    /// Look up the first entry whose key equals `key` (exact byte match).
    ///
    /// `key` should be the full IDX key — i.e. callsign optionally with
    /// `:YYYY` suffix. To find any year for a callsign, use
    /// [`Self::find_callsign`].
    #[must_use]
    pub fn find_exact(&self, key: &[u8]) -> Option<TextEntry<'_>> {
        let index = self.find_entry_index_ge(key)?;
        let entry = self.text_entry_at(index)?;
        (entry.key == key).then_some(entry)
    }

    fn find_entry_index_ge(&self, target: &[u8]) -> Option<usize> {
        let index = self
            .entries
            .partition_point(|entry| self.entry_key(entry) < target);
        (index < self.entries.len()).then_some(index)
    }

    fn find_entry_index_gt(&self, target: &[u8]) -> Option<usize> {
        let index = self
            .entries
            .partition_point(|entry| self.entry_key(entry) <= target);
        (index < self.entries.len()).then_some(index)
    }

    fn text_entry_at(&self, index: usize) -> Option<TextEntry<'_>> {
        let entry = self.entries.get(index)?;
        Some(TextEntry {
            key: self.entry_key(entry),
            dat_offset: entry.dat_offset,
        })
    }

    fn entry_key(&self, entry: &TextIndexEntry) -> &[u8] {
        let start = entry.key_start as usize;
        let end = start + usize::from(entry.key_len);
        &self.mmap[start..end]
    }

    /// Look up the first entry whose key begins with `callsign` followed
    /// by either end-of-key or `:<year>`. Useful when the caller doesn't
    /// care about the year suffix.
    ///
    /// Note: a bare callsign (e.g. `b"AO"`) sorts BEFORE any
    /// `<callsign>:<year>` form (since shorter is smaller and `:` (0x3A)
    /// is greater than nothing). The first entry matching a bare-callsign
    /// query is therefore either the bare-callsign entry itself (if it
    /// exists) or the `<callsign>:<earliest-year>` entry.
    ///
    /// If multiple entries share a callsign (e.g. multiple license years
    /// for one callsign), this returns the lexicographically first one.
    /// Use [`Self::find_callsign_all`] for every match.
    #[must_use]
    pub fn find_callsign(&self, callsign: &[u8]) -> Option<TextEntry<'_>> {
        self.find_callsign_all(callsign).into_iter().next()
    }

    /// Every entry whose key's callsign part equals `callsign`.
    ///
    /// Internally a two-pass lookup:
    /// 1. Look for a bare `<callsign>` entry (one possible hit).
    /// 2. Look for the contiguous run of `<callsign>:<year>` entries
    ///    by binary-searching for the smallest key `>= "<callsign>:"`,
    ///    then walking forward while `key` still starts with that prefix.
    ///
    /// A bare callsign and its `:YYYY` forms are not contiguous in sort
    /// order. Between `AO` and `AO:2015` the production file contains
    /// `AO01DD`, `AO0IMD:2015`, `AO150E:2020`, `AO1B:2015`, … (any byte
    /// in `[0x2F, 0x3A)` — i.e. `/` and digits — sorts between the
    /// callsign's last letter and `:`). Same with `W1AW`: `W1AW/1:2020`,
    /// `W1AW/8:2015`, `W1AW/KP4:2015` all come between bare `W1AW` and
    /// `W1AW:2000`. The two-pass approach scans only actual hits.
    #[must_use]
    pub fn find_callsign_all(&self, callsign: &[u8]) -> Vec<TextEntry<'_>> {
        let mut out = Vec::new();

        // Pass 1: bare-callsign entry, if any.
        if let Some(index) = self.find_entry_index_ge(callsign) {
            if let Some(entry) = self.text_entry_at(index) {
                if entry.key == callsign {
                    out.push(entry);
                }
            }
        }

        // Pass 2: contiguous run of `<callsign>:<year>` entries.
        let mut suffix_prefix = Vec::with_capacity(callsign.len() + 1);
        suffix_prefix.extend_from_slice(callsign);
        suffix_prefix.push(v2::IDX_KEY_YEAR_SEP);
        if let Some(mut index) = self.find_entry_index_ge(&suffix_prefix) {
            while index < self.entries.len() {
                let Some(entry) = self.text_entry_at(index) else {
                    break;
                };
                if !entry.key.starts_with(&suffix_prefix) {
                    break;
                }
                out.push(entry);
                index += 1;
            }
        }

        out
    }

    /// Find the first entry whose key sorts strictly after `key`. Returns
    /// `None` if `key` is greater than or equal to every entry in the file.
    ///
    /// Used by `db::Diagnostics::lookup_v2_raw` to compute the size of a DAT
    /// record (start of next record minus start of this one).
    #[must_use]
    pub fn next_entry_after_key(&self, key: &[u8]) -> Option<TextEntry<'_>> {
        let index = self.find_entry_index_gt(key)?;
        self.text_entry_at(index)
    }

    /// Iterate every entry in the file in sorted order. Performs a linear
    /// scan of the mmap; useful for verification.
    pub fn iter(&self) -> TextIdxIter<'_> {
        TextIdxIter {
            buf: &self.mmap[..],
            pos: 0,
        }
    }

    /// Find the byte position of the first IDX entry whose key compares
    /// `>= target`, or `None` if no such entry exists.
    ///
    /// The parsed entry table is binary-searched directly.
    pub fn find_first_key_at_least(&self, target: &[u8]) -> Option<usize> {
        let index = self.find_entry_index_ge(target)?;
        self.entries
            .get(index)
            .map(|entry| entry.line_start as usize)
    }

    /// Sample stride used for the directory (for diagnostics).
    #[inline]
    #[must_use]
    pub fn directory_stride(&self) -> usize {
        self.stride
    }

    /// Number of directory anchors (for diagnostics).
    #[inline]
    #[must_use]
    pub fn directory_size(&self) -> usize {
        self.directory.len()
    }
}

/// Iterator over every entry in a [`TextIdxFile`], in sorted order.
pub struct TextIdxIter<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for TextIdxIter<'a> {
    type Item = TextEntry<'a>;

    fn next(&mut self) -> Option<TextEntry<'a>> {
        while self.pos < self.buf.len() {
            let rest = &self.buf[self.pos..];
            let line_end = rest.windows(2).position(|w| w == b"\r\n")?;
            let line = &rest[..line_end];
            self.pos += line_end + 2;
            if let Some(entry) = parse_line(line) {
                return Some(entry);
            }
        }
        None
    }
}

/// Parse a single line (no `\r\n`) into a [`TextEntry`].
fn parse_line(line: &[u8]) -> Option<TextEntry<'_>> {
    // Trim trailing spaces only (line shouldn't contain other whitespace).
    let trimmed = trim_ascii_trailing_spaces(line);
    // Find the last run of digits — that's `dat_offset`. Walk back
    // through trailing digits, then require at least one space before.
    let mut i = trimmed.len();
    let digit_end = i;
    while i > 0 && trimmed[i - 1].is_ascii_digit() {
        i -= 1;
    }
    let digit_start = i;
    if digit_start == digit_end {
        return None;
    }
    if i == 0 {
        // Whole line is digits — not a real entry, but tolerate.
        return None;
    }
    if trimmed[i - 1] != b' ' {
        return None;
    }
    // Walk past separating spaces back to the end of the key.
    while i > 0 && trimmed[i - 1] == b' ' {
        i -= 1;
    }
    let key_end = i;
    if key_end == 0 {
        return None;
    }
    let key = &trimmed[..key_end];
    // SAFETY: digits only, ASCII.
    let dat_offset: u64 = std::str::from_utf8(&trimmed[digit_start..digit_end])
        .ok()?
        .parse()
        .ok()?;
    Some(TextEntry { key, dat_offset })
}

/// Drop trailing 0x20 spaces from `line`.
fn trim_ascii_trailing_spaces(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && line[end - 1] == b' ' {
        end -= 1;
    }
    &line[..end]
}

/// Single linear pass over the IDX to build the sparse directory.
fn build_index(
    mmap: &Mmap,
    stride: usize,
    path: &Path,
) -> Result<(Vec<u32>, usize, Vec<TextIndexEntry>)> {
    let mut directory = Vec::new();
    let mut entries = Vec::new();
    let mut line_count = 0usize;
    let mut pos = 0usize;
    let cap = mmap.len();

    while pos < cap {
        if line_count % stride == 0 {
            // Production IDX files are well under 4 GiB.
            let off: u32 = pos.try_into().map_err(|_| Error::MalformedHeader {
                path: path.to_owned(),
                reason: "IDX exceeds 4 GiB; u32 directory cannot index it",
            })?;
            directory.push(off);
        }
        let rest = &mmap[pos..];
        let line_end = match rest.windows(2).position(|w| w == b"\r\n") {
            Some(p) => p,
            None => break,
        };
        if let Some(entry) = parse_index_line(pos, &rest[..line_end]) {
            entries.push(entry);
        }
        pos += line_end + 2;
        line_count += 1;
    }

    if line_count == 0 {
        return Err(Error::MalformedHeader {
            path: path.to_owned(),
            reason: "IDX appears empty or malformed",
        });
    }

    Ok((directory, line_count, entries))
}

fn parse_index_line(line_start: usize, line: &[u8]) -> Option<TextIndexEntry> {
    let trimmed = trim_ascii_trailing_spaces(line);
    let mut i = trimmed.len();
    let digit_end = i;
    while i > 0 && trimmed[i - 1].is_ascii_digit() {
        i -= 1;
    }
    let digit_start = i;
    if digit_start == digit_end || i == 0 || trimmed[i - 1] != b' ' {
        return None;
    }
    while i > 0 && trimmed[i - 1] == b' ' {
        i -= 1;
    }
    let key_end = i;
    if key_end == 0 {
        return None;
    }
    let dat_offset = parse_decimal_u64(&trimmed[digit_start..digit_end])?;
    Some(TextIndexEntry {
        key_start: line_start.try_into().ok()?,
        key_len: key_end.try_into().ok()?,
        line_start: line_start.try_into().ok()?,
        dat_offset,
    })
}

fn parse_decimal_u64(bytes: &[u8]) -> Option<u64> {
    let mut out = 0u64;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
    }
    Some(out)
}

/// Verification result for a [`TextIdxFile`].
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// Number of entries scanned.
    pub entries: usize,
    /// Bytes outside the printable ASCII + `\r\n\t` set.
    pub non_ascii_bytes: usize,
    /// Lines that failed to parse as `<key> <decimal offset>`.
    pub parse_failures: usize,
    /// Pairs `(i, key[i-1], key[i])` where keys went backwards.
    pub key_order_violations: usize,
    /// Pairs where the DAT offset did not strictly increase.
    pub offset_monotonicity_violations: usize,
    /// First and last keys observed.
    pub first_key: Option<Vec<u8>>,
    /// Last key observed.
    pub last_key: Option<Vec<u8>>,
    /// First and last DAT offsets observed.
    pub first_offset: Option<u64>,
    /// Last DAT offset.
    pub last_offset: Option<u64>,
}

impl VerifyReport {
    /// All invariants held.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.non_ascii_bytes == 0
            && self.parse_failures == 0
            && self.key_order_violations == 0
            && self.offset_monotonicity_violations == 0
            && self.entries > 0
    }
}

/// Walk every entry of the IDX once and check the §9.4 invariants.
///
/// Note: this is O(filesize). On the production 36 MB IDX it takes well
/// under a second on a current machine. Run it from
/// a dedicated `callbook verify` subcommand — not on every open.
#[must_use]
pub fn verify(idx: &TextIdxFile) -> VerifyReport {
    let mut rep = VerifyReport::default();
    let buf = &idx.mmap[..];

    // Cheap byte-set check. We allow tab in case any entry uses one
    // (none observed in the production file, but the cost is trivial).
    for &b in buf {
        let ok = b == b'\r' || b == b'\n' || b == b'\t' || (0x20..=0x7E).contains(&b);
        if !ok {
            rep.non_ascii_bytes += 1;
        }
    }

    let mut prev_key: Option<Vec<u8>> = None;
    let mut prev_off: Option<u64> = None;
    for entry in idx.iter() {
        rep.entries += 1;
        if rep.first_key.is_none() {
            rep.first_key = Some(entry.key.to_vec());
            rep.first_offset = Some(entry.dat_offset);
        }
        if let Some(prev) = &prev_key {
            if entry.key < prev.as_slice() {
                rep.key_order_violations += 1;
            }
        }
        if let Some(po) = prev_off {
            if entry.dat_offset <= po {
                rep.offset_monotonicity_violations += 1;
            }
        }
        prev_key = Some(entry.key.to_vec());
        prev_off = Some(entry.dat_offset);
    }
    rep.last_key = prev_key;
    rep.last_offset = prev_off;

    // Parse failures = lines walked by the directory scan minus parsed entries.
    if idx.line_count > rep.entries {
        rep.parse_failures = idx.line_count - rep.entries;
    }

    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_idx(content: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content).unwrap();
        f
    }

    /// The exact byte pattern from the production IDX header.
    const PRODUCTION_PROLOGUE: &[u8] =
        b"!!!                     6 \r\n0T805P:2015          1542 \r\n109HA857:2015        3358 \r\n";

    #[test]
    fn parses_proven_prologue() {
        let f = write_idx(PRODUCTION_PROLOGUE);
        let idx = TextIdxFile::open(f.path()).unwrap();
        assert_eq!(idx.len(), 3);
        let entries: Vec<_> = idx.iter().collect();
        assert_eq!(entries[0].key, b"!!!");
        assert_eq!(entries[0].dat_offset, 6);
        assert_eq!(entries[1].key, b"0T805P:2015");
        assert_eq!(entries[1].dat_offset, 1542);
        assert_eq!(entries[2].key, b"109HA857:2015");
        assert_eq!(entries[2].dat_offset, 3358);
    }

    #[test]
    fn finds_exact_and_callsign_part() {
        let body = b"AA1A:2010              100 \r\n\
                     K1ABC:2015             200 \r\n\
                     K1ABC:2020             300 \r\n\
                     W1AW:2000              400 \r\n\
                     W1AWK                  500 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open(f.path()).unwrap();

        // Exact key match.
        assert_eq!(idx.find_exact(b"K1ABC:2020").unwrap().dat_offset, 300);
        assert_eq!(idx.find_exact(b"W1AWK").unwrap().dat_offset, 500);
        assert!(idx.find_exact(b"NOPE").is_none());

        // Callsign-prefix match returns first hit.
        assert_eq!(idx.find_callsign(b"K1ABC").unwrap().dat_offset, 200);
        assert_eq!(idx.find_callsign(b"W1AW").unwrap().dat_offset, 400);
        // W1AWK does not match `W1AW` because the callsign part is
        // "W1AWK", not "W1AW".
        let all_w1aw = idx.find_callsign_all(b"W1AW");
        assert_eq!(all_w1aw.len(), 1);
        assert_eq!(all_w1aw[0].key, b"W1AW:2000");
    }

    #[test]
    fn verify_passes_on_well_formed_idx() {
        let body = b"AAA              10 \r\nBBB              20 \r\nCCC              30 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open(f.path()).unwrap();
        let rep = verify(&idx);
        assert!(rep.is_clean(), "{rep:?}");
        assert_eq!(rep.entries, 3);
        assert_eq!(rep.first_key.as_deref(), Some(b"AAA".as_ref()));
        assert_eq!(rep.last_key.as_deref(), Some(b"CCC".as_ref()));
        assert_eq!(rep.first_offset, Some(10));
        assert_eq!(rep.last_offset, Some(30));
    }

    #[test]
    fn verify_flags_non_monotonic_offsets() {
        let body = b"AAA              30 \r\nBBB              20 \r\nCCC              10 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open(f.path()).unwrap();
        let rep = verify(&idx);
        assert!(!rep.is_clean());
        assert_eq!(rep.offset_monotonicity_violations, 2);
    }

    #[test]
    fn verify_flags_unsorted_keys() {
        let body = b"BBB              10 \r\nAAA              20 \r\nCCC              30 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open(f.path()).unwrap();
        let rep = verify(&idx);
        assert!(!rep.is_clean());
        assert_eq!(rep.key_order_violations, 1);
    }

    #[test]
    fn verify_flags_non_ascii() {
        let mut body: Vec<u8> = b"AAA              10 \r\n".to_vec();
        body.extend_from_slice(&[0xFF, 0xFE]);
        body.extend_from_slice(b"BBB              20 \r\n");
        let f = write_idx(&body);
        let idx = TextIdxFile::open(f.path()).unwrap();
        let rep = verify(&idx);
        assert!(rep.non_ascii_bytes >= 2);
    }

    #[test]
    fn variable_length_lines() {
        // Mix of 4-digit and 10-digit offsets, mix of with/without
        // year suffix — exactly what the production file does.
        let body = b"AA1A                     6 \r\n\
                     ZZZZZZ:2000     2036252049 \r\n\
                     ZZZZZZZZ        2036252293 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open(f.path()).unwrap();
        let entries: Vec<_> = idx.iter().collect();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].key, b"ZZZZZZZZ");
        assert_eq!(entries[2].dat_offset, 2_036_252_293);
    }

    #[test]
    fn find_callsign_skips_over_intervening_entries() {
        // In the production IDX, digit-suffixed callsigns interleave between
        // bare `AO` and `AO:YYYY` entries.
        //
        // Same shape for `W1AW`: production has `W1AW/1:2020`,
        // `W1AW/8:2015`, `W1AW/KP4:2015` between bare `W1AW` and
        // `W1AW:2000`.
        let mut body = Vec::new();
        // Pad with > 256 entries to cross at least one directory anchor.
        for i in 0..280u32 {
            let key = format!("AN{i:04}");
            let line = format!("{key:<22}{:>3} ", i);
            body.extend_from_slice(line.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        // Mimic the production "AO" neighborhood: digit-suffixed
        // callsigns interleave between bare `AO` and `AO:YYYY`.
        body.extend_from_slice(b"AO01DD              290 \r\n");
        body.extend_from_slice(b"AO0IMD:2015         291 \r\n");
        body.extend_from_slice(b"AO150E:2020         292 \r\n");
        body.extend_from_slice(b"AO1B:2010           293 \r\n");
        body.extend_from_slice(b"AO9X:2020           294 \r\n");
        // The actual targets:
        body.extend_from_slice(b"AO:1980             295 \r\n");
        body.extend_from_slice(b"AO:2015             296 \r\n");
        // And some unrelated alphabetic-suffixed entries past it.
        body.extend_from_slice(b"AOAB:2010           500 \r\n");
        body.extend_from_slice(b"AOZZ:1999           501 \r\n");

        let f = write_idx(&body);
        let idx = TextIdxFile::open(f.path()).unwrap();

        let hit = idx.find_callsign(b"AO").expect("AO should match AO:1980");
        assert_eq!(hit.key, b"AO:1980");
        let all = idx.find_callsign_all(b"AO");
        assert_eq!(
            all.iter().map(|e| e.key).collect::<Vec<_>>(),
            vec![b"AO:1980".as_ref(), b"AO:2015".as_ref()],
        );

        // The digit-suffixed entries are their own callsigns and
        // should still be discoverable as such.
        let aoidd = idx.find_callsign(b"AO01DD").unwrap();
        assert_eq!(aoidd.key, b"AO01DD");
        let ao0imd = idx.find_callsign_all(b"AO0IMD");
        assert_eq!(ao0imd.len(), 1);
        assert_eq!(ao0imd[0].key, b"AO0IMD:2015");

        // AOAB matches itself.
        let aoab = idx.find_callsign(b"AOAB").unwrap();
        assert_eq!(aoab.key, b"AOAB:2010");

        // Non-existent: returns None.
        assert!(idx.find_callsign(b"AOAC").is_none());
    }

    #[test]
    fn find_callsign_with_slash_suffix_neighborhood() {
        // The W1AW production pattern: `W1AW/1:2020`, `W1AW/8:2015`,
        // `W1AW/KP4:2015` sort between bare `W1AW` and `W1AW:2000`.
        let body = b"\
W1AVW:1995            10 \r\n\
W1AVY:1990            11 \r\n\
W1AW/1:2020           12 \r\n\
W1AW/8:2015           13 \r\n\
W1AW/KP4:2015         14 \r\n\
W1AW:2000             15 \r\n\
W1AWA:1957            16 \r\n\
W1AWB:1940            17 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open(f.path()).unwrap();

        let w1aw = idx
            .find_callsign(b"W1AW")
            .expect("W1AW should match W1AW:2000");
        assert_eq!(w1aw.key, b"W1AW:2000");
        assert_eq!(w1aw.dat_offset, 15);

        let all = idx.find_callsign_all(b"W1AW");
        // Only "W1AW:2000" — the `W1AW/X` entries are different
        // callsigns, and `W1AWA`/`W1AWB` are also different callsigns.
        assert_eq!(
            all.iter().map(|e| e.key).collect::<Vec<_>>(),
            vec![b"W1AW:2000".as_ref()]
        );

        // The slash-suffix forms are their own callsigns.
        let w1aw_slash_1 = idx.find_callsign(b"W1AW/1").unwrap();
        assert_eq!(w1aw_slash_1.key, b"W1AW/1:2020");
    }

    #[test]
    fn next_entry_after_key_walks_only_forward_locally() {
        // Cross a directory stride boundary.
        let mut body = Vec::new();
        for i in 0..600u32 {
            let key = format!("K{i:05}");
            let line = format!("{key:<20}{:>5} ", i + 1000);
            body.extend_from_slice(line.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        let f = write_idx(&body);
        let idx = TextIdxFile::open(f.path()).unwrap();

        let n = idx.next_entry_after_key(b"K00100").unwrap();
        assert_eq!(n.key, b"K00101");
        let n = idx.next_entry_after_key(b"K00500").unwrap();
        assert_eq!(n.key, b"K00501");
        // Last key has no successor.
        assert!(idx.next_entry_after_key(b"K00599").is_none());
    }

    #[test]
    fn directory_stride_one_works() {
        let body =
            b"A              1 \r\nB              2 \r\nC              3 \r\nD              4 \r\n";
        let f = write_idx(body);
        let idx = TextIdxFile::open_with_stride(f.path(), 1).unwrap();
        assert_eq!(idx.find_exact(b"C").unwrap().dat_offset, 3);
        assert_eq!(idx.directory_size(), idx.len());
    }
}
