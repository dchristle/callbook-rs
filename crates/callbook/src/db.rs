//! Top-level database orchestrator.
//!
//! A [`CallBook`] owns the memory maps and indexes needed for repeated
//! read-only callsign lookups.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::callsign::Callsign;
use crate::country::{CountryInfo, CountryMatch, CountryTable};
use crate::error::{Error, Result};
use crate::format::v2;
use crate::hci::{CallsignPostingStatistics, DecodedHciRecord, HciFile, HciPosting, RawHciRecord};
use crate::idx_text::{TextEntry, TextIdxFile};
use crate::interest::{InterestDefinition, InterestTable};
use crate::modern::{CallSnapshot, Jurisdiction, LookupResult, LookupStatus, SnapshotSource};
use crate::sidecar_impl::{
    BoundaryDataset, BoundaryKind, BoundarySegment, CountryNameTable, GeoPoint, LookupCountRecord,
    LookupCounts, PackedStateMap, PhotoCatalog, StateVectorDataset, UsCountyBoundaryDataset,
};
use crate::us_csv::{UsCsvFile, UsCsvRecord};
use crate::v2_dat::DecodedV2Candidate;

use memmap2::Mmap;

/// Default byte count used for diagnostic hex/text previews.
pub const DEFAULT_TRACE_PREVIEW_LIMIT: usize = 160;

/// One opened database. Read-only. `Sync`/`Send`.
pub struct CallBook {
    /// 2025-format shard (`ham0/hamcall.{idx,dat}` plus sidecars).
    v2: Option<V2Shard>,
    lookup_counts: Mutex<Option<Arc<LookupCounts>>>,
    photo_catalog: Mutex<Option<Arc<PhotoCatalog>>>,
    world_boundaries: Mutex<Option<Arc<BoundaryDataset>>>,
    county_boundaries: Mutex<Option<Arc<BoundaryDataset>>>,
    us_county_boundaries: Mutex<Option<Arc<UsCountyBoundaryDataset>>>,
    state_vectors: Mutex<Option<Arc<StateVectorDataset>>>,
}

/// 2025-format database state.
struct V2Shard {
    dir: PathBuf,
    idx: TextIdxFile,
    dat_path: PathBuf,
    dat_mmap: Arc<Mmap>,
    hci: Option<HciFile>,
    us_csv: Option<UsCsvFile>,
    countries: Option<CountryTable>,
    pc_countries: Option<CountryTable>,
    country_names: Option<CountryNameTable>,
    interests: Option<InterestTable>,
}

/// Builder for opening a [`CallBook`] database.
#[derive(Debug, Clone)]
pub struct CallBookBuilder {
    data_path: PathBuf,
}

/// A callsign lookup plus access to related database sidecars.
pub struct CallsignEntry<'db> {
    db: &'db CallBook,
    report: LookupResult,
}

/// Advanced inspection APIs for on-disk HamCall formats.
#[derive(Clone, Copy)]
pub struct Diagnostics<'db> {
    db: &'db CallBook,
}

/// Reusable lookup context for high-throughput callers.
pub struct BatchLookup<'db> {
    db: &'db CallBook,
    scratch: LookupScratch,
}

/// Builder for iterating domain entries.
#[derive(Clone, Copy)]
pub struct Entries<'db> {
    db: &'db CallBook,
    include_history: bool,
}

/// Iterator over selected domain entries.
pub struct EntryIter<'db> {
    db: &'db CallBook,
    phase: EntryIterPhase,
    include_history: bool,
    seen: BTreeSet<String>,
    modern_idx: Option<crate::idx_text::TextIdxIter<'db>>,
    us_csv_callsigns: Option<Box<dyn Iterator<Item = &'db str> + 'db>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryIterPhase {
    ModernIdx,
    UsCsv,
    Done,
}

#[derive(Default)]
struct LookupScratch {
    hci_decoded: Vec<u8>,
    hci_seen_offsets: BTreeSet<u64>,
    hci_snapshots: Vec<CallSnapshot>,
}

impl CallBookBuilder {
    /// Open the database.
    pub fn open(self) -> Result<CallBook> {
        CallBook::open_path(self.data_path)
    }
}

impl<'db> CallsignEntry<'db> {
    /// Normalized query callsign.
    #[must_use]
    pub fn callsign(&self) -> &str {
        &self.report.query
    }

    /// Lookup status after all user-facing sources are consulted.
    #[must_use]
    pub fn status(&self) -> LookupStatus {
        self.report.status
    }

    /// Best current snapshot, if one is available.
    #[must_use]
    pub fn current(&self) -> Option<&CallSnapshot> {
        self.report.current.as_ref()
    }

    /// Historical snapshots sorted by vintage.
    #[must_use]
    pub fn history(&self) -> &[CallSnapshot] {
        &self.report.history
    }

    /// Matched country metadata for this callsign.
    #[must_use]
    pub fn country(&self) -> Option<CountryMatch> {
        self.db.country(self.callsign())
    }

    /// Assets associated with this callsign and its matched country.
    #[must_use]
    pub fn assets(&self) -> Vec<AssetMetadata> {
        self.db.assets(self.callsign())
    }

    /// HamCall.net web lookup count for this callsign.
    pub fn lookup_count(&self) -> Result<Option<LookupCountRecord>> {
        self.db.lookup_count(self.callsign())
    }

    /// Build a station profile with current data, country context, counts, and assets.
    pub fn profile(&self) -> Result<StationProfile> {
        self.db.profile_from_report(&self.report)
    }

    /// Build map context for this station.
    pub fn map(&self) -> Result<StationMap<'db>> {
        self.db.map_from_report(&self.report)
    }

    /// Convert this handle into its owned lookup report.
    #[must_use]
    pub fn into_report(self) -> LookupResult {
        self.report
    }
}

impl<'db> Diagnostics<'db> {
    /// Trace one lookup through IDX, HCI, DAT boundary scans, and parsing.
    pub fn trace_lookup(&self, callsign: &str) -> Result<LookupTrace> {
        self.db.trace_lookup(callsign)
    }

    /// Trace one lookup with a custom hex/text preview byte limit.
    pub fn trace_lookup_with_limit(&self, callsign: &str, limit: usize) -> Result<LookupTrace> {
        self.db.trace_lookup_with_limit(callsign, limit)
    }

    /// Return the raw encoded DAT record for a callsign.
    #[must_use]
    pub fn lookup_v2_raw(&self, callsign: &str) -> Option<RawV2Record<'db>> {
        self.db.lookup_v2_raw(callsign)
    }

    /// Whether this database opened a 2025-layout shard.
    #[must_use]
    pub fn has_v2(&self) -> bool {
        self.db.has_v2()
    }

    /// Number of entries in the 2025-format IDX.
    #[must_use]
    pub fn v2_idx_len(&self) -> usize {
        self.db.v2_idx_len()
    }

    /// Borrow the 2025-format IDX for advanced inspection.
    #[must_use]
    pub fn v2_idx(&self) -> Option<&TextIdxFile> {
        self.db.v2_idx()
    }

    /// Whether this database opened the 2025 HCI corpus.
    #[must_use]
    pub fn has_hci(&self) -> bool {
        self.db.has_hci()
    }

    /// Number of raw HCI records.
    #[must_use]
    pub fn hci_len(&self) -> usize {
        self.db.hci_len()
    }

    /// Return one encoded HCI record by ordinal.
    #[must_use]
    pub fn hci_raw_record(&self, index: usize) -> Option<RawHciRecord<'db>> {
        self.db.hci_raw_record(index)
    }

    /// Decode one HCI record by ordinal.
    #[must_use]
    pub fn hci_decoded_record(&self, index: usize) -> Option<DecodedHciRecord> {
        self.db.hci_decoded_record(index)
    }

    /// Aggregate source statistics.
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.db.stats()
    }

    /// Scan lookup keys and count current versus historical records.
    #[must_use]
    pub fn record_statistics(&self) -> RecordStatistics {
        self.db.record_statistics()
    }

    /// Return one callsign from the current-US sidecar.
    #[must_use]
    pub fn sample_us_current_callsign(&self) -> Option<String> {
        self.db.sample_us_current_callsign()
    }

    /// Scan lookup sources for interest-profile coverage.
    #[must_use]
    pub fn interest_statistics(&self) -> InterestStatistics {
        self.db.interest_statistics()
    }

    /// Scan the DAT file and count observed field tags.
    #[must_use]
    pub fn modern_tag_statistics(&self) -> ModernTagStatistics {
        self.db.modern_tag_statistics()
    }

    /// Check whether callsign-looking HCI postings point at DAT record starts.
    #[must_use]
    pub fn callsign_hci_posting_start_invariant(&self) -> Option<HciPostingStartInvariant> {
        self.db.callsign_hci_posting_start_invariant()
    }

    /// Run the format verifier and return a human-readable report.
    #[must_use]
    pub fn verify(&self) -> String {
        self.db.verify()
    }
}

impl<'db> BatchLookup<'db> {
    /// Look up one callsign using this reusable batch context.
    pub fn lookup(&mut self, callsign: &str) -> Result<CallsignEntry<'db>> {
        Ok(CallsignEntry {
            db: self.db,
            report: self
                .db
                .lookup_report_with_scratch(callsign, &mut self.scratch)?,
        })
    }
}

impl<'db> Entries<'db> {
    /// Include or suppress historical snapshots from lookups.
    #[must_use]
    pub fn include_history(mut self, include_history: bool) -> Self {
        self.include_history = include_history;
        self
    }

    /// Run a callback for each selected entry.
    pub fn run(self, mut visit: impl FnMut(CallsignEntry<'db>) -> Result<()>) -> Result<()> {
        for entry in self {
            visit(entry?)?;
        }
        Ok(())
    }
}

impl<'db> IntoIterator for Entries<'db> {
    type Item = Result<CallsignEntry<'db>>;
    type IntoIter = EntryIter<'db>;

    fn into_iter(self) -> Self::IntoIter {
        let v2 = self.db.v2.as_ref();
        let modern_idx = v2.map(|v2| v2.idx.iter());
        let us_csv_callsigns = v2.and_then(|v2| {
            v2.us_csv
                .as_ref()
                .map(|us_csv| Box::new(us_csv.callsigns()) as Box<dyn Iterator<Item = &str>>)
        });
        EntryIter {
            db: self.db,
            phase: EntryIterPhase::ModernIdx,
            include_history: self.include_history,
            seen: BTreeSet::new(),
            modern_idx,
            us_csv_callsigns,
        }
    }
}

impl<'db> Iterator for EntryIter<'db> {
    type Item = Result<CallsignEntry<'db>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let callsign = self.next_callsign()?;
            if !self.seen.insert(callsign.clone()) {
                continue;
            }
            let mut entry = match self.db.lookup(&callsign) {
                Ok(entry) => entry,
                Err(err) => return Some(Err(err)),
            };
            if !self.include_history {
                entry.report.history.clear();
                if entry.report.current.is_none() {
                    entry.report.status = LookupStatus::NotFound;
                    continue;
                }
            }
            return Some(Ok(entry));
        }
    }
}

impl<'db> EntryIter<'db> {
    fn next_callsign(&mut self) -> Option<String> {
        loop {
            match self.phase {
                EntryIterPhase::ModernIdx => {
                    let Some(iter) = &mut self.modern_idx else {
                        self.phase = EntryIterPhase::UsCsv;
                        continue;
                    };
                    for entry in iter.by_ref() {
                        let Some((callsign, vintage)) = stat_key_parts(entry.key) else {
                            continue;
                        };
                        if !self.include_history && vintage.is_some() {
                            continue;
                        }
                        return Some(callsign);
                    }
                    self.phase = EntryIterPhase::UsCsv;
                }
                EntryIterPhase::UsCsv => {
                    if let Some(iter) = &mut self.us_csv_callsigns {
                        if let Some(callsign) = iter.next() {
                            return Some(callsign.to_owned());
                        }
                    }
                    self.phase = EntryIterPhase::Done;
                }
                EntryIterPhase::Done => return None,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordBoundsTrace {
    start: usize,
    end: usize,
    source: RecordBoundarySource,
    backward_scan_bytes: Option<usize>,
    forward_scan_bytes: Option<usize>,
}

/// How a DAT record boundary was resolved for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordBoundarySource {
    /// Boundary came from treating an HCI posting offset as a DAT record start.
    HciPostingStart,
    /// Boundary came from the byte-scan fallback.
    ScanFallback,
}

impl std::fmt::Display for RecordBoundarySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HciPostingStart => "hci_posting_start",
            Self::ScanFallback => "scan_fallback",
        })
    }
}

/// Aggregated statistics across all opened database sources.
#[derive(Debug, Clone)]
pub struct Stats {
    /// Number of opened lookup sources.
    /// The `ham0` layout counts as one source when present.
    pub shard_count: usize,
    /// Number of IDX entries.
    pub total_records: usize,
    /// Number of entries in the 2025-format IDX.
    pub modern_idx_records: usize,
    /// Number of HCI offset-table entries in the 2025-format database.
    pub modern_hci_records: usize,
    /// Whether the current-US CSV source was opened.
    pub has_us_csv: bool,
}

/// Record counts from a full scan of database keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordStatistics {
    /// Unique current callsigns across current sources.
    pub current: JurisdictionCounts,
    /// Historical `CALL:YYYY` records in the IDX.
    pub archive: JurisdictionCounts,
    /// Current records contributed by bare keys in `hamcall.idx`.
    pub modern_idx_current_records: usize,
    /// Archive records contributed by `CALL:YYYY` keys in `hamcall.idx`.
    pub modern_idx_archive_records: usize,
    /// Current-US records contributed by `usa.csv.zip`.
    pub us_csv_current_records: usize,
    /// Current plus archive records after current-source de-duplication.
    pub total_records_including_archive: usize,
    /// Archive counts by publication year.
    pub archive_years: Vec<ArchiveYearStatistics>,
    /// Callsign-looking HCI posting counts.
    pub hci_callsigns: Option<CallsignPostingStatistics>,
}

/// Interest-profile statistics from decoded records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestStatistics {
    /// Number of entries loaded from `ham0/interest`.
    pub catalog_entries: usize,
    /// Unique current callsigns with one or more raw interest codes.
    pub current_callsigns_with_interests: usize,
    /// Current snapshots with one or more raw interest codes.
    pub current_snapshots_with_interests: usize,
    /// Archive snapshots with one or more raw interest codes.
    pub archive_snapshots_with_interests: usize,
    /// Unique current callsigns with one or more resolved interest labels.
    pub current_callsigns_with_resolved_interests: usize,
    /// Unknown interest-code occurrence counts.
    pub unknown_codes: Vec<UnknownInterestCode>,
}

