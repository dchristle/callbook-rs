//! Readers for `ham0/` sidecar files.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::error::Result;

const COUNT_RECORD_LEN: usize = 33;
const COUNT_KEY_LEN: usize = 15;
const COUNT_FIELD_END: usize = 22;
const COUNT_DATE_END: usize = 30;

/// One web lookup-count record from `counts.dat`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LookupCountRecord {
    /// Callsign key. Archive records may include `:YYYY` when it fits.
    pub key: String,
    /// Displayed HamCall.net lookup count.
    pub count: u32,
    /// Last count-update date as `YYYYMMDD`, when present.
    pub updated_yyyymmdd: Option<u32>,
    /// Trailing status byte from the source record.
    pub status: Option<char>,
}

/// Random-access reader for encoded `ham0/counts.dat`.
pub struct LookupCountFile {
    path: PathBuf,
    mmap: Mmap,
    records: usize,
}

/// Cached catalog view over the searchable records in `ham0/counts.dat`.
pub struct LookupCounts {
    file: LookupCountFile,
}

/// Iterator over parsed lookup-count records in the searchable table.
pub struct LookupCountIter<'a> {
    file: &'a LookupCountFile,
    next: usize,
    end: usize,
}

impl LookupCountFile {
    /// Open `ham0/counts.dat`.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let file = File::open(&path)?;
        let mmap = {
            // SAFETY: read-only view of a stable on-disk sidecar file.
            unsafe { Mmap::map(&file)? }
        };
        Ok(Self {
            records: mmap.len() / COUNT_RECORD_LEN,
            path,
            mmap,
        })
    }

    /// Source file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decode one fixed-width slot by ordinal.
    #[must_use]
    pub fn record(&self, index: usize) -> Option<LookupCountRecord> {
        let raw = self.decoded_slot(index)?;
        parse_count_record(&raw)
    }

    /// Binary-search lookup by current callsign key.
    #[must_use]
    pub fn lookup(&self, callsign: &str) -> Option<LookupCountRecord> {
        let key = callsign.trim().to_ascii_uppercase();
        if key.is_empty() {
            return None;
        }
        let (mut low, mut high) = self.searchable_record_range();
        while low < high {
            let mid = low + ((high - low) / 2);
            let Some(record_key) = self.decoded_key(mid) else {
                low = mid + 1;
                continue;
            };
            match record_key.as_str().cmp(&key) {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => return self.record(mid),
            }
        }
        None
    }

    fn searchable_record_range(&self) -> (usize, usize) {
        if self.has_prologue_and_sentinels() {
            (3, self.records - 1)
        } else {
            (0, self.records)
        }
    }

    fn has_prologue_and_sentinels(&self) -> bool {
        self.records >= 4
            && self.decoded_key(0).is_none()
            && self
                .decoded_key(2)
                .as_deref()
                .is_some_and(|key| repeated_key(key, b'!'))
            && self
                .decoded_key(self.records - 1)
                .as_deref()
                .is_some_and(|key| repeated_key(key, b'z'))
    }

    fn decoded_slot(&self, index: usize) -> Option<[u8; COUNT_RECORD_LEN]> {
        if index >= self.records {
            return None;
        }
        let start = index * COUNT_RECORD_LEN;
        let raw = &self.mmap[start..start + COUNT_RECORD_LEN];
        let mut out = [0u8; COUNT_RECORD_LEN];
        for (i, byte) in raw.iter().copied().enumerate() {
            out[i] = decode_v2_sidecar_byte((start + i) as u64, byte);
        }
        Some(out)
    }

    fn decoded_key(&self, index: usize) -> Option<String> {
        let record = self.decoded_slot(index)?;
        let key = trim_ascii_lossy(&record[..COUNT_KEY_LEN]);
        (!key.is_empty()).then_some(key)
    }
}

