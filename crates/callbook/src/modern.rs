//! Owned lookup records for the verified 2025 `ham0` database.

use std::collections::BTreeMap;

use crate::interest::ResolvedInterest;
use crate::us_csv::UsCsvRecord;

/// Result of a callsign lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupResult {
    /// Normalized query callsign.
    pub query: String,
    /// Lookup status after all supported sources are consulted.
    pub status: LookupStatus,
    /// Best current record, if one is available.
    pub current: Option<CallSnapshot>,
    /// Historical snapshots for the callsign, sorted by vintage.
    pub history: Vec<CallSnapshot>,
}

/// High-level lookup outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupStatus {
    /// A current/non-vintage record was found.
    Current,
    /// Only vintage `CALL:YYYY` records were found.
    ArchiveOnly,
    /// No matching current or historical data was found.
    NotFound,
}

/// Source that contributed a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapshotSource {
    /// Current US FCC catalog shipped as `usa.csv.zip`.
    UsCsv,
    /// Direct hit from `hamcall.idx` into `hamcall.dat`.
    HamCallDatIdx,
    /// Hit routed through `hciindex.dat`/`hci.dat` postings.
    HamCallHci,
}

/// Country/jurisdiction classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jurisdiction {
    /// United States.
    UnitedStates,
    /// Canada.
    Canada,
    /// Any non-US/non-Canada country.
    International,
    /// No country data is available.
    Unknown,
}

/// One current or historical callsign snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSnapshot {
    /// Callsign without `:YYYY`.
    pub callsign: String,
    /// Publication vintage from `CALL:YYYY`, or `None` for current records.
    pub vintage: Option<u16>,
    /// Sources that contributed data to this snapshot.
    pub sources: Vec<SnapshotSource>,
    /// Jurisdiction derived from data-backed country/source information.
    pub jurisdiction: Jurisdiction,
    /// License class.
    pub license_class: Option<String>,
    /// Buckmaster record code.
    pub record_code: Option<String>,
    /// First name.
    pub first_name: Option<String>,
    /// Middle name or initial.
    pub middle_name: Option<String>,
    /// Last name, or full name/org when split name is absent.
    pub last_name: Option<String>,
    /// Suffix.
    pub suffix: Option<String>,
    /// Mailing street address.
    pub address: Option<String>,
    /// City.
    pub city: Option<String>,
    /// State/province.
    pub state_or_province: Option<String>,
    /// Postal code.
    pub postal_code: Option<String>,
    /// County.
    pub county: Option<String>,
    /// Country name or code.
    pub country: Option<String>,
    /// Birth date in `YYYYMMDD` form.
    pub birth_date: Option<String>,
    /// First-issued date in `YYYYMMDD` form.
    pub first_issued: Option<String>,
    /// Expiration date in `YYYYMMDD` form.
    pub expires: Option<String>,
    /// Last process/change date in `YYYYMMDD` form.
    pub last_changed: Option<String>,
    /// GMT offset/time zone.
    pub gmt_offset: Option<String>,
    /// Latitude.
    pub latitude: Option<String>,
    /// Longitude.
    pub longitude: Option<String>,
    /// Maidenhead grid square.
    pub grid: Option<String>,
    /// Area code.
    pub area_code: Option<String>,
    /// Previous call.
    pub previous_call: Option<String>,
    /// Previous license class.
    pub previous_class: Option<String>,
    /// FCC transaction type.
    pub fcc_transaction_type: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// QSL manager/instructions.
    pub qsl: Option<String>,
    /// URL.
    pub url: Option<String>,
    /// Fax number.
    pub fax_number: Option<String>,
    /// IOTA designator.
    pub iota: Option<String>,
    /// Raw concatenated 4-digit interest codes.
    pub interest_codes_raw: Option<String>,
    /// Parsed 4-digit interest codes.
    pub interest_codes: Vec<String>,
    /// Interest codes resolved through `ham0/interest`.
    pub interests: Vec<ResolvedInterest>,
    /// FCC/license identifier.
    pub license_id: Option<String>,
    /// FCC FRN.
    pub frn: Option<String>,
    /// Other observed numeric identifier.
    pub numeric_id: Option<String>,
    /// Fields whose semantics are not fully labeled yet.
    pub raw_tags: BTreeMap<u8, String>,
}

impl LookupResult {
    /// Empty not-found result for `query`.
    #[must_use]
    pub fn not_found(query: String) -> Self {
        Self {
            query,
            status: LookupStatus::NotFound,
            current: None,
            history: Vec::new(),
        }
    }
}