/// Search results for one interest-profile code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestSearch {
    /// Four-digit interest code.
    pub code: String,
    /// Catalog definition for the code, when known.
    pub definition: Option<InterestDefinition>,
    /// Matching current and archive snapshots.
    pub matches: Vec<InterestSearchMatch>,
}

impl InterestSearch {
    /// Number of matching snapshots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Whether no snapshots matched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Iterate current-record matches.
    pub fn current(&self) -> impl Iterator<Item = &InterestSearchMatch> {
        self.matches.iter().filter(|entry| entry.vintage.is_none())
    }

    /// Iterate archive-record matches.
    pub fn archive(&self) -> impl Iterator<Item = &InterestSearchMatch> {
        self.matches.iter().filter(|entry| entry.vintage.is_some())
    }
}

/// One callsign snapshot containing an interest-profile code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestSearchMatch {
    /// Callsign without `:YYYY`.
    pub callsign: String,
    /// Archive publication year, or `None` for current records.
    pub vintage: Option<u16>,
}

/// One unknown interest-code occurrence count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownInterestCode {
    /// Four-digit interest code.
    pub code: String,
    /// Number of snapshots in which this code appears.
    pub occurrences: usize,
    /// Occurrences in current snapshots.
    pub current_occurrences: usize,
    /// Occurrences in archive snapshots.
    pub archive_occurrences: usize,
    /// Example records containing this unresolved code.
    pub examples: Vec<InterestCodeExample>,
}

/// One example record containing an unresolved interest code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestCodeExample {
    /// Callsign without `:YYYY`.
    pub callsign: String,
    /// Archive publication year, or `None` for current records.
    pub vintage: Option<u16>,
}

/// Modern DAT tag inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModernTagStatistics {
    /// Per-tag counts and samples.
    pub tags: Vec<ModernTagCount>,
}

/// Count and sample values for one DAT tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModernTagCount {
    /// Raw tag byte.
    pub tag: u8,
    /// Stable field name when the tag is mapped.
    pub field_name: Option<&'static str>,
    /// Total occurrence count.
    pub occurrences: usize,
    /// Current-record occurrence count.
    pub current_occurrences: usize,
    /// Archive-record occurrence count.
    pub archive_occurrences: usize,
    /// Up to three observed values.
    pub sample_values: Vec<String>,
}

/// Invariant check for callsign-looking HCI postings into `hamcall.dat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HciPostingStartInvariant {
    /// Total postings under callsign-looking HCI keys.
    pub total_postings: usize,
    /// Postings whose `hamcall.dat` target decodes to the record-start sentinel.
    pub record_start_postings: usize,
    /// Postings whose target is in range but does not decode to a record start.
    pub non_record_start_postings: usize,
    /// Postings whose target offset is outside `hamcall.dat`.
    pub out_of_bounds_postings: usize,
    /// First observed invariant failures, if any.
    pub samples: Vec<HciPostingStartInvariantSample>,
}

/// One callsign-looking HCI posting that does not point at a DAT record start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HciPostingStartInvariantSample {
    /// Decoded HCI search key.
    pub hci_key: String,
    /// Posting target offset into `hamcall.dat`.
    pub dat_offset: u64,
    /// Extra posting position byte.
    pub position: u8,
    /// Decoded target byte, or `None` when `dat_offset` is out of bounds.
    pub decoded_byte: Option<u8>,
}

/// Structured diagnostics for one callsign lookup.
///
/// This is a format-inspection API for the undocumented 2025 `ham0` layout.
/// Normal [`CallBook::lookup`] does not build these diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupTrace {
    /// Original query string.
    pub query: String,
    /// Parsed and normalized callsign.
    pub normalized_callsign: String,
    /// Whether `usa.csv.zip` returned a current-record hit.
    pub us_csv_hit: bool,
    /// Matching `hamcall.idx` entries and decoded DAT previews.
    pub idx_hits: Vec<IdxTrace>,
    /// HCI search keys and postings inspected for the lookup.
    pub hci_keys: Vec<HciKeyTrace>,
    /// Lookup status produced by the normal lookup API.
    pub final_status: LookupStatus,
}

/// Diagnostics for one `hamcall.idx` hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdxTrace {
    /// IDX key text.
    pub key: String,
    /// Byte offset into `hamcall.dat`.
    pub dat_offset: u64,
    /// Byte offset of the next IDX entry, when one exists.
    pub next_dat_offset: Option<u64>,
    /// Encoded DAT slice length.
    pub raw_len: usize,
    /// Prefix of decoded bytes as lowercase hexadecimal.
    pub decoded_hex_prefix: String,
    /// Prefix of decoded bytes as printable text.
    pub decoded_text_prefix: String,
    /// Parser outcome for this decoded slice.
    pub parsed_snapshots: ParsedSnapshotTrace,
}

/// Diagnostics for one HCI key searched during lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HciKeyTrace {
    /// Exact decoded HCI key searched.
    pub searched_key: String,
    /// Matching HCI records.
    pub hci_entries: Vec<HciEntryTrace>,
}

/// Diagnostics for one HCI record whose decoded header matched a search key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HciEntryTrace {
    /// Zero-based ordinal in `hciindex.dat`.
    pub ordinal: usize,
    /// Byte offset into `hci.dat`.
    pub hci_dat_offset: u64,
    /// Encoded HCI record length.
    pub raw_len: usize,
    /// Decoded header length including the decoded `0xb5` terminator.
    pub header_len: usize,
    /// Decoded header text without the in-band terminator.
    pub decoded_header: String,
    /// Prefix of encoded HCI record bytes as lowercase hexadecimal.
    pub encoded_hex_prefix: String,
    /// Prefix of decoded HCI header bytes as lowercase hexadecimal.
    pub decoded_hex_prefix: String,
    /// Decoded postings from this HCI record.
    pub postings: Vec<HciPostingTrace>,
}

/// Diagnostics for one 5-byte HCI posting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HciPostingTrace {
    /// Big-endian 4-byte `hamcall.dat` offset from the posting.
    pub dat_offset: u64,
    /// One-byte posting position payload.
    pub position: u8,
    /// Boundary resolver that found this posting's containing DAT record.
    pub record_boundary_source: Option<RecordBoundarySource>,
    /// Decoded `0xb5` record-start sentinel offset, when found.
    pub record_start: Option<u64>,
    /// Offset of the next decoded `0xb5` sentinel, when found.
    pub record_end: Option<u64>,
    /// Decoded record byte length from start sentinel to end sentinel.
    pub record_len: Option<usize>,
    /// Distance from the posting offset back to the containing record start.
    pub distance_to_record_start: Option<usize>,
    /// Distance from the posting offset forward to the containing record end.
    pub distance_to_record_end: Option<usize>,
    /// Bytes scanned backward from posting offset to record-start sentinel.
    pub backward_scan_bytes: Option<usize>,
    /// Bytes scanned forward from posting offset to record-end sentinel.
    pub forward_scan_bytes: Option<usize>,
    /// Whether `dat_offset` equals `record_start`.
    pub posting_matches_record_start: Option<bool>,
    /// Prefix of decoded record bytes as lowercase hexadecimal.
    pub decoded_record_hex_prefix: String,
    /// Prefix of decoded record bytes as printable text.
    pub decoded_record_text_prefix: String,
    /// Parser outcome for this posting.
    pub parsed_snapshots: ParsedSnapshotTrace,
}

/// Summary of decoded records parsed during diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSnapshotTrace {
    /// Number of parsed snapshots matching the requested callsign.
    pub count: usize,
    /// Callsigns parsed from matching snapshots.
    pub callsigns: Vec<String>,
    /// Snapshot vintages, or `None` for current records.
    pub vintages: Vec<Option<u16>>,
}

/// Lightweight asset metadata from non-core HamCall sidecar files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetMetadata {
    /// Asset kind.
    pub kind: AssetKind,
    /// Callsign or country code used to match this asset.
    pub key: String,
    /// Media/content type inferred from path.
    pub media_type: String,
    /// On-disk path.
    pub path: PathBuf,
}

/// File-backed asset reference for workflow APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    /// Asset kind.
    pub kind: AssetKind,
    /// Callsign or country code used to match this asset.
    pub key: String,
    /// Media/content type inferred from path.
    pub media_type: String,
    /// On-disk path.
    pub path: PathBuf,
}

/// Text asset reference that can read its content on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAssetRef {
    /// Asset kind.
    pub kind: AssetKind,
    /// Callsign key used to match this asset.
    pub key: String,
    /// Media/content type inferred from path.
    pub media_type: String,
    /// On-disk path.
    pub path: PathBuf,
}

impl TextAssetRef {
    /// Read the text asset as UTF-8.
    pub fn read_text(&self) -> Result<String> {
        Ok(fs::read_to_string(&self.path)?)
    }
}

/// Profile asset collection with convenience selectors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileAssets {
    photos: Vec<AssetRef>,
    bio: Option<TextAssetRef>,
    country_flag: Option<AssetRef>,
    country_map: Option<AssetRef>,
}

impl ProfileAssets {
    /// Stable primary photo when one is available.
    #[must_use]
    pub fn primary_photo(&self) -> Option<&AssetRef> {
        self.photos.first()
    }

    /// Callsign photo assets.
    #[must_use]
    pub fn photos(&self) -> &[AssetRef] {
        &self.photos
    }

    /// Biography text asset.
    #[must_use]
    pub fn bio(&self) -> Option<&TextAssetRef> {
        self.bio.as_ref()
    }

    /// Read biography text when a bio asset exists.
    pub fn bio_text(&self) -> Result<Option<String>> {
        self.bio.as_ref().map(TextAssetRef::read_text).transpose()
    }

    /// Country flag image.
    #[must_use]
    pub fn country_flag(&self) -> Option<&AssetRef> {
        self.country_flag.as_ref()
    }

    /// Country map image.
    #[must_use]
    pub fn country_map(&self) -> Option<&AssetRef> {
        self.country_map.as_ref()
    }
}

/// Unified country lookup model spanning prefix, fallback, and name sidecars.
#[derive(Debug, Clone, Copy)]
pub struct CountryCatalog<'db> {
    primary: Option<&'db CountryTable>,
    fallback: Option<&'db CountryTable>,
    names: Option<&'db CountryNameTable>,
}

/// Country catalog coverage summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CountryCatalogStatistics {
    /// Rules in the primary `countrys`/`gcmcountrys` table.
    pub primary_rules: usize,
    /// Rules in the `COUNTRYS.PC` fallback table.
    pub fallback_rules: usize,
    /// Centroids in `countrys.nam`.
    pub name_centroids: usize,
    /// Distinct raw country labels across prefix tables.
    pub raw_labels: usize,
    /// Distinct cleaned country labels across prefix tables.
    pub cleaned_labels: usize,
}

impl CountryCatalog<'_> {
    /// Whether no country sidecars were loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primary.is_none() && self.fallback.is_none() && self.names.is_none()
    }

    /// Primary country-prefix table.
    #[must_use]
    pub fn primary_table(&self) -> Option<&CountryTable> {
        self.primary
    }

    /// Fallback country-prefix table.
    #[must_use]
    pub fn fallback_table(&self) -> Option<&CountryTable> {
        self.fallback
    }

    /// Match a callsign against primary then fallback prefix tables.
    #[must_use]
    pub fn lookup_info(&self, callsign: &str) -> Option<CountryInfo> {
        let info = self
            .primary
            .and_then(|table| table.lookup_info(callsign))
            .or_else(|| self.fallback.and_then(|table| table.lookup_info(callsign)))?;
        Some(enrich_country_info(info, self.names))
    }

    /// Return the cleaned country label used for grouping diagnostics.
    #[must_use]
    pub fn grouping_label(&self, callsign: &str) -> Option<String> {
        self.lookup_info(callsign).map(|country| {
            if country.cleaned_name.is_empty() {
                country.name
            } else {
                country.cleaned_name
            }
        })
    }

    /// Return all prefix-table records in source priority order.
    #[must_use]
    pub fn records(&self) -> Vec<CountryInfo> {
        self.primary
            .into_iter()
            .chain(self.fallback)
            .flat_map(CountryTable::records)
            .map(|info| enrich_country_info(info, self.names))
            .collect()
    }

    /// Summarize loaded country sidecars.
    #[must_use]
    pub fn statistics(&self) -> CountryCatalogStatistics {
        let mut raw_labels = BTreeSet::new();
        let mut cleaned_labels = BTreeSet::new();
        for record in self.records() {
            raw_labels.insert(record.raw_name);
            cleaned_labels.insert(record.cleaned_name);
        }
        CountryCatalogStatistics {
            primary_rules: self.primary.map_or(0, CountryTable::len),
            fallback_rules: self.fallback.map_or(0, CountryTable::len),
            name_centroids: self.names.map_or(0, CountryNameTable::len),
            raw_labels: raw_labels.len(),
            cleaned_labels: cleaned_labels.len(),
        }
    }
}

/// Summary of archive snapshots attached to a callsign lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySummary {
    /// Number of historical snapshots.
    pub snapshot_count: usize,
    /// Publication vintages observed in history.
    pub vintages: Vec<u16>,
}

/// User-workflow station profile.
#[derive(Debug, Clone, PartialEq)]
pub struct StationProfile {
    /// Normalized callsign.
    pub callsign: String,
    /// Lookup status after database sources are consulted.
    pub status: LookupStatus,
    /// Best current snapshot, if one is available.
    pub current: Option<CallSnapshot>,
    /// Archive summary.
    pub history: HistorySummary,
    /// Matched country metadata.
    pub country: Option<CountryInfo>,
    /// HamCall.net web lookup count.
    pub lookup_count: Option<LookupCountRecord>,
    /// Profile assets.
    pub assets: ProfileAssets,
}

/// Map-oriented station context.
pub struct StationMap<'db> {
    db: &'db CallBook,
    callsign: String,
    mappable_snapshot: Option<CallSnapshot>,
    country: Option<CountryInfo>,
    assets: ProfileAssets,
}

/// Lazy map-layer catalog over all supported geographic sidecars.
pub struct MapLayers<'db> {
    db: &'db CallBook,
}