impl LookupCounts {
    /// Open `ham0/counts.dat` as a catalog for repeated lookups and iteration.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            file: LookupCountFile::open(path)?,
        })
    }

    /// Source file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// Number of searchable sorted-table slots.
    #[must_use]
    pub fn len(&self) -> usize {
        let (start, end) = self.file.searchable_record_range();
        end.saturating_sub(start)
    }

    /// Whether the searchable sorted table contains no slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Binary-search lookup by current callsign key.
    #[must_use]
    pub fn lookup(&self, callsign: &str) -> Option<LookupCountRecord> {
        self.file.lookup(callsign)
    }

    /// Iterate over parsed records in the searchable sorted table.
    #[must_use]
    pub fn iter(&self) -> LookupCountIter<'_> {
        let (next, end) = self.file.searchable_record_range();
        LookupCountIter {
            file: &self.file,
            next,
            end,
        }
    }
}

impl Iterator for LookupCountIter<'_> {
    type Item = LookupCountRecord;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.end {
            let index = self.next;
            self.next += 1;
            if let Some(record) = self.file.record(index) {
                return Some(record);
            }
        }
        None
    }
}

/// Bounding box for a map boundary segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryBox {
    /// Minimum longitude.
    pub min_lon: f64,
    /// Maximum longitude.
    pub max_lon: f64,
    /// Minimum latitude.
    pub min_lat: f64,
    /// Maximum latitude.
    pub max_lat: f64,
}

/// One longitude/latitude coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    /// Longitude in decimal degrees.
    pub lon: f64,
    /// Latitude in decimal degrees.
    pub lat: f64,
}

/// One boundary segment from `wc.dat` or `cb.dat`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BoundarySegment {
    /// Effective segment bounds, expanded when needed to include parsed points.
    pub bounds: BoundaryBox,
    /// Byte offset of the next segment header in the source file.
    pub next_offset: u64,
    /// Segment points.
    pub points: Vec<GeoPoint>,
}

/// Boundary dataset kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BoundaryKind {
    /// World/country boundaries from `wc.dat`.
    World,
    /// County boundaries from `cb.dat`.
    County,
    /// United States county boundaries from `USCOUN.DAT`.
    UsCounty,
}

/// Parsed semantic boundary dataset.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BoundaryDataset {
    /// Source path.
    pub path: PathBuf,
    /// Dataset kind.
    pub kind: BoundaryKind,
    /// Boundary segments.
    pub segments: Vec<BoundarySegment>,
}

impl BoundaryDataset {
    /// Open and parse a text boundary dataset.
    pub(crate) fn open(path: impl AsRef<Path>, kind: BoundaryKind) -> Result<Self> {
        let file = BoundaryFile::open(path)?;
        Ok(Self {
            path: file.path,
            kind,
            segments: file.segments,
        })
    }

    /// Total coordinate count across all segments.
    #[must_use]
    pub fn point_count(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.points.len())
            .sum()
    }
}

/// Parsed text boundary file such as `wc.dat` or `cb.dat`.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryFile {
    /// Source path.
    pub path: PathBuf,
    /// Boundary segments.
    pub segments: Vec<BoundarySegment>,
}

impl BoundaryFile {
    /// Open and parse a CRLF text boundary file.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let mut segments = Vec::new();
        let mut current: Option<BoundarySegment> = None;

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(header) = line.strip_prefix("# -b ") {
                if let Some(segment) = current.take() {
                    segments.push(segment_with_point_bounds(segment));
                }
                current = parse_boundary_header(header);
                continue;
            }
            if let (Some(segment), Some(point)) = (&mut current, parse_geo_point(line)) {
                segment.points.push(point);
            }
        }
        if let Some(segment) = current {
            segments.push(segment_with_point_bounds(segment));
        }
        Ok(Self { path, segments })
    }
}

/// One county record from `USCOUN.DAT`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UsCountyBoundary {
    /// County identifier when present.
    pub id: Option<String>,
    /// County display name when present.
    pub name: Option<String>,
    /// Effective bounds when present, expanded when needed to include parsed points.
    pub bounds: Option<BoundaryBox>,
    /// Byte offset of the next record when present.
    pub next_offset: Option<u64>,
    /// Boundary points.
    pub points: Vec<GeoPoint>,
}

