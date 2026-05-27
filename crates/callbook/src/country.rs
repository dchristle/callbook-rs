//! Country-prefix table reader for the 2025 sidecar files.

use std::fs;
use std::path::Path;

use crate::error::Result;
use crate::modern::Jurisdiction;

/// Country metadata matched from a callsign prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountryMatch {
    /// Country name.
    pub name: String,
    /// Two-letter code when present in the sidecar file.
    pub code: Option<String>,
    /// Broad jurisdiction bucket.
    pub jurisdiction: Jurisdiction,
}

/// Source sidecar used for country metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountryInfoSource {
    /// Source was not identified by the caller.
    Unknown,
    /// `ham0/countrys`.
    Countrys,
    /// `ham0/gcmcountrys`.
    GcMcountrys,
    /// `ham0/COUNTRYS.PC`.
    CountrysPc,
}

/// Rich country metadata parsed from `countrys`/`gcmcountrys`.
#[derive(Debug, Clone, PartialEq)]
pub struct CountryInfo {
    /// Country label from the source sidecar.
    pub name: String,
    /// Exact country label from the source sidecar before cleanup.
    pub raw_name: String,
    /// Country label with source-specific routing suffixes removed.
    pub cleaned_name: String,
    /// Two-letter code when present in the sidecar file.
    pub code: Option<String>,
    /// Broad jurisdiction bucket.
    pub jurisdiction: Jurisdiction,
    /// ITU zone parsed from `!`.
    pub itu_zone: Option<u16>,
    /// CQ zone parsed from `@`.
    pub cq_zone: Option<u16>,
    /// Continent code parsed from `#`.
    pub continent: Option<String>,
    /// Representative latitude parsed from `$`.
    pub latitude: Option<f64>,
    /// Representative longitude parsed from `%`.
    pub longitude: Option<f64>,
    /// Numeric country/DXCC-like identifier parsed from `^`.
    pub numeric_code: Option<u16>,
    /// Source sidecar.
    pub source: CountryInfoSource,
}

impl CountryInfo {
    /// Convert rich metadata to the lightweight country match.
    #[must_use]
    pub fn to_match(&self) -> CountryMatch {
        CountryMatch {
            name: self.name.clone(),
            code: self.code.clone(),
            jurisdiction: self.jurisdiction,
        }
    }
}

#[derive(Debug, Clone)]
struct CountryRule {
    start: String,
    end: String,
    country: CountryInfo,
}

/// Prefix/range matcher for `countrys`/`gcmcountrys`.
#[derive(Debug, Clone, Default)]
pub struct CountryTable {
    rules: Vec<CountryRule>,
    first_byte_index: Vec<Vec<usize>>,
}

impl CountryTable {
    /// Load country-prefix rules from a sidecar text file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = match path.file_name().and_then(|name| name.to_str()) {
            Some("countrys") => CountryInfoSource::Countrys,
            Some("gcmcountrys") => CountryInfoSource::GcMcountrys,
            Some("COUNTRYS.PC") => CountryInfoSource::CountrysPc,
            _ => CountryInfoSource::Unknown,
        };
        let text = fs::read_to_string(path)?;
        Ok(Self::parse_with_source(&text, source))
    }

    /// Parse country-prefix rules.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self::parse_with_source(text, CountryInfoSource::Unknown)
    }

    /// Parse country-prefix rules with an explicit source label.
    #[must_use]
    pub fn parse_with_source(text: &str, source: CountryInfoSource) -> Self {
        let mut rules = Vec::new();
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() || line.len() < 12 {
                continue;
            }
            let start = line.get(0..6).unwrap_or("").trim();
            let end = line.get(6..12).unwrap_or("").trim();
            if start.is_empty() || end.is_empty() {
                continue;
            }
            let rest = line.get(12..).unwrap_or("").trim();
            let parsed = parse_country_info(rest, source);
            let name = parsed.name.clone();
            if name.is_empty() {
                continue;
            }
            rules.push(CountryRule {
                start: start.to_ascii_uppercase(),
                end: end.to_ascii_uppercase(),
                country: parsed,
            });
        }
        rules.sort_by(|a, b| {
            b.start
                .len()
                .cmp(&a.start.len())
                .then_with(|| a.start.cmp(&b.start))
        });
        let first_byte_index = build_first_byte_index(&rules);
        Self {
            rules,
            first_byte_index,
        }
    }

    /// Number of parsed country-prefix rules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether no country-prefix rules were parsed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Return parsed country metadata records in matching priority order.
    #[must_use]
    pub fn records(&self) -> Vec<CountryInfo> {
        self.rules.iter().map(|rule| rule.country.clone()).collect()
    }

    /// Match a callsign to the most specific country-prefix rule.
    #[must_use]
    pub fn lookup(&self, callsign: &str) -> Option<CountryMatch> {
        self.lookup_info(callsign).map(|country| country.to_match())
    }

    /// Match a callsign to rich country metadata.
    #[must_use]
    pub fn lookup_info(&self, callsign: &str) -> Option<CountryInfo> {
        let call = callsign.trim().to_ascii_uppercase();
        let first = call.as_bytes().first().copied();
        let candidates = first
            .filter(|byte| byte.is_ascii())
            .and_then(|byte| self.first_byte_index.get(usize::from(byte)));

        if let Some(candidates) = candidates {
            for &index in candidates {
                let rule = &self.rules[index];
                if prefix_range_matches(&call, &rule.start, &rule.end) {
                    return Some(rule.country.clone());
                }
            }
            return None;
        }

        for rule in &self.rules {
            if prefix_range_matches(&call, &rule.start, &rule.end) {
                return Some(rule.country.clone());
            }
        }
        None
    }

    /// Match a callsign by scanning every parsed rule without using indexes.
    #[doc(hidden)]
    #[must_use]
    pub fn lookup_linear(&self, callsign: &str) -> Option<CountryMatch> {
        let call = callsign.trim().to_ascii_uppercase();
        self.lookup_linear_normalized(&call)
    }

    fn lookup_linear_normalized(&self, call: &str) -> Option<CountryMatch> {
        for rule in &self.rules {
            if prefix_range_matches(call, &rule.start, &rule.end) {
                return Some(rule.country.to_match());
            }
        }
        None
    }
}