/// Parsed map-layer coverage summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MapLayerStatistics {
    /// `wc.dat` boundary segments.
    pub world_segments: usize,
    /// `wc.dat` coordinate count.
    pub world_points: usize,
    /// `cb.dat` boundary segments.
    pub county_segments: usize,
    /// `cb.dat` coordinate count.
    pub county_points: usize,
    /// `USCOUN.DAT` county records.
    pub us_counties: usize,
    /// `USCOUN.DAT` coordinate count.
    pub us_county_points: usize,
    /// `state.dat` vector paths.
    pub state_segments: usize,
    /// `state.dat` coordinate count.
    pub state_points: usize,
}

impl MapLayers<'_> {
    /// World/country boundary dataset.
    pub fn world_boundaries(&self) -> Result<Option<Arc<BoundaryDataset>>> {
        self.db.world_boundary_dataset()
    }

    /// County boundary dataset.
    pub fn county_boundaries(&self) -> Result<Option<Arc<BoundaryDataset>>> {
        self.db.county_boundary_dataset()
    }

    /// United States county boundary dataset.
    pub fn us_county_boundaries(&self) -> Result<Option<Arc<UsCountyBoundaryDataset>>> {
        self.db.us_county_boundaries()
    }

    /// Official state-map vector paths.
    pub fn state_vectors(&self) -> Result<Option<Arc<StateVectorDataset>>> {
        self.db.state_vector_dataset()
    }

    /// Load available layers and return their coverage counts.
    pub fn statistics(&self) -> Result<MapLayerStatistics> {
        let world = self.world_boundaries()?;
        let county = self.county_boundaries()?;
        let us_county = self.us_county_boundaries()?;
        let state = self.state_vectors()?;
        Ok(MapLayerStatistics {
            world_segments: world.as_ref().map_or(0, |layer| layer.segments.len()),
            world_points: world.as_ref().map_or(0, |layer| layer.point_count()),
            county_segments: county.as_ref().map_or(0, |layer| layer.segments.len()),
            county_points: county.as_ref().map_or(0, |layer| layer.point_count()),
            us_counties: us_county.as_ref().map_or(0, |layer| layer.counties.len()),
            us_county_points: us_county.as_ref().map_or(0, |layer| {
                layer
                    .counties
                    .iter()
                    .map(|county| county.points.len())
                    .sum()
            }),
            state_segments: state.as_ref().map_or(0, |layer| layer.segments.len()),
            state_points: state.as_ref().map_or(0, |layer| layer.point_count()),
        })
    }
}

/// Rendering controls for [`StationMap`] SVG output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StationMapRenderOptions {
    /// Maximum number of boundary segments to render.
    pub max_boundary_segments: Option<usize>,
    /// Include `cb.dat` county boundaries.
    pub include_county_boundaries: bool,
    /// Include `USCOUN.DAT` county boundaries.
    pub include_us_county_boundaries: bool,
    /// Include `state.dat` vector paths.
    pub include_state_vectors: bool,
}

impl StationMapRenderOptions {
    /// Options for a lightweight preview map.
    #[must_use]
    pub fn preview() -> Self {
        Self {
            max_boundary_segments: Some(512),
            ..Self::default()
        }
    }

    /// Options for rendering every supported map sidecar layer.
    #[must_use]
    pub fn all_layers() -> Self {
        Self {
            max_boundary_segments: None,
            include_county_boundaries: true,
            include_us_county_boundaries: true,
            include_state_vectors: true,
        }
    }
}

impl From<AssetMetadata> for AssetRef {
    fn from(value: AssetMetadata) -> Self {
        Self {
            kind: value.kind,
            key: value.key,
            media_type: value.media_type,
            path: value.path,
        }
    }
}

impl From<AssetMetadata> for TextAssetRef {
    fn from(value: AssetMetadata) -> Self {
        Self {
            kind: value.kind,
            key: value.key,
            media_type: value.media_type,
            path: value.path,
        }
    }
}

impl<'db> StationMap<'db> {
    /// Normalized callsign for this map context.
    #[must_use]
    pub fn callsign(&self) -> &str {
        &self.callsign
    }

    /// Station location parsed from the best mappable snapshot.
    #[must_use]
    pub fn station_location(&self) -> Option<GeoPoint> {
        self.mappable_snapshot.as_ref().and_then(snapshot_location)
    }

    /// Matched country metadata.
    #[must_use]
    pub fn country(&self) -> Option<&CountryInfo> {
        self.country.as_ref()
    }

    /// Country flag image.
    #[must_use]
    pub fn country_flag(&self) -> Option<&AssetRef> {
        self.assets.country_flag()
    }

    /// Country map image.
    #[must_use]
    pub fn country_map(&self) -> Option<&AssetRef> {
        self.assets.country_map()
    }

    /// Lazy map-layer catalog for this station map.
    #[must_use]
    pub fn layers(&self) -> MapLayers<'db> {
        self.db.map_layers()
    }

    /// World/country boundary dataset.
    pub fn world_boundaries(&self) -> Result<Option<Arc<BoundaryDataset>>> {
        self.layers().world_boundaries()
    }

    /// County boundary dataset.
    pub fn county_boundaries(&self) -> Result<Option<Arc<BoundaryDataset>>> {
        self.layers().county_boundaries()
    }

    /// United States county boundary dataset.
    pub fn us_county_boundaries(&self) -> Result<Option<Arc<UsCountyBoundaryDataset>>> {
        self.layers().us_county_boundaries()
    }

    /// Official state-map vector paths.
    pub fn state_vectors(&self) -> Result<Option<Arc<StateVectorDataset>>> {
        self.layers().state_vectors()
    }

    /// Render a simple SVG map from all available boundaries and station marker.
    ///
    /// This can produce a large string for real boundary sidecars; use
    /// [`Self::render_svg_with_options`] with [`StationMapRenderOptions::preview`]
    /// for UI previews.
    pub fn render_svg(&self) -> Result<Option<String>> {
        self.render_svg_with_options(StationMapRenderOptions::default())
    }

    /// Render a simple SVG map using explicit render options.
    pub fn render_svg_with_options(
        &self,
        options: StationMapRenderOptions,
    ) -> Result<Option<String>> {
        let location = self.station_location();
        let world_boundaries = self.world_boundaries()?;
        let county_boundaries = options
            .include_county_boundaries
            .then(|| self.county_boundaries())
            .transpose()?
            .flatten();
        let us_county_boundaries = options
            .include_us_county_boundaries
            .then(|| self.us_county_boundaries())
            .transpose()?
            .flatten();
        let state_vectors = options
            .include_state_vectors
            .then(|| self.state_vectors())
            .transpose()?
            .flatten();
        if location.is_none()
            && world_boundaries.is_none()
            && county_boundaries.is_none()
            && us_county_boundaries.is_none()
            && state_vectors.is_none()
        {
            return Ok(None);
        }
        let mut svg =
            String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-180 -90 360 180\">");
        if let Some(boundaries) = world_boundaries {
            render_boundary_segments(
                &mut svg,
                &boundaries.segments,
                options.max_boundary_segments,
                "#789",
                0.25,
            );
        }
        if let Some(boundaries) = county_boundaries {
            render_boundary_segments(
                &mut svg,
                &boundaries.segments,
                options.max_boundary_segments,
                "#9ab",
                0.18,
            );
        }
        if let Some(boundaries) = us_county_boundaries {
            render_us_county_segments(&mut svg, &boundaries, options.max_boundary_segments);
        }
        if let Some(vectors) = state_vectors {
            render_state_vector_segments(
                &mut svg,
                &vectors.segments,
                options.max_boundary_segments,
            );
        }
        if let Some(location) = location {
            svg.push_str(&format!(
                "<circle cx=\"{:.6}\" cy=\"{:.6}\" r=\"1.5\" fill=\"#d22\"/>",
                location.lon, -location.lat
            ));
        }
        svg.push_str("</svg>");
        Ok(Some(svg))
    }
}

/// Supported sidecar asset classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// Callsign biography text file under `bios/`.
    Biography,
    /// Callsign photo/QSL image under `photos/`.
    Photo,
    /// Country flag image under `flags/`.
    Flag,
    /// Country map image under `maps/`.
    Map,
    /// HamCall-level sidecar file.
    SidecarData,
}

/// Catalog for profile, country, manifest, and sidecar assets.
pub struct AssetCatalog<'db> {
    db: &'db CallBook,
}

/// Asset sidecar coverage diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetCatalogDiagnostics {
    /// Database-level sidecar files discovered.
    pub sidecar_files: usize,
    /// Entries parsed from `photos/PHOTOS.TXT`.
    pub photo_manifest_entries: usize,
    /// Manifest entries that name a file or relative path.
    pub photo_manifest_entries_with_file: usize,
    /// Manifest file references resolved to at least one on-disk file.
    pub photo_manifest_files_found: usize,
    /// Manifest file references that did not resolve to an on-disk file.
    pub photo_manifest_files_missing: usize,
    /// Whether a `bios/` directory exists.
    pub bios_dir_present: bool,
    /// Whether a `photos/` directory exists.
    pub photos_dir_present: bool,
    /// Whether a `flags/` directory exists.
    pub flags_dir_present: bool,
    /// Whether a `maps/` directory exists.
    pub maps_dir_present: bool,
}

impl AssetCatalog<'_> {
    /// Return database-level sidecar files.
    #[must_use]
    pub fn sidecar_files(&self) -> Vec<AssetMetadata> {
        self.db.sidecar_files()
    }

    /// Return biography and photo assets associated with a callsign.
    #[must_use]
    pub fn callsign_assets(&self, callsign: &str) -> Vec<AssetMetadata> {
        self.db.callsign_assets(callsign)
    }

    /// Return flag and map assets for the callsign's matched country.
    #[must_use]
    pub fn country_assets_for_callsign(&self, callsign: &str) -> Vec<AssetMetadata> {
        self.db.country_assets_for_callsign(callsign)
    }

    /// Resolve profile assets for a callsign.
    pub fn profile_assets(&self, callsign: &str) -> Result<ProfileAssets> {
        let country = self.db.country_info(callsign);
        self.db.profile_assets(callsign, country.as_ref())
    }

    /// Return filesystem and manifest coverage diagnostics.
    pub fn diagnostics(&self) -> Result<AssetCatalogDiagnostics> {
        self.db.asset_catalog_diagnostics()
    }
}

/// Counts split into broad jurisdiction buckets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JurisdictionCounts {
    /// United States records.
    pub united_states: usize,
    /// Canada records.
    pub canada: usize,
    /// Non-US/non-Canada records.
    pub international: usize,
    /// Records that could not be classified from available metadata.
    pub unknown: usize,
}

impl JurisdictionCounts {
    /// Total records across all jurisdiction buckets.
    #[must_use]
    pub fn total(&self) -> usize {
        self.united_states + self.canada + self.international + self.unknown
    }

    fn increment(&mut self, jurisdiction: Jurisdiction) {
        match jurisdiction {
            Jurisdiction::UnitedStates => self.united_states += 1,
            Jurisdiction::Canada => self.canada += 1,
            Jurisdiction::International => self.international += 1,
            Jurisdiction::Unknown => self.unknown += 1,
        }
    }
}

/// Archive counts for one publication year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveYearStatistics {
    /// Archive publication year.
    pub year: u16,
    /// Counts for that year.
    pub counts: JurisdictionCounts,
}

impl CallBook {
    /// Create a builder for opening a HamCall data tree.
    pub fn builder(data_path: impl AsRef<Path>) -> CallBookBuilder {
        CallBookBuilder {
            data_path: data_path.as_ref().to_owned(),
        }
    }

    /// Open a HamCall data tree.
    ///
    /// Recognised inputs for `data_path`:
    ///
    /// - The DVD root containing `ham0/` (2025 layout).
    /// - The `ham0/` directory itself (2025 layout).
    pub fn open(data_path: impl AsRef<Path>) -> Result<Self> {
        Self::builder(data_path).open()
    }

    fn open_path(data_path: impl AsRef<Path>) -> Result<Self> {
        let supplied = data_path.as_ref();
        if !supplied.is_dir() {
            return Err(Error::DataPathNotFound(supplied.to_owned()));
        }

        let v2 = open_v2(supplied)?;

        if v2.is_none() {
            return Err(Error::NoDataFiles(supplied.to_owned()));
        }

        Ok(Self {
            v2,
            lookup_counts: Mutex::new(None),
            photo_catalog: Mutex::new(None),
            world_boundaries: Mutex::new(None),
            county_boundaries: Mutex::new(None),
            us_county_boundaries: Mutex::new(None),
            state_vectors: Mutex::new(None),
        })
    }