/// Parsed `USCOUN.DAT` county boundary sidecar.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UsCountyBoundaryDataset {
    /// Source path.
    pub path: PathBuf,
    /// County records.
    pub counties: Vec<UsCountyBoundary>,
}

impl UsCountyBoundaryDataset {
    /// Open and parse `USCOUN.DAT`.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let mut counties = Vec::new();
        let mut current: Option<UsCountyBoundary> = None;

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(header) = line.strip_prefix("# -b ") {
                if let Some(county) = current.take() {
                    counties.push(county_with_point_bounds(county));
                }
                let segment = parse_boundary_header(header);
                current = Some(UsCountyBoundary {
                    id: None,
                    name: None,
                    bounds: segment.as_ref().map(|segment| segment.bounds),
                    next_offset: segment.as_ref().map(|segment| segment.next_offset),
                    points: Vec::new(),
                });
                continue;
            }
            if let Some(header) = line.strip_prefix('#') {
                if let Some(county) = current.take() {
                    counties.push(county_with_point_bounds(county));
                }
                current = Some(parse_us_county_header(header.trim()));
                continue;
            }
            if line.contains('*') {
                if let Some(county) = current.take() {
                    counties.push(county_with_point_bounds(county));
                }
                current = Some(parse_us_county_header(line));
                continue;
            }
            if let (Some(county), Some(point)) = (&mut current, parse_geo_point(line)) {
                county.points.push(point);
            }
        }
        if let Some(county) = current {
            counties.push(county_with_point_bounds(county));
        }
        Ok(Self { path, counties })
    }

    /// Convert county records to generic boundary segments.
    #[must_use]
    pub fn boundary_dataset(&self) -> BoundaryDataset {
        let segments = self
            .counties
            .iter()
            .map(|county| BoundarySegment {
                bounds: county
                    .bounds
                    .unwrap_or_else(|| bounds_for_points(&county.points)),
                next_offset: county.next_offset.unwrap_or(0),
                points: county.points.clone(),
            })
            .collect();
        BoundaryDataset {
            path: self.path.clone(),
            kind: BoundaryKind::UsCounty,
            segments,
        }
    }
}

/// One photo manifest entry from `photos/PHOTOS.TXT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoManifestEntry {
    /// Callsign key.
    pub callsign: String,
    /// Manifest file name or relative path when present.
    pub file: Option<String>,
}

/// Parsed photo manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoCatalog {
    /// Source path.
    pub path: PathBuf,
    entries: Vec<PhotoManifestEntry>,
}

impl PhotoCatalog {
    /// Open and parse `photos/PHOTOS.TXT`.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let text = std::fs::read_to_string(&path)?;
        Ok(Self {
            path,
            entries: parse_photo_manifest(&text),
        })
    }

    /// Parse a photo manifest.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn parse(text: &str) -> Self {
        Self {
            path: PathBuf::new(),
            entries: parse_photo_manifest(text),
        }
    }

    /// Return all manifest entries.
    #[must_use]
    pub fn entries(&self) -> &[PhotoManifestEntry] {
        &self.entries
    }

    /// Return manifest entries for a callsign.
    #[must_use]
    pub fn lookup(&self, callsign: &str) -> Vec<&PhotoManifestEntry> {
        let callsign = callsign.trim().to_ascii_uppercase();
        self.entries
            .iter()
            .filter(|entry| entry.callsign == callsign)
            .collect()
    }
}

/// One packed state-map triple from `state.dat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedStatePoint {
    /// Drawing command.
    pub command: i16,
    /// Latitude in signed arc-minutes.
    pub lat_minutes: i16,
    /// Longitude in signed arc-minutes.
    pub lon_minutes: i16,
}

/// Parsed packed coordinate stream from `state.dat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedStateMap {
    /// Source path.
    pub path: PathBuf,
    /// Six-byte little-endian records.
    pub points: Vec<PackedStatePoint>,
}

/// One renderable vector path from `state.dat`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StateVectorSegment {
    /// Drawing command that starts this path.
    pub command: i16,
    /// Absolute geographic points.
    pub points: Vec<GeoPoint>,
}