fn build_first_byte_index(rules: &[CountryRule]) -> Vec<Vec<usize>> {
    let mut index = vec![Vec::new(); 128];
    for (rule_index, rule) in rules.iter().enumerate() {
        for byte in matching_first_bytes(&rule.start, &rule.end) {
            index[usize::from(byte)].push(rule_index);
        }
    }
    index
}

fn matching_first_bytes(start: &str, end: &str) -> std::ops::RangeInclusive<u8> {
    let start = start.as_bytes().first().copied().unwrap_or(0);
    let end = end.as_bytes().first().copied().unwrap_or(start);
    start.min(end)..=start.max(end)
}

fn prefix_range_matches(call: &str, start: &str, end: &str) -> bool {
    if start == end {
        return call.starts_with(start);
    }
    let width = start.len().max(end.len()).min(call.len());
    let head = &call[..width];
    let start_head = &start[..start.len().min(width)];
    let end_head = &end[..end.len().min(width)];
    head >= start_head && head <= end_head
}

fn parse_country_info(rest: &str, source: CountryInfoSource) -> CountryInfo {
    let tag_start = rest.find(" [");
    let metadata_start = ['!', '@', '#', '$', '%', '^']
        .into_iter()
        .filter_map(|tag| rest.find(tag))
        .min();
    let name_end = tag_start
        .into_iter()
        .chain(metadata_start)
        .min()
        .unwrap_or(rest.len());
    let raw_name = rest[..name_end].trim().to_owned();
    let cleaned_name = clean_country_label(&raw_name);
    let code = tag_start.and_then(|start| {
        let after = &rest[start + 2..];
        let end = after.find(']')?;
        let code = after[..end].trim();
        (!code.is_empty()).then(|| code.to_owned())
    });
    let jurisdiction = classify(&raw_name, code.as_deref());
    CountryInfo {
        name: raw_name.clone(),
        raw_name,
        cleaned_name,
        code,
        jurisdiction,
        itu_zone: parse_tag_u16(rest, '!'),
        cq_zone: parse_tag_u16(rest, '@'),
        continent: parse_tag_string(rest, '#'),
        latitude: parse_tag_f64(rest, '$'),
        longitude: parse_tag_f64(rest, '%'),
        numeric_code: parse_tag_u16(rest, '^'),
        source,
    }
}

fn clean_country_label(label: &str) -> String {
    label
        .split_once(" - ")
        .map_or(label, |(head, _)| head)
        .trim()
        .to_owned()
}