    /// Look up a callsign in the verified database sources.
    pub fn lookup(&self, callsign: &str) -> Result<CallsignEntry<'_>> {
        Ok(CallsignEntry {
            db: self,
            report: self.lookup_report(callsign)?,
        })
    }

    /// Build a station profile for a callsign.
    pub fn profile_for_callsign(&self, callsign: &str) -> Result<StationProfile> {
        let report = self.lookup_report(callsign)?;
        self.profile_from_report(&report)
    }

    /// Build map context for a callsign when lookup data exists.
    pub fn map_for_callsign(&self, callsign: &str) -> Result<Option<StationMap<'_>>> {
        let report = self.lookup_report(callsign)?;
        if matches!(report.status, LookupStatus::NotFound) {
            return Ok(None);
        }
        Ok(Some(self.map_from_report(&report)?))
    }

    /// Return the lazy map-layer catalog.
    #[must_use]
    pub fn map_layers(&self) -> MapLayers<'_> {
        MapLayers { db: self }
    }

    /// Return the asset catalog.
    #[must_use]
    pub fn asset_catalog(&self) -> AssetCatalog<'_> {
        AssetCatalog { db: self }
    }

    pub(crate) fn lookup_report(&self, callsign: &str) -> Result<LookupResult> {
        let mut scratch = LookupScratch::default();
        self.lookup_report_with_scratch(callsign, &mut scratch)
    }

    fn lookup_report_with_scratch(
        &self,
        callsign: &str,
        scratch: &mut LookupScratch,
    ) -> Result<LookupResult> {
        let key = Callsign::parse(callsign)?;
        let query = key.to_string();
        let mut current = self
            .lookup_us_current(&query)
            .map(CallSnapshot::from_us_csv);
        let mut history = Vec::new();

        if let Some(v2) = &self.v2 {
            for snapshot in v2.lookup_dat_idx(&query) {
                merge_snapshot(&mut current, &mut history, snapshot);
            }
            v2.lookup_hci_into(&query, scratch);
            for snapshot in scratch.hci_snapshots.drain(..) {
                merge_snapshot(&mut current, &mut history, snapshot);
            }
        }

        history.sort_by(|a, b| {
            a.vintage
                .cmp(&b.vintage)
                .then_with(|| a.callsign.cmp(&b.callsign))
        });

        let status = if current.is_some() {
            LookupStatus::Current
        } else if history.is_empty() {
            LookupStatus::NotFound
        } else {
            LookupStatus::ArchiveOnly
        };

        Ok(LookupResult {
            query,
            status,
            current,
            history,
        })
    }

    /// Return assets associated with a callsign and its matched country.
    #[must_use]
    pub fn assets(&self, callsign: &str) -> Vec<AssetMetadata> {
        let mut assets = self.callsign_assets(callsign);
        assets.extend(self.country_assets_for_callsign(callsign));
        assets
    }

    /// Return the matched country metadata for `callsign`.
    #[must_use]
    pub fn country(&self, callsign: &str) -> Option<CountryMatch> {
        self.country_info(callsign)
            .map(|country| country.to_match())
    }

    /// Return rich matched country metadata for `callsign`.
    #[must_use]
    pub fn country_info(&self, callsign: &str) -> Option<CountryInfo> {
        self.country_catalog().lookup_info(callsign)
    }

    /// Return the unified country catalog.
    #[must_use]
    pub fn country_catalog(&self) -> CountryCatalog<'_> {
        let Some(v2) = &self.v2 else {
            return CountryCatalog {
                primary: None,
                fallback: None,
                names: None,
            };
        };
        CountryCatalog {
            primary: v2.countries.as_ref(),
            fallback: v2.pc_countries.as_ref(),
            names: v2.country_names.as_ref(),
        }
    }

    /// Return the loaded primary country-prefix catalog, when available.
    #[must_use]
    pub fn country_prefix_catalog(&self) -> Option<&CountryTable> {
        self.v2.as_ref().and_then(|v2| v2.countries.as_ref())
    }

    /// Return the loaded fallback country-prefix catalog, when available.
    #[must_use]
    pub fn pc_country_catalog(&self) -> Option<&CountryTable> {
        self.v2.as_ref().and_then(|v2| v2.pc_countries.as_ref())
    }

    /// Return the current-US catalog loaded from `usa.csv.zip`, when available.
    #[must_use]
    pub fn current_us_catalog(&self) -> Option<&UsCsvFile> {
        self.v2.as_ref().and_then(|v2| v2.us_csv.as_ref())
    }

    /// Return the interest-code catalog loaded from `ham0/interest`, when available.
    #[must_use]
    pub fn interest_catalog(&self) -> Option<&InterestTable> {
        self.v2.as_ref().and_then(|v2| v2.interests.as_ref())
    }

    /// Find current and archive snapshots that contain a four-digit interest code.
    pub fn search_interest(&self, code: &str) -> Result<InterestSearch> {
        let code = normalize_interest_code(code)?;
        let definition = self
            .interest_catalog()
            .and_then(|catalog| catalog.lookup(&code))
            .cloned();
        let Some(v2) = &self.v2 else {
            return Ok(InterestSearch {
                code,
                definition,
                matches: Vec::new(),
            });
        };
        Ok(InterestSearch {
            matches: scan_interest_code(&v2.dat_mmap, &code),
            code,
            definition,
        })
    }

    fn photo_catalog(&self) -> Result<Option<Arc<PhotoCatalog>>> {
        let Some(v2) = &self.v2 else {
            return Ok(None);
        };
        let path = v2.dir.join("photos/PHOTOS.TXT");
        if !path.is_file() {
            return Ok(None);
        }
        let mut catalog = self
            .photo_catalog
            .lock()
            .expect("photo catalog cache lock poisoned");
        if catalog.is_none() {
            *catalog = Some(Arc::new(PhotoCatalog::open(path)?));
        }
        Ok(catalog.clone())
    }

    /// Return advanced format and diagnostics APIs.
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics<'_> {
        Diagnostics { db: self }
    }

    /// Create a reusable lookup context.
    #[must_use]
    pub fn batch_lookup(&self) -> BatchLookup<'_> {
        BatchLookup {
            db: self,
            scratch: LookupScratch::default(),
        }
    }

    /// Create an iterator builder for database entries.
    #[must_use]
    pub fn entries(&self) -> Entries<'_> {
        Entries {
            db: self,
            include_history: true,
        }
    }

    /// Trace one lookup through IDX, HCI, DAT boundary scans, and parsing.
    ///
    /// This diagnostics API is explicit and allocation-heavy by design; the
    /// normal [`Self::lookup`] path does not call it.
    fn trace_lookup(&self, callsign: &str) -> Result<LookupTrace> {
        self.trace_lookup_with_limit(callsign, DEFAULT_TRACE_PREVIEW_LIMIT)
    }

    /// Trace one lookup with a custom hex/text preview byte limit.
    fn trace_lookup_with_limit(&self, callsign: &str, limit: usize) -> Result<LookupTrace> {
        let key = Callsign::parse(callsign)?;
        let query = callsign.to_owned();
        let normalized_callsign = key.to_string();
        let us_csv_hit = self.lookup_us_current(&normalized_callsign).is_some();
        let (idx_hits, hci_keys) = if let Some(v2) = &self.v2 {
            (
                v2.trace_dat_idx(&normalized_callsign, limit),
                v2.trace_hci(&normalized_callsign, limit),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let final_status = self.lookup_report(&normalized_callsign)?.status;
        Ok(LookupTrace {
            query,
            normalized_callsign,
            us_csv_hit,
            idx_hits,
            hci_keys,
            final_status,
        })
    }

    /// Look up `callsign` in the 2025-format IDX and return the raw encoded
    /// DAT bytes for the matched record.
    ///
    /// "Raw" means: the byte slice from the offset given in the IDX up
    /// to the start of the next IDX entry's offset. Use
    /// [`RawV2Record::best_decoded_candidate`] to decode the slice.
    /// Returns `None` if there's no 2025 shard or the callsign is not in
    /// the IDX.
    #[must_use]
    fn lookup_v2_raw(&self, callsign: &str) -> Option<RawV2Record<'_>> {
        let v2 = self.v2.as_ref()?;
        let upper = callsign.trim().to_ascii_uppercase();
        let entry = v2
            .idx
            .find_callsign(upper.as_bytes())
            .or_else(|| v2.idx.find_exact(upper.as_bytes()))?;
        v2.raw_record_for(entry)
    }

    /// Look up a current US callsign in the shipped `usa.csv.zip` catalog.
    ///
    /// This covers current FCC-derived US rows that are not necessarily
    /// present in `hamcall.idx`; historical and international rows still
    /// require the DAT/HCI paths.
    fn lookup_us_current(&self, callsign: &str) -> Option<&UsCsvRecord> {
        let us_csv = self.v2.as_ref().and_then(|v| v.us_csv.as_ref())?;
        us_csv.get(callsign)
    }

    fn sample_us_current_callsign(&self) -> Option<String> {
        self.v2
            .as_ref()
            .and_then(|v2| v2.us_csv.as_ref())
            .and_then(|us_csv| {
                us_csv
                    .callsigns()
                    .find(|callsign| Callsign::parse(callsign).is_ok())
                    .map(str::to_owned)
            })
    }

    /// Whether this database opened a 2025-layout shard.
    #[inline]
    #[must_use]
    fn has_v2(&self) -> bool {
        self.v2.is_some()
    }

    /// Number of entries in the 2025-format IDX (or `0` if none).
    #[inline]
    #[must_use]
    fn v2_idx_len(&self) -> usize {
        self.v2.as_ref().map(|v| v.idx.len()).unwrap_or(0)
    }

    /// Borrow the 2025-format IDX for advanced inspection (verify,
    /// iteration, etc.).
    #[inline]
    #[must_use]
    fn v2_idx(&self) -> Option<&TextIdxFile> {
        self.v2.as_ref().map(|v| &v.idx)
    }

    /// Whether this database opened the 2025 HCI corpus.
    #[inline]
    #[must_use]
    fn has_hci(&self) -> bool {
        self.v2.as_ref().and_then(|v| v.hci.as_ref()).is_some()
    }

    /// Number of raw HCI records, or `0` when no HCI corpus is open.
    #[inline]
    #[must_use]
    fn hci_len(&self) -> usize {
        self.v2
            .as_ref()
            .and_then(|v| v.hci.as_ref())
            .map(HciFile::len)
            .unwrap_or(0)
    }

    /// Return one encoded HCI record by ordinal.
    #[must_use]
    fn hci_raw_record(&self, index: usize) -> Option<RawHciRecord<'_>> {
        self.v2.as_ref()?.hci.as_ref()?.raw_record(index)
    }

    /// Decode one HCI record by ordinal.
    ///
    /// This is an inspection helper; normal callsign lookup uses the HCI
    /// exact-key index internally.
    #[must_use]
    fn hci_decoded_record(&self, index: usize) -> Option<DecodedHciRecord> {
        self.v2.as_ref()?.hci.as_ref()?.decode_record(index)
    }

    /// Aggregate statistics.
    #[must_use]
    fn stats(&self) -> Stats {
        let modern_idx_records = self.v2.as_ref().map_or(0, |v| v.idx.len());
        let modern_hci_records = self
            .v2
            .as_ref()
            .and_then(|v| v.hci.as_ref())
            .map_or(0, HciFile::len);
        let has_us_csv = self.v2.as_ref().is_some_and(|v| v.us_csv.is_some());
        Stats {
            shard_count: usize::from(self.v2.is_some()),
            total_records: modern_idx_records,
            modern_idx_records,
            modern_hci_records,
            has_us_csv,
        }
    }

    /// Scan lookup keys and count current versus historical records.
    ///
    /// This is more expensive than [`Self::stats`] because it walks the
    /// IDX and de-duplicates current callsigns across `hamcall.idx`
    /// and `usa.csv.zip`.
    #[must_use]
    fn record_statistics(&self) -> RecordStatistics {
        let mut current_calls = BTreeMap::<String, Jurisdiction>::new();
        let mut archive = JurisdictionCounts::default();
        let mut archive_years = BTreeMap::<u16, JurisdictionCounts>::new();
        let mut modern_idx_current_records = 0usize;
        let mut modern_idx_archive_records = 0usize;

        if let Some(v2) = &self.v2 {
            for entry in v2.idx.iter() {
                let Some((callsign, vintage)) = stat_key_parts(entry.key) else {
                    continue;
                };
                let jurisdiction = v2.classify_callsign(&callsign);
                if let Some(year) = vintage {
                    modern_idx_archive_records += 1;
                    archive.increment(jurisdiction);
                    archive_years
                        .entry(year)
                        .or_default()
                        .increment(jurisdiction);
                } else {
                    modern_idx_current_records += 1;
                    current_calls.entry(callsign).or_insert(jurisdiction);
                }
            }

            if let Some(us_csv) = &v2.us_csv {
                for callsign in us_csv.callsigns() {
                    current_calls
                        .entry(callsign.to_owned())
                        .or_insert(Jurisdiction::UnitedStates);
                }
            }
        }

        let mut current = JurisdictionCounts::default();
        for jurisdiction in current_calls.values().copied() {
            current.increment(jurisdiction);
        }

        let archive_years = archive_years
            .into_iter()
            .map(|(year, counts)| ArchiveYearStatistics { year, counts })
            .collect();
        let total_records_including_archive = current.total() + archive.total();
        let hci_callsigns = self
            .v2
            .as_ref()
            .and_then(|v| v.hci.as_ref())
            .map(HciFile::callsign_posting_statistics);
        RecordStatistics {
            current,
            archive,
            modern_idx_current_records,
            modern_idx_archive_records,
            us_csv_current_records: self
                .v2
                .as_ref()
                .and_then(|v| v.us_csv.as_ref())
                .map_or(0, UsCsvFile::len),
            total_records_including_archive,
            archive_years,
            hci_callsigns,
        }
    }

    /// Scan lookup sources for interest-profile coverage.
    #[must_use]
    fn interest_statistics(&self) -> InterestStatistics {
        let catalog_entries = self
            .v2
            .as_ref()
            .and_then(|v| v.interests.as_ref())
            .map_or(0, InterestTable::len);

        let Some(v2) = &self.v2 else {
            return InterestStatistics {
                catalog_entries,
                current_callsigns_with_interests: 0,
                current_snapshots_with_interests: 0,
                archive_snapshots_with_interests: 0,
                current_callsigns_with_resolved_interests: 0,
                unknown_codes: Vec::new(),
            };
        };

        let mut scan = InterestScan::new(v2.interests.as_ref());
        scan.scan_dat(&v2.dat_mmap);
        scan.finish(catalog_entries)
    }

    /// Scan the DAT file and count observed field tags.
    #[must_use]
    fn modern_tag_statistics(&self) -> ModernTagStatistics {
        let Some(v2) = &self.v2 else {
            return ModernTagStatistics { tags: Vec::new() };
        };
        let mut scan = TagScan::default();
        scan.scan_dat(&v2.dat_mmap);
        scan.finish()
    }

    fn callsign_hci_posting_start_invariant(&self) -> Option<HciPostingStartInvariant> {
        self.v2
            .as_ref()
            .and_then(V2Shard::callsign_hci_posting_start_invariant)
    }

    /// Return sidecar assets associated with a callsign.
    #[must_use]
    pub fn callsign_assets(&self, callsign: &str) -> Vec<AssetMetadata> {
        let Some(v2) = &self.v2 else {
            return Vec::new();
        };
        let Ok(callsign) = Callsign::parse(callsign) else {
            return Vec::new();
        };
        let key = callsign.to_string();
        let mut out = Vec::new();
        collect_callsign_assets(&v2.dir.join("bios"), AssetKind::Biography, &key, &mut out);
        collect_callsign_assets(&v2.dir.join("photos"), AssetKind::Photo, &key, &mut out);
        out.sort_by(|left, right| {
            left.kind
                .cmp_rank()
                .cmp(&right.kind.cmp_rank())
                .then_with(|| left.path.cmp(&right.path))
        });
        out
    }

    /// Return flag and map assets for the callsign's matched country.
    #[must_use]
    pub fn country_assets_for_callsign(&self, callsign: &str) -> Vec<AssetMetadata> {
        let Some(v2) = &self.v2 else {
            return Vec::new();
        };
        let Some(country) = self.country_info(callsign) else {
            return Vec::new();
        };
        let Some(code) = country.code else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect_country_assets(&v2.dir.join("flags"), AssetKind::Flag, &code, &mut out);
        collect_country_assets(&v2.dir.join("maps"), AssetKind::Map, &code, &mut out);
        out.sort_by(|left, right| {
            left.kind
                .cmp_rank()
                .cmp(&right.kind.cmp_rank())
                .then_with(|| left.path.cmp(&right.path))
        });
        out
    }

    fn profile_from_report(&self, report: &LookupResult) -> Result<StationProfile> {
        let country = self.country_info(&report.query);
        let lookup_count = self.lookup_count(&report.query)?;
        let assets = self.profile_assets(&report.query, country.as_ref())?;
        let mut vintages = report
            .history
            .iter()
            .filter_map(|snapshot| snapshot.vintage)
            .collect::<Vec<_>>();
        vintages.sort_unstable();
        vintages.dedup();
        Ok(StationProfile {
            callsign: report.query.clone(),
            status: report.status,
            current: report.current.clone(),
            history: HistorySummary {
                snapshot_count: report.history.len(),
                vintages,
            },
            country,
            lookup_count,
            assets,
        })
    }

    fn map_from_report<'db>(&'db self, report: &LookupResult) -> Result<StationMap<'db>> {
        let country = self.country_info(&report.query);
        let assets = self.profile_assets(&report.query, country.as_ref())?;
        let mappable_snapshot = best_mappable_snapshot(report);
        Ok(StationMap {
            db: self,
            callsign: report.query.clone(),
            mappable_snapshot,
            country,
            assets,
        })
    }

    fn profile_assets(
        &self,
        callsign: &str,
        country: Option<&CountryInfo>,
    ) -> Result<ProfileAssets> {
        let catalog = self.photo_catalog().ok().flatten();
        let mut assets = self.callsign_assets(callsign);
        if let Some(catalog) = &catalog {
            self.collect_manifest_photos(callsign, catalog, &mut assets);
        }
        if let Some(country) = country.and_then(|country| country.code.as_deref()) {
            if let Some(v2) = &self.v2 {
                collect_country_assets(
                    &v2.dir.join("flags"),
                    AssetKind::Flag,
                    country,
                    &mut assets,
                );
                collect_country_assets(&v2.dir.join("maps"), AssetKind::Map, country, &mut assets);
            }
        }
        assets.sort_by(|left, right| {
            left.kind
                .cmp_rank()
                .cmp(&right.kind.cmp_rank())
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut photos = assets
            .iter()
            .filter(|asset| asset.kind == AssetKind::Photo)
            .cloned()
            .map(AssetRef::from)
            .collect::<Vec<_>>();
        dedup_assets_by_path(&mut photos);
        if let Some(catalog) = catalog {
            let manifest = catalog.lookup(callsign);
            photos.sort_by(|left, right| {
                let left_rank = photo_manifest_rank(left, &manifest);
                let right_rank = photo_manifest_rank(right, &manifest);
                left_rank
                    .cmp(&right_rank)
                    .then_with(|| left.path.cmp(&right.path))
            });
        }

        Ok(ProfileAssets {
            photos,
            bio: assets
                .iter()
                .find(|asset| asset.kind == AssetKind::Biography)
                .cloned()
                .map(TextAssetRef::from),
            country_flag: assets
                .iter()
                .find(|asset| asset.kind == AssetKind::Flag)
                .cloned()
                .map(AssetRef::from),
            country_map: assets
                .iter()
                .find(|asset| asset.kind == AssetKind::Map)
                .cloned()
                .map(AssetRef::from),
        })
    }

    fn collect_manifest_photos(
        &self,
        callsign: &str,
        catalog: &PhotoCatalog,
        assets: &mut Vec<AssetMetadata>,
    ) {
        let Some(v2) = &self.v2 else {
            return;
        };
        for entry in catalog.lookup(callsign) {
            let Some(file) = entry.file.as_deref() else {
                continue;
            };
            let Some(relative) = safe_manifest_photo_path(file) else {
                continue;
            };
            let direct = v2.dir.join("photos").join(&relative);
            if is_regular_file_no_symlink(&direct) {
                assets.push(asset_metadata(AssetKind::Photo, callsign, direct));
                continue;
            }
            collect_manifest_photo_fallback(&v2.dir.join("photos"), callsign, &relative, assets);
        }
    }

    /// Return database-level sidecar files discovered in the `ham0` layout.
    #[must_use]
    pub fn sidecar_files(&self) -> Vec<AssetMetadata> {
        let Some(v2) = &self.v2 else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for name in [
            "COUNTRYS.PC",
            "USCOUN.DAT",
            "countrys",
            "countrys.nam",
            "gcmcountrys",
            "counts.dat",
            "cb.dat",
            "wc.dat",
            "state.dat",
            "interest",
            "usa.csv.zip",
            "photos/PHOTOS.TXT",
        ] {
            let path = v2.dir.join(name);
            if path.is_file() {
                out.push(asset_metadata(AssetKind::SidecarData, name, path));
            }
        }
        out
    }

    fn asset_catalog_diagnostics(&self) -> Result<AssetCatalogDiagnostics> {
        let Some(v2) = &self.v2 else {
            return Ok(AssetCatalogDiagnostics::default());
        };
        let mut diagnostics = AssetCatalogDiagnostics {
            sidecar_files: self.sidecar_files().len(),
            bios_dir_present: v2.dir.join("bios").is_dir(),
            photos_dir_present: v2.dir.join("photos").is_dir(),
            flags_dir_present: v2.dir.join("flags").is_dir(),
            maps_dir_present: v2.dir.join("maps").is_dir(),
            ..AssetCatalogDiagnostics::default()
        };
        let Some(catalog) = self.photo_catalog()? else {
            return Ok(diagnostics);
        };
        diagnostics.photo_manifest_entries = catalog.entries().len();
        for entry in catalog.entries() {
            let Some(file) = entry.file.as_deref() else {
                continue;
            };
            diagnostics.photo_manifest_entries_with_file += 1;
            if manifest_photo_exists(&v2.dir.join("photos"), &entry.callsign, file) {
                diagnostics.photo_manifest_files_found += 1;
            } else {
                diagnostics.photo_manifest_files_missing += 1;
            }
        }
        Ok(diagnostics)
    }

    /// Look up a HamCall.net web lookup count from `counts.dat`.
    pub fn lookup_count(&self, callsign: &str) -> Result<Option<LookupCountRecord>> {
        Ok(self
            .lookup_counts()?
            .and_then(|counts| counts.lookup(callsign)))
    }

    /// Return the cached lookup-count catalog from `counts.dat`, when present.
    pub fn lookup_counts(&self) -> Result<Option<Arc<LookupCounts>>> {
        let Some(v2) = &self.v2 else {
            return Ok(None);
        };
        let path = v2.dir.join("counts.dat");
        if !path.is_file() {
            return Ok(None);
        }
        let mut lookup_counts = self
            .lookup_counts
            .lock()
            .expect("lookup count cache lock poisoned");
        if lookup_counts.is_none() {
            *lookup_counts = Some(Arc::new(LookupCounts::open(path)?));
        }
        Ok(lookup_counts.clone())
    }

    /// Open and cache the semantic world/country boundary dataset.
    pub fn world_boundary_dataset(&self) -> Result<Option<Arc<BoundaryDataset>>> {
        let Some(v2) = &self.v2 else {
            return Ok(None);
        };
        let path = v2.dir.join("wc.dat");
        if !path.is_file() {
            return Ok(None);
        }
        let mut cache = self
            .world_boundaries
            .lock()
            .expect("world boundary cache lock poisoned");
        if cache.is_none() {
            *cache = Some(Arc::new(BoundaryDataset::open(path, BoundaryKind::World)?));
        }
        Ok(cache.clone())
    }

    /// Open and cache the semantic county boundary dataset.
    pub fn county_boundary_dataset(&self) -> Result<Option<Arc<BoundaryDataset>>> {
        let Some(v2) = &self.v2 else {
            return Ok(None);
        };
        let path = v2.dir.join("cb.dat");
        if !path.is_file() {
            return Ok(None);
        }
        let mut cache = self
            .county_boundaries
            .lock()
            .expect("county boundary cache lock poisoned");
        if cache.is_none() {
            *cache = Some(Arc::new(BoundaryDataset::open(path, BoundaryKind::County)?));
        }
        Ok(cache.clone())
    }

    /// Open and cache the `USCOUN.DAT` county boundary dataset.
    pub fn us_county_boundaries(&self) -> Result<Option<Arc<UsCountyBoundaryDataset>>> {
        let Some(v2) = &self.v2 else {
            return Ok(None);
        };
        let path = v2.dir.join("USCOUN.DAT");
        if !path.is_file() {
            return Ok(None);
        }
        let mut cache = self
            .us_county_boundaries
            .lock()
            .expect("US county boundary cache lock poisoned");
        if cache.is_none() {
            *cache = Some(Arc::new(UsCountyBoundaryDataset::open(path)?));
        }
        Ok(cache.clone())
    }

    /// Open and cache the renderable vector paths from `state.dat`.
    pub fn state_vector_dataset(&self) -> Result<Option<Arc<StateVectorDataset>>> {
        let Some(v2) = &self.v2 else {
            return Ok(None);
        };
        let path = v2.dir.join("state.dat");
        if !path.is_file() {
            return Ok(None);
        }
        let mut cache = self
            .state_vectors
            .lock()
            .expect("state vector cache lock poisoned");
        if cache.is_none() {
            *cache = Some(Arc::new(PackedStateMap::open(path)?.vector_dataset()));
        }
        Ok(cache.clone())
    }

    /// Run the format verifier and return a human-readable report.
    #[must_use]
    fn verify(&self) -> String {
        use std::fmt::Write;
        let mut report = String::new();

        if let Some(v2) = &self.v2 {
            let rep = crate::idx_text::verify(&v2.idx);
            let _ = writeln!(
                report,
                "v2 (2025 DVD layout):\n  idx: {} ({} entries, dir={} anchors @ stride {})\n  dat: {} ({} B)\n  invariants: ascii_violations={} parse_failures={} key_order={} offset_monotonic={} {}",
                v2.idx.path().display(),
                rep.entries,
                v2.idx.directory_size(),
                v2.idx.directory_stride(),
                v2.dat_path.display(),
                v2.dat_mmap.len(),
                rep.non_ascii_bytes,
                rep.parse_failures,
                rep.key_order_violations,
                rep.offset_monotonicity_violations,
                if rep.is_clean() { "[CLEAN]" } else { "[FAIL]" },
            );
            if let (Some(fk), Some(lk)) = (&rep.first_key, &rep.last_key) {
                let _ = writeln!(
                    report,
                    "  first key: {:?}  last key: {:?}",
                    String::from_utf8_lossy(fk),
                    String::from_utf8_lossy(lk),
                );
            }
            if let (Some(fo), Some(lo)) = (rep.first_offset, rep.last_offset) {
                let dat_size = v2.dat_mmap.len() as u64;
                let _ = writeln!(
                    report,
                    "  first offset: {fo}  last offset: {lo}  dat_size: {dat_size}  delta: {}",
                    lo as i128 - dat_size as i128,
                );
            }
            let _ = writeln!(
                report,
                "  dat decoder: xor7 absolute-position mod101 [available]"
            );
            if let Some(hci) = &v2.hci {
                let rep = crate::hci::verify(hci);
                let _ = writeln!(
                    report,
                    "  hci: {} ({} offsets, {} indexed keys) -> {} ({} B)\n    invariants: offset_order={} out_of_bounds={} zero_len={} record_len={:?}..{:?}",
                    hci.index_path().display(),
                    rep.entries,
                    hci.indexed_key_count(),
                    hci.dat_path().display(),
                    rep.dat_size,
                    rep.offset_order_violations,
                    rep.out_of_bounds_offsets,
                    rep.zero_length_records,
                    rep.min_record_len,
                    rep.max_record_len,
                );
                if let (Some(first), Some(last)) = (rep.first_offset, rep.last_offset) {
                    let _ = writeln!(
                        report,
                        "    first offset: {first}  last offset: {last}  trailing: {}",
                        rep.dat_size as i128 - last as i128,
                    );
                }
            }
            if let Some(us_csv) = &v2.us_csv {
                let _ = writeln!(
                    report,
                    "  us_csv: {} [current US lookup available]",
                    us_csv.path().display(),
                );
            }
        }

        if self.v2.is_none() {
            report.push_str("(no sources open)\n");
        }
        report
    }
}

/// Raw DAT slice for [`Diagnostics::lookup_v2_raw`].
///
/// This inspection type holds the IDX key and the encoded bytes between two
/// adjacent IDX offsets. Use [`Self::best_decoded_candidate`] to decode the
/// slice with the verified DAT transform.
#[derive(Debug, Clone)]
pub struct RawV2Record<'a> {
    /// IDX key bytes (e.g. `b"W1AW:2000"`).
    pub key: &'a [u8],
    /// Byte offset into `hamcall.dat` where this record starts.
    pub dat_offset: u64,
    /// Length in bytes — distance to the next IDX entry's offset.
    pub raw_len: usize,
    /// Borrowed slice of the still-encoded DAT bytes.
    pub raw_bytes: &'a [u8],
}