/// Renderable vector-path dataset decoded from `state.dat`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StateVectorDataset {
    /// Source path.
    pub path: PathBuf,
    /// Vector paths.
    pub segments: Vec<StateVectorSegment>,
}

impl StateVectorDataset {
    /// Total coordinate count across all vector paths.
    #[must_use]
    pub fn point_count(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.points.len())
            .sum()
    }
}

impl PackedStateMap {
    /// Open and parse `state.dat` as little-endian packed triples.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let bytes = std::fs::read(&path)?;
        let points = bytes
            .chunks_exact(6)
            .map(|chunk| PackedStatePoint {
                command: i16::from_le_bytes([chunk[0], chunk[1]]),
                lat_minutes: i16::from_le_bytes([chunk[2], chunk[3]]),
                lon_minutes: i16::from_le_bytes([chunk[4], chunk[5]]),
            })
            .collect();
        Ok(Self { path, points })
    }

    /// Convert packed triples into official state-map vector paths.
    #[must_use]
    pub(crate) fn vector_dataset(&self) -> StateVectorDataset {
        let mut segments = Vec::new();
        let mut current: Option<StateVectorSegment> = None;
        for point in &self.points {
            if point.command > 10 {
                if let Some(segment) = current.take() {
                    segments.push(segment);
                }
                current = Some(StateVectorSegment {
                    command: point.command,
                    points: vec![state_vector_geo_point(point)],
                });
            } else if let Some(segment) = &mut current {
                segment.points.push(state_vector_geo_point(point));
            }
        }
        if let Some(segment) = current {
            segments.push(segment);
        }
        StateVectorDataset {
            path: self.path.clone(),
            segments,
        }
    }
}

/// Country centroid parsed from `countrys.nam`.
#[derive(Debug, Clone, PartialEq)]
pub struct CountryNameCentroid {
    /// Country display name.
    pub name: String,
    /// Representative country location.
    pub location: GeoPoint,
}

/// Parsed `countrys.nam` country-centroid catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CountryNameTable {
    centroids: HashMap<String, CountryNameCentroid>,
}

impl CountryNameTable {
    /// Open and parse `countrys.nam`.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::parse(&text))
    }

    /// Parse a country-centroid catalog.
    #[must_use]
    pub(crate) fn parse(text: &str) -> Self {
        let mut centroids = HashMap::new();
        for line in text.lines() {
            if let Some(centroid) = parse_country_name_centroid(line) {
                centroids.insert(centroid.name.to_ascii_uppercase(), centroid);
            }
        }
        Self { centroids }
    }

    /// Number of country centroids.
    #[must_use]
    pub fn len(&self) -> usize {
        self.centroids.len()
    }

    /// Whether no centroids were parsed.
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.centroids.is_empty()
    }

    /// Look up a centroid by country name.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&CountryNameCentroid> {
        self.centroids.get(&name.trim().to_ascii_uppercase())
    }
}

fn parse_count_record(record: &[u8; COUNT_RECORD_LEN]) -> Option<LookupCountRecord> {
    let key = trim_ascii_lossy(&record[..COUNT_KEY_LEN]);
    if key.is_empty() {
        return None;
    }
    let count = trim_ascii_lossy(&record[COUNT_KEY_LEN..COUNT_FIELD_END])
        .parse::<u32>()
        .ok()?;
    let updated_yyyymmdd = trim_ascii_lossy(&record[COUNT_FIELD_END..COUNT_DATE_END])
        .parse::<u32>()
        .ok()
        .filter(|date| (19900101..=21000101).contains(date));
    let status = record
        .get(COUNT_DATE_END)
        .copied()
        .filter(u8::is_ascii_graphic)
        .map(char::from);
    Some(LookupCountRecord {
        key,
        count,
        updated_yyyymmdd,
        status,
    })
}

fn repeated_key(key: &str, byte: u8) -> bool {
    !key.is_empty() && key.bytes().all(|candidate| candidate == byte)
}

fn state_vector_geo_point(point: &PackedStatePoint) -> GeoPoint {
    GeoPoint {
        lat: f64::from(point.lat_minutes) / 60.0,
        lon: f64::from(point.lon_minutes) / 60.0,
    }
}