fn parse_tag_string(text: &str, tag: char) -> Option<String> {
    let start = text.find(tag)? + tag.len_utf8();
    let value = text[start..]
        .split(['!', '@', '#', '$', '%', '^', '&'])
        .next()
        .unwrap_or("")
        .trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_tag_u16(text: &str, tag: char) -> Option<u16> {
    parse_tag_string(text, tag)?.parse().ok()
}

fn parse_tag_f64(text: &str, tag: char) -> Option<f64> {
    parse_tag_string(text, tag)?.parse().ok()
}

fn classify(name: &str, code: Option<&str>) -> Jurisdiction {
    match (name, code) {
        (_, Some("US")) | ("UNITED STATES", _) => Jurisdiction::UnitedStates,
        (_, Some("CA")) | ("CANADA", _) => Jurisdiction::Canada,
        _ => Jurisdiction::International,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_matches_prefixes() {
        let table = CountryTable::parse(
            "S5    S5    SLOVENIA [SI] !28@15#EU$46.03%14.31\n\
             K     K     UNITED STATES [US] !08@05#NA\n",
        );
        let si = table.lookup("S51DX").unwrap();
        assert_eq!(si.name, "SLOVENIA");
        assert_eq!(si.code.as_deref(), Some("SI"));
        assert_eq!(si.jurisdiction, Jurisdiction::International);
        let si_info = table.lookup_info("S51DX").unwrap();
        assert_eq!(si_info.raw_name, "SLOVENIA");
        assert_eq!(si_info.cleaned_name, "SLOVENIA");
        assert_eq!(si_info.itu_zone, Some(28));
        assert_eq!(si_info.cq_zone, Some(15));
        assert_eq!(si_info.continent.as_deref(), Some("EU"));
        assert_eq!(si_info.latitude, Some(46.03));
        assert_eq!(si_info.longitude, Some(14.31));
        assert_eq!(
            table.lookup("K0ABC").unwrap().jurisdiction,
            Jurisdiction::UnitedStates
        );
    }

    #[test]
    fn preserves_raw_and_cleaned_country_labels() {
        let table = CountryTable::parse(
            "OH0   OH0ZZ ALAND ISLANDS - FINLAND [FI] !18@15#EU$60.25%20\n\
             T8    T8    PALAU$7.479%134.548^022\n",
        );
        let info = table.lookup_info("OH0ZZ").unwrap();

        assert_eq!(info.name, "ALAND ISLANDS - FINLAND");
        assert_eq!(info.raw_name, "ALAND ISLANDS - FINLAND");
        assert_eq!(info.cleaned_name, "ALAND ISLANDS");

        let palau = table.lookup_info("T88ZZ").unwrap();
        assert_eq!(palau.name, "PALAU");
        assert_eq!(palau.longitude, Some(134.548));
    }

    #[test]
    fn first_byte_index_preserves_range_matches() {
        let table = CountryTable::parse(
            "K     N     UNITED STATES [US] !08@05#NA\n\
             S5    S5    SLOVENIA [SI] !28@15#EU$46.03%14.31\n",
        );

        assert_eq!(
            table.lookup("N0CALL").unwrap().jurisdiction,
            Jurisdiction::UnitedStates
        );
        assert_eq!(table.lookup("S51DX").unwrap().code.as_deref(), Some("SI"));
        assert!(table.lookup("Z9ZZZ").is_none());
    }

    #[test]
    fn indexed_lookup_matches_linear_lookup_for_generated_callsigns() {
        let table = CountryTable::parse(
            "1A    1A    SOV. MIL. ORDER OF MALTA !28#EU$41.9%12.4^246\n\
             3B6   3B7   AGALEGA & ST. BRANDON IS. !53@39#AF$-10.4%56.6^004\n\
             4A    4C    MEXICO [MX] !10@06#NA$19.4%-99.1^050\n\
             K     N     UNITED STATES [US] !08@05#NA\n\
             OH0   OH0ZZ ALAND ISLANDS - FINLAND [FI] !18@15#EU$60.25%20\n\
             S5    S5    SLOVENIA [SI] !28@15#EU$46.03%14.31\n\
             VP2E  VP2EZZANGUILLA - LEEWARD ISLANDS [AV] !11@08#NA$18%-63\n",
        );
        let mut rng = DeterministicRng::new(0x5eed_c0de);

        for _ in 0..10_000 {
            let callsign = generated_callsign(&mut rng);
            assert_eq!(
                table.lookup(&callsign),
                table.lookup_linear(&callsign),
                "indexed country lookup differs for {callsign}"
            );
        }
    }

    struct DeterministicRng(u64);

    impl DeterministicRng {
        fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            self.0
        }

        fn range(&mut self, end: usize) -> usize {
            (self.next() as usize) % end
        }
    }

    fn generated_callsign(rng: &mut DeterministicRng) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/";
        let len = 1 + rng.range(12);
        let mut out = String::with_capacity(len);
        for _ in 0..len {
            out.push(CHARS[rng.range(CHARS.len())] as char);
        }
        out
    }
}