impl RawV2Record<'_> {
    /// Return decoded phase candidates for this encoded DAT slice.
    #[must_use]
    pub fn decoded_candidates(&self) -> Vec<DecodedV2Candidate> {
        crate::v2_dat::decode_candidates(self.dat_offset, self.raw_bytes, self.key)
    }

    /// Return the highest-scoring decoded phase candidate.
    #[must_use]
    pub fn best_decoded_candidate(&self) -> Option<DecodedV2Candidate> {
        crate::v2_dat::best_candidate(self.dat_offset, self.raw_bytes, self.key)
    }
}

impl V2Shard {
    fn classify_callsign(&self, callsign: &str) -> Jurisdiction {
        self.countries
            .as_ref()
            .and_then(|countries| countries.lookup(callsign))
            .or_else(|| {
                self.pc_countries
                    .as_ref()
                    .and_then(|countries| countries.lookup(callsign))
            })
            .map_or(Jurisdiction::Unknown, |country| country.jurisdiction)
    }

    fn lookup_dat_idx(&self, callsign: &str) -> Vec<CallSnapshot> {
        let mut out = Vec::new();
        for entry in self.idx.find_callsign_all(callsign.as_bytes()) {
            let Some(raw) = self.raw_record_for(entry) else {
                continue;
            };
            let Some(decoded) = raw.best_decoded_candidate() else {
                continue;
            };
            out.extend(self.parse_and_filter(
                &decoded.bytes,
                Some(raw.key),
                callsign,
                SnapshotSource::HamCallDatIdx,
            ));
        }
        out
    }