fn parse_country_name_centroid(line: &str) -> Option<CountryNameCentroid> {
    let line = line.trim();
    let open = line.rfind('(')?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let name = line[..open].trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let mut parts = line[open + 1..close].split(',').map(str::trim);
    let lat = parts.next()?.parse::<f64>().ok()?;
    let lon = parts.next()?.parse::<f64>().ok()?;
    if !(lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon))
    {
        return None;
    }
    Some(CountryNameCentroid {
        name,
        location: GeoPoint { lon, lat },
    })
}

fn parse_boundary_header(header: &str) -> Option<BoundarySegment> {
    let parts = header.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return None;
    }
    Some(BoundarySegment {
        bounds: BoundaryBox {
            min_lon: parts[0].parse().ok()?,
            max_lon: parts[1].parse().ok()?,
            min_lat: parts[2].parse().ok()?,
            max_lat: parts[3].parse().ok()?,
        },
        next_offset: parts[4].parse().ok()?,
        points: Vec::new(),
    })
}

fn parse_geo_point(line: &str) -> Option<GeoPoint> {
    let mut parts = line.split_whitespace();
    let lon = parts.next()?.parse().ok()?;
    let lat = parts.next()?.parse().ok()?;
    Some(GeoPoint { lon, lat })
}

fn parse_us_county_header(header: &str) -> UsCountyBoundary {
    let separator = if header.contains('*') { '*' } else { '|' };
    let mut fields = header.split(separator).map(str::trim);
    let id = fields
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let name = fields
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let values = fields.collect::<Vec<_>>();
    let bounds = if values.len() >= 4 {
        match (
            values[0].parse(),
            values[1].parse(),
            values[2].parse(),
            values[3].parse(),
        ) {
            (Ok(min_lon), Ok(max_lon), Ok(min_lat), Ok(max_lat)) => Some(BoundaryBox {
                min_lon,
                max_lon,
                min_lat,
                max_lat,
            }),
            _ => None,
        }
    } else {
        None
    };
    let next_offset = values.get(4).and_then(|value| value.parse().ok());
    UsCountyBoundary {
        id,
        name,
        bounds,
        next_offset,
        points: Vec::new(),
    }
}

fn bounds_for_points(points: &[GeoPoint]) -> BoundaryBox {
    let mut bounds = BoundaryBox {
        min_lon: 0.0,
        max_lon: 0.0,
        min_lat: 0.0,
        max_lat: 0.0,
    };
    let Some(first) = points.first() else {
        return bounds;
    };
    bounds = BoundaryBox {
        min_lon: first.lon,
        max_lon: first.lon,
        min_lat: first.lat,
        max_lat: first.lat,
    };
    for point in points {
        bounds.min_lon = bounds.min_lon.min(point.lon);
        bounds.max_lon = bounds.max_lon.max(point.lon);
        bounds.min_lat = bounds.min_lat.min(point.lat);
        bounds.max_lat = bounds.max_lat.max(point.lat);
    }
    bounds
}

fn segment_with_point_bounds(mut segment: BoundarySegment) -> BoundarySegment {
    expand_bounds_to_points(&mut segment.bounds, &segment.points);
    segment
}

fn county_with_point_bounds(mut county: UsCountyBoundary) -> UsCountyBoundary {
    if let Some(bounds) = &mut county.bounds {
        expand_bounds_to_points(bounds, &county.points);
    }
    county
}

fn expand_bounds_to_points(bounds: &mut BoundaryBox, points: &[GeoPoint]) {
    for point in points {
        bounds.min_lon = bounds.min_lon.min(point.lon);
        bounds.max_lon = bounds.max_lon.max(point.lon);
        bounds.min_lat = bounds.min_lat.min(point.lat);
        bounds.max_lat = bounds.max_lat.max(point.lat);
    }
}