impl CallSnapshot {
    /// Create an empty snapshot for `callsign`.
    #[must_use]
    pub fn new(callsign: String, vintage: Option<u16>, source: SnapshotSource) -> Self {
        Self {
            callsign,
            vintage,
            sources: vec![source],
            jurisdiction: Jurisdiction::Unknown,
            license_class: None,
            record_code: None,
            first_name: None,
            middle_name: None,
            last_name: None,
            suffix: None,
            address: None,
            city: None,
            state_or_province: None,
            postal_code: None,
            county: None,
            country: None,
            birth_date: None,
            first_issued: None,
            expires: None,
            last_changed: None,
            gmt_offset: None,
            latitude: None,
            longitude: None,
            grid: None,
            area_code: None,
            previous_call: None,
            previous_class: None,
            fcc_transaction_type: None,
            email: None,
            qsl: None,
            url: None,
            fax_number: None,
            iota: None,
            interest_codes_raw: None,
            interest_codes: Vec::new(),
            interests: Vec::new(),
            license_id: None,
            frn: None,
            numeric_id: None,
            raw_tags: BTreeMap::new(),
        }
    }

    /// Build a current US snapshot from `usa.csv.zip`.
    #[must_use]
    pub fn from_us_csv(record: &UsCsvRecord) -> Self {
        let mut out = Self::new(record.callsign.clone(), None, SnapshotSource::UsCsv);
        out.jurisdiction = Jurisdiction::UnitedStates;
        out.license_class = non_empty_ref(&record.class);
        out.last_name = non_empty_ref(&record.name);
        out.address = non_empty_ref(&record.address);
        out.city = non_empty_ref(&record.city);
        out.state_or_province = non_empty_ref(&record.state);
        out.postal_code = non_empty_ref(&record.zip);
        out.county = non_empty_ref(&record.county);
        out.country = Some("United States".to_owned());
        out.first_issued = non_empty_ref(&record.license_issue_date);
        out.fcc_transaction_type = non_empty_ref(&record.fcc_transaction_type);
        out
    }

    /// Merge missing fields from `other`, preserving existing data.
    pub fn merge_missing_from(&mut self, other: CallSnapshot) {
        for source in other.sources {
            if !self.sources.contains(&source) {
                self.sources.push(source);
            }
        }
        self.sources.sort_unstable();

        if matches!(self.jurisdiction, Jurisdiction::Unknown) {
            self.jurisdiction = other.jurisdiction;
        }

        fill(&mut self.license_class, other.license_class);
        fill(&mut self.record_code, other.record_code);
        let other_has_split_name = other.first_name.is_some() || other.middle_name.is_some();
        fill(&mut self.first_name, other.first_name);
        fill(&mut self.middle_name, other.middle_name);
        if other_has_split_name && self.first_name.is_some() {
            if other.last_name.is_some() {
                self.last_name = other.last_name;
            }
        } else {
            fill(&mut self.last_name, other.last_name);
        }
        fill(&mut self.suffix, other.suffix);
        fill(&mut self.address, other.address);
        fill(&mut self.city, other.city);
        fill(&mut self.state_or_province, other.state_or_province);
        fill(&mut self.postal_code, other.postal_code);
        fill(&mut self.county, other.county);
        fill(&mut self.country, other.country);
        fill(&mut self.birth_date, other.birth_date);
        fill(&mut self.first_issued, other.first_issued);
        fill(&mut self.expires, other.expires);
        fill(&mut self.last_changed, other.last_changed);
        fill(&mut self.gmt_offset, other.gmt_offset);
        fill(&mut self.latitude, other.latitude);
        fill(&mut self.longitude, other.longitude);
        fill(&mut self.grid, other.grid);
        fill(&mut self.area_code, other.area_code);
        fill(&mut self.previous_call, other.previous_call);
        fill(&mut self.previous_class, other.previous_class);
        fill(&mut self.fcc_transaction_type, other.fcc_transaction_type);
        fill(&mut self.email, other.email);
        fill(&mut self.qsl, other.qsl);
        fill(&mut self.url, other.url);
        fill(&mut self.fax_number, other.fax_number);
        fill(&mut self.iota, other.iota);
        fill(&mut self.interest_codes_raw, other.interest_codes_raw);
        if self.interest_codes.is_empty() {
            self.interest_codes = other.interest_codes;
        }
        if self.interests.is_empty() {
            self.interests = other.interests;
        }
        fill(&mut self.license_id, other.license_id);
        fill(&mut self.frn, other.frn);
        fill(&mut self.numeric_id, other.numeric_id);
        self.raw_tags.extend(other.raw_tags);
    }

    /// Human-readable name assembled from split fields.
    #[must_use]
    pub fn display_name(&self) -> Option<String> {
        if self.first_name.is_none() && self.middle_name.is_none() {
            return self.last_name.clone();
        }
        let mut parts = Vec::new();
        if let Some(s) = &self.first_name {
            parts.push(s.as_str());
        }
        if let Some(s) = &self.middle_name {
            parts.push(s.as_str());
        }
        if let Some(s) = &self.last_name {
            parts.push(s.as_str());
        }
        if let Some(s) = &self.suffix {
            parts.push(s.as_str());
        }
        let name = parts.join(" ");
        (!name.is_empty()).then_some(name)
    }
}

fn fill(dst: &mut Option<String>, src: Option<String>) {
    if dst.as_ref().map_or(true, |s| s.is_empty()) {
        *dst = src;
    }
}

fn non_empty_ref(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