    fn trace_dat_idx(&self, callsign: &str, limit: usize) -> Vec<IdxTrace> {
        let mut out = Vec::new();
        for entry in self.idx.find_callsign_all(callsign.as_bytes()) {
            let next_dat_offset = self
                .next_entry_after_key(entry.key)
                .map(|next| next.dat_offset.min(self.dat_mmap.len() as u64));
            let Some(raw) = self.raw_record_for(entry) else {
                continue;
            };
            let Some(decoded) = raw.best_decoded_candidate() else {
                continue;
            };
            let snapshots = self.parse_and_filter(
                &decoded.bytes,
                Some(raw.key),
                callsign,
                SnapshotSource::HamCallDatIdx,
            );
            out.push(IdxTrace {
                key: String::from_utf8_lossy(raw.key).into_owned(),
                dat_offset: raw.dat_offset,
                next_dat_offset,
                raw_len: raw.raw_len,
                decoded_hex_prefix: trace_hex_prefix(&decoded.bytes, limit),
                decoded_text_prefix: trace_text_prefix(&decoded.bytes, limit),
                parsed_snapshots: parsed_snapshot_trace(&snapshots),
            });
        }
        out
    }

    fn lookup_hci_into(&self, callsign: &str, scratch: &mut LookupScratch) {
        scratch.hci_seen_offsets.clear();
        scratch.hci_snapshots.clear();
        let Some(hci) = &self.hci else {
            return;
        };
        visit_hci_lookup_keys(callsign, hci.publication_years(), |key| {
            hci.visit_postings_for_key(key, |posting| {
                if !scratch.hci_seen_offsets.insert(posting.dat_offset) {
                    return;
                }
                scratch
                    .hci_snapshots
                    .extend(self.decode_and_parse_hci_offset(
                        posting.dat_offset,
                        callsign,
                        &mut scratch.hci_decoded,
                    ));
            });
        });
    }

    fn callsign_hci_posting_start_invariant(&self) -> Option<HciPostingStartInvariant> {
        let hci = self.hci.as_ref()?;
        let mut report = HciPostingStartInvariant {
            total_postings: 0,
            record_start_postings: 0,
            non_record_start_postings: 0,
            out_of_bounds_postings: 0,
            samples: Vec::new(),
        };

        hci.visit_callsign_postings(|key, posting| {
            report.total_postings += 1;
            let decoded_byte = usize::try_from(posting.dat_offset)
                .ok()
                .and_then(|offset| self.dat_mmap.get(offset).copied())
                .map(|byte| decode_dat_byte(posting.dat_offset, byte));

            match decoded_byte {
                Some(0xb5) => report.record_start_postings += 1,
                Some(byte) => {
                    report.non_record_start_postings += 1;
                    push_hci_posting_start_sample(&mut report, key, posting, Some(byte));
                }
                None => {
                    report.out_of_bounds_postings += 1;
                    push_hci_posting_start_sample(&mut report, key, posting, None);
                }
            }
        });

        Some(report)
    }

    fn trace_hci(&self, callsign: &str, limit: usize) -> Vec<HciKeyTrace> {
        let Some(hci) = &self.hci else {
            return Vec::new();
        };
        let mut out = Vec::new();
        visit_hci_lookup_keys(callsign, hci.publication_years(), |key| {
            let mut entries = Vec::new();
            hci.visit_records_for_key(key, |raw, header_len| {
                let decoded = hci.decode_record(raw.index);
                let decoded_header_bytes = decoded
                    .as_ref()
                    .map_or_else(Vec::new, |decoded| decoded.bytes.clone());
                let decoded_header = decoded_header_text(&decoded_header_bytes);
                let postings = hci
                    .postings_for_key(key)
                    .into_iter()
                    .filter(|posting| {
                        raw.raw_bytes
                            .get(header_len..)
                            .is_some_and(|bytes| posting_is_in_record(bytes, *posting))
                    })
                    .map(|posting| self.trace_hci_posting(posting, callsign, limit))
                    .collect();
                entries.push(HciEntryTrace {
                    ordinal: raw.index,
                    hci_dat_offset: raw.dat_offset,
                    raw_len: raw.raw_len,
                    header_len,
                    decoded_header,
                    encoded_hex_prefix: trace_hex_prefix(raw.raw_bytes, limit),
                    decoded_hex_prefix: trace_hex_prefix(&decoded_header_bytes, limit),
                    postings,
                });
            });
            out.push(HciKeyTrace {
                searched_key: String::from_utf8_lossy(key).into_owned(),
                hci_entries: entries,
            });
        });
        out
    }

    fn trace_hci_posting(
        &self,
        posting: HciPosting,
        callsign: &str,
        limit: usize,
    ) -> HciPostingTrace {
        let Some(bounds) = self.record_bounds_for_hci_posting(posting.dat_offset) else {
            let mut decoded = Vec::new();
            let snapshots =
                self.decode_and_parse_hci_offset(posting.dat_offset, callsign, &mut decoded);
            return HciPostingTrace {
                dat_offset: posting.dat_offset,
                position: posting.position,
                record_boundary_source: None,
                record_start: None,
                record_end: None,
                record_len: None,
                distance_to_record_start: None,
                distance_to_record_end: None,
                backward_scan_bytes: None,
                forward_scan_bytes: None,
                posting_matches_record_start: None,
                decoded_record_hex_prefix: trace_hex_prefix(&decoded, limit),
                decoded_record_text_prefix: trace_text_prefix(&decoded, limit),
                parsed_snapshots: parsed_snapshot_trace(&snapshots),
            };
        };
        let raw = &self.dat_mmap[bounds.start..bounds.end];
        let phase = crate::v2_dat::phase_for_dat_offset(bounds.start as u64);
        let mut decoded = Vec::with_capacity(raw.len());
        crate::v2_dat::decode_phase_into(bounds.start as u64, raw, phase, &mut decoded);
        let mut parsed_decoded = Vec::new();
        let snapshots =
            self.decode_and_parse_hci_offset(posting.dat_offset, callsign, &mut parsed_decoded);
        let dat_offset = usize::try_from(posting.dat_offset).ok();
        HciPostingTrace {
            dat_offset: posting.dat_offset,
            position: posting.position,
            record_boundary_source: Some(bounds.source),
            record_start: Some(bounds.start as u64),
            record_end: Some(bounds.end as u64),
            record_len: Some(bounds.end - bounds.start),
            distance_to_record_start: dat_offset.map(|offset| offset.saturating_sub(bounds.start)),
            distance_to_record_end: dat_offset.map(|offset| bounds.end.saturating_sub(offset)),
            backward_scan_bytes: bounds.backward_scan_bytes,
            forward_scan_bytes: bounds.forward_scan_bytes,
            posting_matches_record_start: Some(posting.dat_offset == bounds.start as u64),
            decoded_record_hex_prefix: trace_hex_prefix(&decoded, limit),
            decoded_record_text_prefix: trace_text_prefix(&decoded, limit),
            parsed_snapshots: parsed_snapshot_trace(&snapshots),
        }
    }

    fn decode_and_parse_hci_offset(
        &self,
        dat_offset: u64,
        callsign: &str,
        decoded: &mut Vec<u8>,
    ) -> Vec<CallSnapshot> {
        self.decode_hci_posting_record_into(dat_offset, decoded);
        let snapshots = self.parse_and_filter(decoded, None, callsign, SnapshotSource::HamCallHci);
        if !snapshots.is_empty() {
            return snapshots;
        }
        self.decode_and_parse_hci_offset_with_windows(dat_offset, callsign, decoded)
    }

    fn decode_and_parse_hci_offset_with_windows(
        &self,
        dat_offset: u64,
        callsign: &str,
        decoded: &mut Vec<u8>,
    ) -> Vec<CallSnapshot> {
        for max_len in [1024, 2048, 4096] {
            self.decode_from_offset_into(dat_offset, max_len, decoded);
            let snapshots = crate::v2_record::parse_complete_matching_snapshots(
                decoded,
                None,
                callsign,
                SnapshotSource::HamCallHci,
                self.countries.as_ref(),
                self.interests.as_ref(),
            );
            if !snapshots.is_empty() {
                return snapshots;
            }
        }
        self.decode_from_offset_into(dat_offset, 8192, decoded);
        self.parse_and_filter(decoded, None, callsign, SnapshotSource::HamCallHci)
    }

    fn decode_hci_posting_record_into(&self, dat_offset: u64, out: &mut Vec<u8>) {
        out.clear();
        if let Some(bounds) = self.record_bounds_for_hci_posting(dat_offset) {
            self.decode_bounds_into(bounds.start, bounds.end, out);
        }
    }

    fn decode_bounds_into(&self, start: usize, end: usize, out: &mut Vec<u8>) {
        let raw = &self.dat_mmap[start..end];
        let phase = crate::v2_dat::phase_for_dat_offset(start as u64);
        crate::v2_dat::decode_phase_into(start as u64, raw, phase, out);
    }

    fn record_bounds_for_hci_posting(&self, dat_offset: u64) -> Option<RecordBoundsTrace> {
        self.record_bounds_from_hci_posting_start(dat_offset)
            .or_else(|| record_bounds_containing_offset_in(&self.dat_mmap, dat_offset))
    }

    fn record_bounds_from_hci_posting_start(&self, dat_offset: u64) -> Option<RecordBoundsTrace> {
        record_bounds_from_hci_posting_start_in(&self.dat_mmap, dat_offset)
    }

    fn decode_from_offset_into(&self, dat_offset: u64, max_len: usize, out: &mut Vec<u8>) {
        out.clear();
        let start = dat_offset as usize;
        if start >= self.dat_mmap.len() {
            return;
        }
        let end = start.saturating_add(max_len).min(self.dat_mmap.len());
        let raw = &self.dat_mmap[start..end];
        let phase = crate::v2_dat::phase_for_dat_offset(dat_offset);
        crate::v2_dat::decode_phase_into(dat_offset, raw, phase, out);
    }

    fn parse_and_filter(
        &self,
        decoded: &[u8],
        default_key: Option<&[u8]>,
        callsign: &str,
        source: SnapshotSource,
    ) -> Vec<CallSnapshot> {
        crate::v2_record::parse_matching_snapshots(
            decoded,
            default_key,
            callsign,
            source,
            self.countries.as_ref(),
            self.interests.as_ref(),
        )
    }

    /// Compute the raw byte slice for an IDX entry by finding the next
    /// entry's offset and slicing `[offset, next_offset)` out of the
    /// DAT mmap. Falls back to slicing to end-of-file if there is no
    /// next entry.
    fn raw_record_for<'a>(&'a self, entry: TextEntry<'a>) -> Option<RawV2Record<'a>> {
        let dat_size = self.dat_mmap.len() as u64;
        // The IDX is sorted; advance one step past `entry.key` and read
        // the offset of whatever entry follows.
        let next = self.next_entry_after_key(entry.key);
        let next_off = next.map(|e| e.dat_offset).unwrap_or(dat_size);
        // Clamp to file size — the trailing sentinel `ZZZZZZZZ` is at
        // file_size + 5, so a naive slice would go past the mmap end.
        let next_off = next_off.min(dat_size);
        if entry.dat_offset >= dat_size {
            return None;
        }
        let start = entry.dat_offset as usize;
        let end = next_off as usize;
        if end < start {
            return None;
        }
        let raw_bytes = &self.dat_mmap[start..end];
        Some(RawV2Record {
            key: entry.key,
            dat_offset: entry.dat_offset,
            raw_len: end - start,
            raw_bytes,
        })
    }

    /// Find the first IDX entry whose key sorts strictly after `key`.
    fn next_entry_after_key(&self, key: &[u8]) -> Option<TextEntry<'_>> {
        self.idx.next_entry_after_key(key)
    }
}

fn record_bounds_containing_offset_in(
    encoded: &[u8],
    dat_offset: u64,
) -> Option<RecordBoundsTrace> {
    let mut position = usize::try_from(dat_offset).ok()?;
    if position >= encoded.len() {
        return None;
    }
    let original = position;
    while position > 0 && decode_dat_byte(position as u64, encoded[position]) != 0xb5 {
        position -= 1;
    }
    if decode_dat_byte(position as u64, encoded[position]) != 0xb5 {
        return None;
    }
    let start = position;
    position += 1;
    while position < encoded.len() && decode_dat_byte(position as u64, encoded[position]) != 0xb5 {
        position += 1;
    }
    Some(RecordBoundsTrace {
        start,
        end: position,
        source: RecordBoundarySource::ScanFallback,
        backward_scan_bytes: Some(original - start),
        forward_scan_bytes: Some(position.saturating_sub(original)),
    })
}

fn record_bounds_from_hci_posting_start_in(
    encoded: &[u8],
    dat_offset: u64,
) -> Option<RecordBoundsTrace> {
    let start = usize::try_from(dat_offset).ok()?;
    if start >= encoded.len() || decode_dat_byte(dat_offset, encoded[start]) != 0xb5 {
        return None;
    }
    let mut end = start + 1;
    while end < encoded.len() && decode_dat_byte(end as u64, encoded[end]) != 0xb5 {
        end += 1;
    }
    Some(RecordBoundsTrace {
        start,
        end,
        source: RecordBoundarySource::HciPostingStart,
        backward_scan_bytes: None,
        forward_scan_bytes: None,
    })
}

fn push_hci_posting_start_sample(
    report: &mut HciPostingStartInvariant,
    key: &[u8],
    posting: HciPosting,
    decoded_byte: Option<u8>,
) {
    if report.samples.len() >= 20 {
        return;
    }
    report.samples.push(HciPostingStartInvariantSample {
        hci_key: String::from_utf8_lossy(key).into_owned(),
        dat_offset: posting.dat_offset,
        position: posting.position,
        decoded_byte,
    });
}