fn parse_photo_manifest(text: &str) -> Vec<PhotoManifestEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split(|ch: char| ch.is_ascii_whitespace() || matches!(ch, ',' | '|'));
        let Some(callsign) = parts.next() else {
            continue;
        };
        let callsign = callsign.trim().to_ascii_uppercase();
        if callsign.is_empty() {
            continue;
        }
        let file = parts
            .find(|part| !part.trim().is_empty())
            .map(|part| part.trim().to_owned());
        entries.push(PhotoManifestEntry { callsign, file });
    }
    entries
}

fn decode_v2_sidecar_byte(offset: u64, encoded: u8) -> u8 {
    let stream_key = ((offset + 4) % 101) as u8;
    (encoded ^ 7) ^ stream_key
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lookup_count_record() {
        let bytes = *b"----W4O        1      20130805N\r\n";
        let record = parse_count_record(&bytes).unwrap();
        assert_eq!(record.key, "----W4O");
        assert_eq!(record.count, 1);
        assert_eq!(record.updated_yyyymmdd, Some(20130805));
        assert_eq!(record.status, Some('N'));
    }

    #[test]
    fn parses_lookup_count_record_ignores_sentinel_date() {
        let bytes = *b"!!!!!!!!!!!!!!!1      99999999N\r\n";
        let record = parse_count_record(&bytes).unwrap();
        assert_eq!(record.key, "!!!!!!!!!!!!!!!");
        assert_eq!(record.count, 1);
        assert_eq!(record.updated_yyyymmdd, None);
        assert_eq!(record.status, Some('N'));
    }

    #[test]
    fn lookup_count_searches_main_table_after_prologue_and_sentinels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counts.dat");
        write_encoded_count_slots(
            &path,
            &[
                *b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
                *b"M0GGO          1560   20240701 \r\n",
                *b"!!!!!!!!!!!!!!!1      99999999N\r\n",
                *b"M0GGO          1951   20260309 \r\n",
                *b"zzzzzzzzzzzzzzz1      99999999N\r\n",
            ],
        );
        let counts = LookupCountFile::open(&path).unwrap();

        assert_eq!(counts.lookup("M0GGO").unwrap().count, 1951);
        assert_eq!(counts.lookup("!!!!!!!!!!!!!!!"), None);
        assert_eq!(counts.lookup("zzzzzzzzzzzzzzz"), None);
    }

    #[test]
    fn lookup_counts_iterates_searchable_catalog_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counts.dat");
        write_encoded_count_slots(
            &path,
            &[
                *b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
                *b"K0AB           1      20240101 \r\n",
                *b"!!!!!!!!!!!!!!!1      99999999N\r\n",
                *b"K0AB           7      20260509N\r\n",
                *b"W1AW           9      20260510N\r\n",
                *b"zzzzzzzzzzzzzzz1      99999999N\r\n",
            ],
        );
        let counts = LookupCounts::open(&path).unwrap();

        assert_eq!(counts.path(), path.as_path());
        assert_eq!(counts.len(), 2);
        assert!(!counts.is_empty());
        assert_eq!(counts.lookup("k0ab").unwrap().count, 7);
        assert_eq!(
            counts
                .iter()
                .map(|record| (record.key, record.count))
                .collect::<Vec<_>>(),
            vec![("K0AB".to_owned(), 7), ("W1AW".to_owned(), 9)]
        );
    }

    #[test]
    fn lookup_count_uses_whole_file_without_real_sentinel_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counts.dat");
        write_encoded_count_slots(&path, &[*b"K0AB           7      20260509N\r\n"]);
        let counts = LookupCountFile::open(&path).unwrap();

        assert_eq!(counts.lookup("K0AB").unwrap().count, 7);
    }

    #[test]
    fn parses_boundary_header_and_points() {
        let segment = parse_boundary_header("-168.978876 -90 71.641176 72.000315 583").unwrap();
        assert_eq!(segment.next_offset, 583);
        assert_eq!(
            segment.bounds,
            BoundaryBox {
                min_lon: -168.978876,
                max_lon: -90.0,
                min_lat: 71.641176,
                max_lat: 72.000315,
            }
        );
        assert_eq!(
            parse_geo_point("-168.978876\t72.000315"),
            Some(GeoPoint {
                lon: -168.978876,
                lat: 72.000315,
            })
        );
    }

    #[test]
    fn boundary_file_bounds_include_points() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wc.dat");
        std::fs::write(
            &path,
            "# -b -59.885 -59.885 75.913 75.913 123\n-59.885 75.913\n-59.217 75.913\n",
        )
        .unwrap();
        let dataset = BoundaryFile::open(&path).unwrap();
        assert_eq!(dataset.segments[0].bounds.min_lon, -59.885);
        assert_eq!(dataset.segments[0].bounds.max_lon, -59.217);
    }

    #[test]
    fn parses_photo_manifest() {
        let catalog = PhotoCatalog::parse("K0AB K0AB-2.JPG\n# comment\nw1aw|w1aw.png\n");
        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(
            catalog.lookup("k0ab")[0].file.as_deref(),
            Some("K0AB-2.JPG")
        );
        assert_eq!(catalog.lookup("W1AW")[0].file.as_deref(), Some("w1aw.png"));
    }

    #[test]
    fn parses_us_county_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("USCOUN.DAT");
        std::fs::write(
            &path,
            "# 001|Test County|-72|-71|41|42|123\n-72 41\n-71 42\n",
        )
        .unwrap();
        let dataset = UsCountyBoundaryDataset::open(&path).unwrap();
        assert_eq!(dataset.counties.len(), 1);
        assert_eq!(dataset.counties[0].id.as_deref(), Some("001"));
        assert_eq!(dataset.counties[0].name.as_deref(), Some("Test County"));
        assert_eq!(dataset.counties[0].points.len(), 2);
        assert_eq!(dataset.boundary_dataset().kind, BoundaryKind::UsCounty);
    }

    #[test]
    fn parses_star_delimited_us_county_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("USCOUN.DAT");
        std::fs::write(
            &path,
            "114*Example County*-95.34261*-94.4287*48.36597*49.38436*0000006792\n-94.88496 48.77333\n",
        )
        .unwrap();
        let dataset = UsCountyBoundaryDataset::open(&path).unwrap();
        assert_eq!(dataset.counties.len(), 1);
        assert_eq!(dataset.counties[0].id.as_deref(), Some("114"));
        assert_eq!(dataset.counties[0].name.as_deref(), Some("Example County"));
        assert_eq!(dataset.counties[0].next_offset, Some(6792));
        assert_eq!(dataset.counties[0].points.len(), 1);
    }

    #[test]
    fn malformed_us_county_bounds_are_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("USCOUN.DAT");
        std::fs::write(&path, "# 001|Test County|bad|-71|41|42|123\n-72 41\n").unwrap();
        let dataset = UsCountyBoundaryDataset::open(&path).unwrap();
        assert_eq!(dataset.counties.len(), 1);
        assert_eq!(dataset.counties[0].bounds, None);
        assert_eq!(dataset.counties[0].next_offset, Some(123));
    }

    #[test]
    fn parses_country_name_centroids() {
        let table = CountryNameTable::parse("AFGHANISTAN (34, 69.2)\nBAD\n");
        let centroid = table.lookup("afghanistan").unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(
            centroid.location,
            GeoPoint {
                lon: 69.2,
                lat: 34.0
            }
        );
    }

    #[test]
    fn country_name_centroids_reject_out_of_range_coordinates() {
        let table = CountryNameTable::parse("BADLAT (91, 0)\nBADLON (0, 181)\n");
        assert!(table.is_empty());
    }

    #[test]
    fn converts_packed_state_map_to_vector_paths() {
        let map = PackedStateMap {
            path: PathBuf::from("state.dat"),
            points: vec![
                PackedStatePoint {
                    command: 4000,
                    lat_minutes: 1800,
                    lon_minutes: -7451,
                },
                PackedStatePoint {
                    command: 1,
                    lat_minutes: 2940,
                    lon_minutes: -4244,
                },
            ],
        };
        let dataset = map.vector_dataset();
        assert_eq!(dataset.segments.len(), 1);
        assert_eq!(dataset.point_count(), 2);
        assert_eq!(
            dataset.segments[0].points[0],
            GeoPoint {
                lat: 30.0,
                lon: -124.18333333333334,
            }
        );
    }

    #[test]
    #[ignore = "requires licensed CALLBOOK_DB; validates raw PHOTOS.TXT parser invariants"]
    fn real_photo_manifest_raw_parser_invariants() {
        let path = real_ham0_dir().join("photos").join("PHOTOS.TXT");
        let catalog = PhotoCatalog::open(&path).expect("open real PHOTOS.TXT");
        let entries = catalog.entries();
        let entries_with_files = entries.iter().filter(|entry| entry.file.is_some()).count();
        let sample = entries
            .iter()
            .find(|entry| entry.file.is_some())
            .expect("real PHOTOS.TXT should include file-backed entries");

        assert!(
            entries.len() >= 1_000,
            "PHOTOS.TXT parsed too few entries: {}",
            entries.len()
        );
        assert!(
            entries_with_files > 0,
            "PHOTOS.TXT parsed no file-backed entries"
        );
        assert!(
            entries.iter().all(|entry| !entry.callsign.is_empty()
                && !entry.callsign.bytes().any(|byte| byte.is_ascii_lowercase())),
            "PHOTOS.TXT parsed an empty or non-normalized callsign"
        );
        assert!(
            catalog
                .lookup(&sample.callsign)
                .into_iter()
                .any(|entry| entry == sample),
            "PHOTOS.TXT lookup did not return a parsed sample entry"
        );

        println!(
            "PHOTOS.TXT path={} entries={} entries_with_files={entries_with_files}",
            catalog.path.display(),
            entries.len()
        );
    }

    #[test]
    #[ignore = "requires licensed CALLBOOK_DB; validates raw state.dat parser invariants"]
    fn real_packed_state_map_raw_parser_invariants() {
        let map =
            PackedStateMap::open(real_ham0_dir().join("state.dat")).expect("open real state.dat");
        let bytes = std::fs::metadata(&map.path)
            .expect("stat real state.dat")
            .len();
        let vector_dataset = map.vector_dataset();
        let start_commands = map.points.iter().filter(|point| point.command > 10).count();

        assert_eq!(
            bytes % 6,
            0,
            "state.dat length is not a whole packed stream"
        );
        assert!(
            map.points.len() >= 100,
            "state.dat parsed too few packed triples: {}",
            map.points.len()
        );
        assert!(
            start_commands >= 100,
            "state.dat parsed too few vector starts: {start_commands}"
        );
        assert_eq!(
            start_commands,
            vector_dataset.segments.len(),
            "state.dat vector conversion lost or added path starts"
        );
        assert!(
            map.points
                .iter()
                .all(|point| (-5400..=5400).contains(&point.lat_minutes)
                    && (-10800..=10800).contains(&point.lon_minutes)),
            "state.dat parsed out-of-range packed coordinates"
        );

        println!(
            "state.dat path={} packed_triples={} vector_segments={} vector_points={}",
            map.path.display(),
            map.points.len(),
            vector_dataset.segments.len(),
            vector_dataset.point_count()
        );
    }

    fn real_ham0_dir() -> PathBuf {
        let root = std::env::var_os("CALLBOOK_DB")
            .map(PathBuf::from)
            .expect("CALLBOOK_DB must point at a licensed HamCall database root");
        [root.join("ham0"), root]
            .into_iter()
            .find(|dir| dir.is_dir())
            .expect("CALLBOOK_DB did not contain a usable ham0 directory")
    }

    fn write_encoded_count_slots(path: &Path, slots: &[[u8; COUNT_RECORD_LEN]]) {
        let mut encoded = Vec::new();
        for (offset, byte) in slots.iter().flatten().copied().enumerate() {
            encoded.push(encode_v2_sidecar_byte(offset as u64, byte));
        }
        std::fs::write(path, encoded).unwrap();
    }

    fn encode_v2_sidecar_byte(offset: u64, decoded: u8) -> u8 {
        let stream_key = ((offset + 4) % 101) as u8;
        (decoded ^ stream_key) ^ 7
    }
}