/// Return a lowercase hexadecimal prefix for diagnostics.
#[must_use]
pub fn trace_hex_prefix(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Return a printable ASCII text prefix for diagnostics.
#[must_use]
pub fn trace_text_prefix(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

fn parsed_snapshot_trace(snapshots: &[CallSnapshot]) -> ParsedSnapshotTrace {
    ParsedSnapshotTrace {
        count: snapshots.len(),
        callsigns: snapshots
            .iter()
            .map(|snapshot| snapshot.callsign.clone())
            .collect(),
        vintages: snapshots.iter().map(|snapshot| snapshot.vintage).collect(),
    }
}

fn decoded_header_text(bytes: &[u8]) -> String {
    let header = bytes
        .iter()
        .take_while(|byte| **byte != 0xb5 && **byte <= 0xb4)
        .copied()
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&header).into_owned()
}

fn posting_is_in_record(bytes: &[u8], posting: HciPosting) -> bool {
    let usable = bytes.len().saturating_sub(1);
    bytes[..usable].chunks_exact(5).any(|chunk| {
        let offset = u32::from_be_bytes(chunk[0..4].try_into().expect("chunk len")) as u64;
        offset == posting.dat_offset && chunk[4] == posting.position
    })
}

fn visit_hci_lookup_keys(callsign: &str, years: &[u16], mut visit: impl FnMut(&[u8])) {
    let call = callsign.trim().to_ascii_uppercase();
    let suffix = if call.len() > 1 { &call[1..] } else { &call };
    visit(call.as_bytes());
    if suffix != call {
        visit(suffix.as_bytes());
    }
    for year in years {
        let mut key = [0u8; 32];
        let Some(key) = hci_archive_key(&call, *year, &mut key) else {
            continue;
        };
        visit(key);
        if suffix != call {
            let mut key = [0u8; 32];
            let Some(key) = hci_archive_key(suffix, *year, &mut key) else {
                continue;
            };
            visit(key);
        }
    }
}

fn stat_key_parts(key: &[u8]) -> Option<(String, Option<u16>)> {
    let key = std::str::from_utf8(key).ok()?.trim();
    if key.is_empty() || key == "!!!" || key == "ZZZZZZZZ" {
        return None;
    }
    let (callsign, vintage) = if let Some((callsign, year)) = key.split_once(':') {
        if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let year = year.parse::<u16>().ok()?;
        if !(1900..=2099).contains(&year) {
            return None;
        }
        (callsign, Some(year))
    } else {
        (key, None)
    };
    Callsign::parse(callsign).ok()?;
    Some((callsign.to_owned(), vintage))
}

fn hci_archive_key<'a>(call: &str, year: u16, buf: &'a mut [u8; 32]) -> Option<&'a [u8]> {
    let call = call.as_bytes();
    let len = call.len().checked_add(5)?;
    if len > buf.len() {
        return None;
    }
    buf[..call.len()].copy_from_slice(call);
    buf[call.len()] = b':';
    let digits = &mut buf[call.len() + 1..len];
    digits[0] = b'0' + ((year / 1000) % 10) as u8;
    digits[1] = b'0' + ((year / 100) % 10) as u8;
    digits[2] = b'0' + ((year / 10) % 10) as u8;
    digits[3] = b'0' + (year % 10) as u8;
    Some(&buf[..len])
}

struct InterestScan<'a> {
    table: Option<&'a InterestTable>,
    key: Vec<u8>,
    interest_raw: Vec<u8>,
    current_tag: Option<u8>,
    in_record: bool,
    key_complete: bool,
    current_calls: BTreeSet<String>,
    current_calls_resolved: BTreeSet<String>,
    current_snapshots: usize,
    archive_snapshots: usize,
    unknown_codes: BTreeMap<String, UnknownInterestAccumulator>,
}

#[derive(Default)]
struct UnknownInterestAccumulator {
    occurrences: usize,
    current_occurrences: usize,
    archive_occurrences: usize,
    examples: Vec<InterestCodeExample>,
}

fn normalize_interest_code(code: &str) -> Result<String> {
    let code = code.trim();
    if code.len() == 4 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(code.to_owned())
    } else {
        Err(Error::InvalidInterestCode(code.to_owned()))
    }
}

fn scan_interest_code(encoded: &[u8], target: &str) -> Vec<InterestSearchMatch> {
    let mut scan = InterestSearchScan::new(target);
    scan.scan_dat(encoded);
    scan.finish()
}

struct InterestSearchScan<'a> {
    target: &'a str,
    key: Vec<u8>,
    interest_raw: Vec<u8>,
    current_tag: Option<u8>,
    in_record: bool,
    key_complete: bool,
    matches: Vec<InterestSearchMatch>,
}

impl<'a> InterestSearchScan<'a> {
    fn new(target: &'a str) -> Self {
        Self {
            target,
            key: Vec::with_capacity(16),
            interest_raw: Vec::with_capacity(64),
            current_tag: None,
            in_record: false,
            key_complete: false,
            matches: Vec::new(),
        }
    }

    fn scan_dat(&mut self, encoded: &[u8]) {
        for (offset, byte) in encoded.iter().copied().enumerate() {
            let decoded = decode_dat_byte(offset as u64, byte);
            match decoded {
                0xb5 => {
                    self.finish_record();
                    self.start_record();
                }
                0xb6..=0xdf if self.in_record => {
                    self.key_complete = true;
                    self.current_tag = Some(decoded);
                    if decoded == 0xd5 {
                        self.interest_raw.clear();
                    }
                }
                byte if self.in_record && !self.key_complete => self.key.push(byte),
                byte if self.in_record && self.current_tag == Some(0xd5) => {
                    self.interest_raw.push(byte);
                }
                _ => {}
            }
        }
        self.finish_record();
    }

    fn finish(mut self) -> Vec<InterestSearchMatch> {
        self.matches.sort_by(|left, right| {
            left.callsign
                .cmp(&right.callsign)
                .then(left.vintage.cmp(&right.vintage))
        });
        self.matches.dedup();
        self.matches
    }

    fn start_record(&mut self) {
        self.in_record = true;
        self.key_complete = false;
        self.current_tag = None;
        self.key.clear();
        self.interest_raw.clear();
    }

    fn finish_record(&mut self) {
        if !self.in_record || self.interest_raw.is_empty() {
            return;
        }
        let Some((callsign, vintage)) = interest_record_key(&self.key) else {
            return;
        };
        let raw = trim_ascii_lossy(&self.interest_raw);
        if InterestTable::codes(&raw)
            .iter()
            .any(|code| code == self.target)
        {
            self.matches.push(InterestSearchMatch { callsign, vintage });
        }
    }
}

impl<'a> InterestScan<'a> {
    fn new(table: Option<&'a InterestTable>) -> Self {
        Self {
            table,
            key: Vec::with_capacity(16),
            interest_raw: Vec::with_capacity(64),
            current_tag: None,
            in_record: false,
            key_complete: false,
            current_calls: BTreeSet::new(),
            current_calls_resolved: BTreeSet::new(),
            current_snapshots: 0,
            archive_snapshots: 0,
            unknown_codes: BTreeMap::new(),
        }
    }

    fn scan_dat(&mut self, encoded: &[u8]) {
        for (offset, byte) in encoded.iter().copied().enumerate() {
            let decoded = decode_dat_byte(offset as u64, byte);
            match decoded {
                0xb5 => {
                    self.finish_record();
                    self.start_record();
                }
                0xb6..=0xdf if self.in_record => {
                    self.key_complete = true;
                    self.current_tag = Some(decoded);
                    if decoded == 0xd5 {
                        self.interest_raw.clear();
                    }
                }
                byte if self.in_record && !self.key_complete => self.key.push(byte),
                byte if self.in_record && self.current_tag == Some(0xd5) => {
                    self.interest_raw.push(byte);
                }
                _ => {}
            }
        }
        self.finish_record();
    }

    fn finish(self, catalog_entries: usize) -> InterestStatistics {
        InterestStatistics {
            catalog_entries,
            current_callsigns_with_interests: self.current_calls.len(),
            current_snapshots_with_interests: self.current_snapshots,
            archive_snapshots_with_interests: self.archive_snapshots,
            current_callsigns_with_resolved_interests: self.current_calls_resolved.len(),
            unknown_codes: self
                .unknown_codes
                .into_iter()
                .map(|(code, acc)| UnknownInterestCode {
                    code,
                    occurrences: acc.occurrences,
                    current_occurrences: acc.current_occurrences,
                    archive_occurrences: acc.archive_occurrences,
                    examples: acc.examples,
                })
                .collect(),
        }
    }

    fn start_record(&mut self) {
        self.in_record = true;
        self.key_complete = false;
        self.current_tag = None;
        self.key.clear();
        self.interest_raw.clear();
    }

    fn finish_record(&mut self) {
        if !self.in_record || self.interest_raw.is_empty() {
            return;
        }
        let Some((callsign, vintage)) = interest_record_key(&self.key) else {
            return;
        };
        let raw = trim_ascii_lossy(&self.interest_raw);
        if raw.is_empty() {
            return;
        }
        let codes = InterestTable::codes(&raw);
        if codes.is_empty() {
            return;
        }
        let resolved = self
            .table
            .map_or_else(Vec::new, |table| table.resolve_raw(&raw));
        if vintage.is_none() {
            self.current_snapshots += 1;
            self.current_calls.insert(callsign.clone());
            if !resolved.is_empty() {
                self.current_calls_resolved.insert(callsign.clone());
            }
        } else {
            self.archive_snapshots += 1;
        }
        for code in codes {
            if !resolved.iter().any(|interest| interest.code == code) {
                let acc = self.unknown_codes.entry(code).or_default();
                acc.occurrences += 1;
                if vintage.is_none() {
                    acc.current_occurrences += 1;
                } else {
                    acc.archive_occurrences += 1;
                }
                if acc.examples.len() < 3
                    && !acc
                        .examples
                        .iter()
                        .any(|item| item.callsign == callsign && item.vintage == vintage)
                {
                    acc.examples.push(InterestCodeExample {
                        callsign: callsign.clone(),
                        vintage,
                    });
                }
            }
        }
    }
}

#[derive(Default)]
struct TagScan {
    key: Vec<u8>,
    value: Vec<u8>,
    current_tag: Option<u8>,
    in_record: bool,
    key_complete: bool,
    vintage: Option<u16>,
    tags: BTreeMap<u8, TagAccumulator>,
}

#[derive(Default)]
struct TagAccumulator {
    occurrences: usize,
    current_occurrences: usize,
    archive_occurrences: usize,
    sample_values: Vec<String>,
}

impl TagScan {
    fn scan_dat(&mut self, encoded: &[u8]) {
        for (offset, byte) in encoded.iter().copied().enumerate() {
            let decoded = decode_dat_byte(offset as u64, byte);
            match decoded {
                0xb5 => {
                    self.finish_value();
                    self.start_record();
                }
                0xb6..=0xdf if self.in_record => {
                    self.finish_value();
                    self.key_complete = true;
                    self.current_tag = Some(decoded);
                    self.value.clear();
                }
                byte if self.in_record && !self.key_complete => self.key.push(byte),
                byte if self.in_record && self.current_tag.is_some() => self.value.push(byte),
                _ => {}
            }
        }
        self.finish_value();
    }

    fn finish(self) -> ModernTagStatistics {
        ModernTagStatistics {
            tags: self
                .tags
                .into_iter()
                .map(|(tag, acc)| ModernTagCount {
                    tag,
                    field_name: modern_tag_name(tag),
                    occurrences: acc.occurrences,
                    current_occurrences: acc.current_occurrences,
                    archive_occurrences: acc.archive_occurrences,
                    sample_values: acc.sample_values,
                })
                .collect(),
        }
    }

    fn start_record(&mut self) {
        self.in_record = true;
        self.key_complete = false;
        self.current_tag = None;
        self.vintage = None;
        self.key.clear();
        self.value.clear();
    }

    fn finish_value(&mut self) {
        let Some(tag) = self.current_tag.take() else {
            if self.in_record && !self.key_complete {
                self.vintage = interest_record_key(&self.key).and_then(|(_, vintage)| vintage);
            }
            return;
        };
        if self.vintage.is_none() {
            self.vintage = interest_record_key(&self.key).and_then(|(_, vintage)| vintage);
        }
        let value = trim_ascii_lossy(&self.value);
        if value.is_empty() {
            return;
        }
        let acc = self.tags.entry(tag).or_default();
        acc.occurrences += 1;
        if self.vintage.is_none() {
            acc.current_occurrences += 1;
        } else {
            acc.archive_occurrences += 1;
        }
        if acc.sample_values.len() < 3 && !acc.sample_values.contains(&value) {
            acc.sample_values.push(value);
        }
    }
}

fn decode_dat_byte(offset: u64, encoded: u8) -> u8 {
    let stream_key = ((offset + 4) % 101) as u8;
    (encoded ^ 7) ^ stream_key
}

fn interest_record_key(bytes: &[u8]) -> Option<(String, Option<u16>)> {
    let key = trim_ascii_lossy(bytes);
    let (callsign, vintage) = if let Some((callsign, year)) = key.split_once(':') {
        let year = year.parse::<u16>().ok()?;
        (callsign, Some(year))
    } else {
        (key.as_str(), None)
    };
    Callsign::parse(callsign).ok()?;
    Some((callsign.to_owned(), vintage))
}

fn trim_ascii_lossy(bytes: &[u8]) -> String {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace() && *b != 0)
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace() && *b != 0)
        .map(|index| index + 1)
        .unwrap_or(start);
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}

fn modern_tag_name(tag: u8) -> Option<&'static str> {
    Some(match tag {
        0xb6 => "license_class",
        0xb7 => "record_code",
        0xb8 => "first_name",
        0xb9 => "middle_name",
        0xba => "last_name",
        0xbb => "suffix",
        0xbc..=0xbe => "address",
        0xbf => "city",
        0xc0 => "state_or_province",
        0xc1 => "postal_code",
        0xc2 => "birth_date",
        0xc3 => "first_issued",
        0xc4 => "expires",
        0xc5 => "last_changed",
        0xc6 => "county",
        0xc7 => "gmt_offset",
        0xc8 => "latitude",
        0xc9 => "longitude",
        0xca => "grid",
        0xcb => "area_code",
        0xcc => "previous_call",
        0xcd => "previous_class",
        0xce => "fcc_transaction_type",
        0xcf => "email",
        0xd0 => "qsl",
        0xd1 => "country",
        0xd2 => "url",
        0xd4 => "fax_number",
        0xd5 => "interest_codes",
        0xd7 => "license_id",
        0xd9 => "frn",
        0xda => "iota",
        0xde => "numeric_id",
        _ => return None,
    })
}

impl AssetKind {
    fn cmp_rank(self) -> u8 {
        match self {
            Self::Biography => 0,
            Self::Photo => 1,
            Self::Flag => 2,
            Self::Map => 3,
            Self::SidecarData => 4,
        }
    }
}

fn collect_callsign_assets(
    root: &Path,
    kind: AssetKind,
    callsign: &str,
    out: &mut Vec<AssetMetadata>,
) {
    let Ok(children) = fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_callsign_assets(&path, kind, callsign, out);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        let stem = stem.to_ascii_uppercase();
        let matches = match kind {
            AssetKind::Biography => stem == callsign,
            AssetKind::Photo => stem == callsign || stem.starts_with(callsign),
            AssetKind::Flag | AssetKind::Map | AssetKind::SidecarData => false,
        };
        if matches {
            out.push(asset_metadata(kind, callsign, path));
        }
    }
}

fn collect_manifest_photo_fallback(
    root: &Path,
    callsign: &str,
    relative: &Path,
    out: &mut Vec<AssetMetadata>,
) {
    let components = relative.components().collect::<Vec<_>>();
    if components.len() == 1 {
        let Some(file_name) = relative.file_name().and_then(OsStr::to_str) else {
            return;
        };
        collect_named_photo_assets(root, callsign, file_name, out);
        return;
    }
    collect_relative_suffix_photo_assets(root, callsign, relative, out);
}

fn manifest_photo_exists(root: &Path, callsign: &str, file: &str) -> bool {
    let Some(relative) = safe_manifest_photo_path(file) else {
        return false;
    };
    if is_regular_file_no_symlink(&root.join(&relative)) {
        return true;
    }
    let mut assets = Vec::new();
    collect_manifest_photo_fallback(root, callsign, &relative, &mut assets);
    !assets.is_empty()
}

fn collect_named_photo_assets(
    root: &Path,
    callsign: &str,
    file_name: &str,
    out: &mut Vec<AssetMetadata>,
) {
    let Ok(children) = fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_named_photo_assets(&path, callsign, file_name, out);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if name.eq_ignore_ascii_case(file_name) {
            out.push(asset_metadata(AssetKind::Photo, callsign, path));
        }
    }
}

fn collect_relative_suffix_photo_assets(
    root: &Path,
    callsign: &str,
    relative: &Path,
    out: &mut Vec<AssetMetadata>,
) {
    let Ok(children) = fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_relative_suffix_photo_assets(&path, callsign, relative, out);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if path_ends_with_components_ignore_ascii_case(&path, relative) {
            out.push(asset_metadata(AssetKind::Photo, callsign, path));
        }
    }
}

fn path_ends_with_components_ignore_ascii_case(path: &Path, suffix: &Path) -> bool {
    let path_components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_ascii_lowercase),
            _ => None,
        })
        .collect::<Vec<_>>();
    let suffix_components = suffix
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_ascii_lowercase),
            _ => None,
        })
        .collect::<Vec<_>>();
    !suffix_components.is_empty() && path_components.ends_with(&suffix_components)
}

fn safe_manifest_photo_path(file: &str) -> Option<PathBuf> {
    let path = Path::new(file);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn collect_country_assets(root: &Path, kind: AssetKind, code: &str, out: &mut Vec<AssetMetadata>) {
    let Ok(children) = fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        let stem = stem
            .split_once('-')
            .map_or(stem, |(prefix, _)| prefix)
            .to_ascii_uppercase();
        if stem == code.to_ascii_uppercase() {
            out.push(asset_metadata(kind, code, path));
        }
    }
}

fn is_regular_file_no_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn dedup_assets_by_path(assets: &mut Vec<AssetRef>) {
    let mut seen = BTreeSet::new();
    assets.retain(|asset| seen.insert(asset.path.clone()));
}

fn best_mappable_snapshot(report: &LookupResult) -> Option<CallSnapshot> {
    report
        .current
        .as_ref()
        .filter(|snapshot| snapshot_location(snapshot).is_some())
        .cloned()
        .or_else(|| {
            report
                .history
                .iter()
                .rev()
                .find(|snapshot| snapshot_location(snapshot).is_some())
                .cloned()
        })
}

fn snapshot_location(snapshot: &CallSnapshot) -> Option<GeoPoint> {
    let lat = snapshot.latitude.as_deref()?.trim().parse::<f64>().ok()?;
    let lon = snapshot.longitude.as_deref()?.trim().parse::<f64>().ok()?;
    (lat.is_finite() && lon.is_finite()).then_some(GeoPoint { lon, lat })
}

fn enrich_country_info(
    mut info: CountryInfo,
    country_names: Option<&CountryNameTable>,
) -> CountryInfo {
    if info.latitude.is_none() || info.longitude.is_none() {
        if let Some(centroid) = country_names.and_then(|table| table.lookup(&info.name)) {
            info.latitude.get_or_insert(centroid.location.lat);
            info.longitude.get_or_insert(centroid.location.lon);
        }
    }
    info
}

fn render_boundary_segments(
    svg: &mut String,
    segments: &[BoundarySegment],
    limit: Option<usize>,
    stroke: &str,
    width: f64,
) {
    let segments: Box<dyn Iterator<Item = &BoundarySegment> + '_> = if let Some(limit) = limit {
        Box::new(segments.iter().take(limit))
    } else {
        Box::new(segments.iter())
    };
    for segment in segments {
        render_polyline(svg, &segment.points, stroke, width);
    }
}

fn render_us_county_segments(
    svg: &mut String,
    dataset: &UsCountyBoundaryDataset,
    limit: Option<usize>,
) {
    let counties: Box<dyn Iterator<Item = &crate::sidecar_impl::UsCountyBoundary> + '_> =
        if let Some(limit) = limit {
            Box::new(dataset.counties.iter().take(limit))
        } else {
            Box::new(dataset.counties.iter())
        };
    for county in counties {
        render_polyline(svg, &county.points, "#bbb", 0.12);
    }
}

fn render_state_vector_segments(
    svg: &mut String,
    segments: &[crate::sidecar_impl::StateVectorSegment],
    limit: Option<usize>,
) {
    let segments: Box<dyn Iterator<Item = &crate::sidecar_impl::StateVectorSegment> + '_> =
        if let Some(limit) = limit {
            Box::new(segments.iter().take(limit))
        } else {
            Box::new(segments.iter())
        };
    for segment in segments {
        render_polyline(svg, &segment.points, "#567", 0.18);
    }
}

fn render_polyline(svg: &mut String, points: &[GeoPoint], stroke: &str, width: f64) {
    if points.len() < 2 {
        return;
    }
    svg.push_str(&format!(
        "<polyline fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{width:.2}\" points=\""
    ));
    for point in points {
        svg.push_str(&format!("{:.6},{:.6} ", point.lon, -point.lat));
    }
    svg.push_str("\"/>");
}

fn asset_metadata(kind: AssetKind, key: &str, path: PathBuf) -> AssetMetadata {
    let media_type = match path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("txt") => "text/plain",
        Some("zip") => "application/zip",
        Some("dat") => "application/octet-stream",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    AssetMetadata {
        kind,
        key: key.to_owned(),
        media_type: media_type.to_owned(),
        path,
    }
}

fn photo_manifest_rank(
    asset: &AssetRef,
    manifest: &[&crate::sidecar_impl::PhotoManifestEntry],
) -> usize {
    let Some(file_name) = asset.path.file_name().and_then(OsStr::to_str) else {
        return usize::MAX;
    };
    manifest
        .iter()
        .position(|entry| {
            entry.file.as_deref().is_some_and(|file| {
                Path::new(file)
                    .file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|manifest_name| manifest_name.eq_ignore_ascii_case(file_name))
            })
        })
        .unwrap_or(usize::MAX)
}

fn merge_snapshot(
    current: &mut Option<CallSnapshot>,
    history: &mut Vec<CallSnapshot>,
    snapshot: CallSnapshot,
) {
    if snapshot.vintage.is_none() {
        if let Some(existing) = current {
            existing.merge_missing_from(snapshot);
        } else {
            *current = Some(snapshot);
        }
        return;
    }

    if let Some(existing) = history
        .iter_mut()
        .find(|item| item.callsign == snapshot.callsign && item.vintage == snapshot.vintage)
    {
        existing.merge_missing_from(snapshot);
    } else {
        history.push(snapshot);
    }
}

/// Open the 2025-format shard if `supplied` looks like (or contains) a
/// `ham0/` directory with `hamcall.idx` + `hamcall.dat`. Returns
/// `Ok(None)` when the layout isn't present — that's not an error.
fn open_v2(supplied: &Path) -> Result<Option<V2Shard>> {
    let Some(dir) = resolve_v2_dir(supplied) else {
        return Ok(None);
    };
    let idx_path = dir.join(v2::IDX_NAME);
    let dat_path = dir.join(v2::DAT_NAME);
    if !idx_path.is_file() || !dat_path.is_file() {
        return Ok(None);
    }
    let idx = TextIdxFile::open(&idx_path)?;
    let file = File::open(&dat_path)?;
    // SAFETY: read-only view of a stable on-disk database.
    let dat_mmap = Arc::new(unsafe { Mmap::map(&file)? });
    let hci = open_hci(&dir)?;
    let us_csv = open_us_csv(&dir)?;
    let countries = open_countries(&dir)?;
    let pc_countries = open_pc_countries(&dir)?;
    let country_names = open_country_names(&dir)?;
    let interests = open_interests(&dir)?;
    Ok(Some(V2Shard {
        dir,
        idx,
        dat_path,
        dat_mmap,
        hci,
        us_csv,
        countries,
        pc_countries,
        country_names,
        interests,
    }))
}

fn open_hci(dir: &Path) -> Result<Option<HciFile>> {
    let index_path = dir.join(v2::HCI_INDEX_NAME);
    let dat_path = dir.join(v2::HCI_DAT_NAME);
    if !index_path.is_file() || !dat_path.is_file() {
        return Ok(None);
    }
    HciFile::open(index_path, dat_path).map(Some)
}

fn open_us_csv(dir: &Path) -> Result<Option<UsCsvFile>> {
    let path = dir.join(v2::US_CSV_ZIP_NAME);
    if !path.is_file() {
        return Ok(None);
    }
    UsCsvFile::open(path).map(Some)
}

fn open_countries(dir: &Path) -> Result<Option<CountryTable>> {
    let path = dir.join("countrys");
    if path.is_file() {
        return CountryTable::open(path).map(Some);
    }
    let path = dir.join("gcmcountrys");
    if path.is_file() {
        return CountryTable::open(path).map(Some);
    }
    Ok(None)
}

fn open_pc_countries(dir: &Path) -> Result<Option<CountryTable>> {
    let path = dir.join("COUNTRYS.PC");
    if !path.is_file() {
        return Ok(None);
    }
    CountryTable::open(path).map(Some)
}

fn open_country_names(dir: &Path) -> Result<Option<CountryNameTable>> {
    let path = dir.join("countrys.nam");
    if !path.is_file() {
        return Ok(None);
    }
    CountryNameTable::open(path).map(Some)
}

fn open_interests(dir: &Path) -> Result<Option<InterestTable>> {
    let path = dir.join("interest");
    if !path.is_file() {
        return Ok(None);
    }
    InterestTable::open(path).map(Some)
}

/// Find the `ham0/` directory: either `supplied` is it, or it contains
/// it. Case-insensitive.
fn resolve_v2_dir(supplied: &Path) -> Option<PathBuf> {
    if supplied
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case(v2::DATA_DIR))
        .unwrap_or(false)
    {
        return Some(supplied.to_owned());
    }
    for variant in [v2::DATA_DIR, &v2::DATA_DIR.to_ascii_uppercase()] {
        let candidate = supplied.join(variant);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    fn encode_dat_byte(offset: u64, decoded: u8) -> u8 {
        let stream_key = ((offset + 4) % 101) as u8;
        (decoded ^ stream_key) ^ 7
    }

    #[test]
    fn public_database_handles_are_send_sync() {
        assert_send_sync::<CallBook>();
        assert_send_sync::<CallsignEntry<'static>>();
        assert_send_sync::<Diagnostics<'static>>();
        assert_send_sync::<BatchLookup<'static>>();
        assert_send_sync::<Entries<'static>>();
        assert_send_sync::<StationMap<'static>>();
        assert_send_sync::<MapLayers<'static>>();
        assert_send_sync::<AssetCatalog<'static>>();
        assert_send_sync::<CountryCatalog<'static>>();
    }

    #[test]
    fn trace_previews_are_bounded_and_printable() {
        let bytes = b"AB\x00\xB5xyz";

        assert_eq!(trace_hex_prefix(bytes, 4), "414200b5");
        assert_eq!(trace_text_prefix(bytes, 6), "AB..xy");
    }

    #[test]
    fn record_bound_trace_reports_sentinel_scan_distances() {
        let decoded = [0xb5, b'A', b'B', b'C', 0xb5, b'D'];
        let encoded = decoded
            .iter()
            .enumerate()
            .map(|(offset, byte)| encode_dat_byte(offset as u64, *byte))
            .collect::<Vec<_>>();

        let bounds = record_bounds_containing_offset_in(&encoded, 2).unwrap();

        assert_eq!(bounds.start, 0);
        assert_eq!(bounds.end, 4);
        assert_eq!(bounds.source, RecordBoundarySource::ScanFallback);
        assert_eq!(bounds.backward_scan_bytes, Some(2));
        assert_eq!(bounds.forward_scan_bytes, Some(2));
    }

    #[test]
    fn hci_posting_start_bounds_decode_forward_only() {
        let decoded = [0xb5, b'A', b'B', 0xb5, b'C'];
        let encoded = decoded
            .iter()
            .enumerate()
            .map(|(offset, byte)| encode_dat_byte(offset as u64, *byte))
            .collect::<Vec<_>>();

        let first = record_bounds_from_hci_posting_start_in(&encoded, 0).unwrap();
        assert_eq!((first.start, first.end), (0, 3));
        assert_eq!(first.source, RecordBoundarySource::HciPostingStart);
        assert_eq!(first.backward_scan_bytes, None);
        assert_eq!(first.forward_scan_bytes, None);

        let last = record_bounds_from_hci_posting_start_in(&encoded, 3).unwrap();
        assert_eq!((last.start, last.end), (3, 5));
        assert!(record_bounds_from_hci_posting_start_in(&encoded, 1).is_none());
        assert!(record_bounds_from_hci_posting_start_in(&encoded, 5).is_none());
    }
}
