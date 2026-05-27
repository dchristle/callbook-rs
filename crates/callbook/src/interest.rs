//! Reader for the `ham0/interest` profile-code catalog.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::Result;

/// One interest-code definition from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestDefinition {
    /// Four-digit interest code.
    pub code: String,
    /// Category heading from the catalog.
    pub category: String,
    /// Human-readable label.
    pub label: String,
}

/// Resolved interest selected on a callsign snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInterest {
    /// Four-digit interest code.
    pub code: String,
    /// Category heading from the catalog.
    pub category: String,
    /// Human-readable label.
    pub label: String,
}

/// Parsed interest-code catalog.
#[derive(Debug, Clone, Default)]
pub struct InterestTable {
    definitions: Vec<InterestDefinition>,
    by_code: HashMap<[u8; 4], usize>,
}

impl InterestTable {
    /// Load the interest-code catalog from `ham0/interest`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self::parse(&text))
    }

    /// Parse an interest-code catalog.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut definitions = Vec::new();
        let mut by_code = HashMap::new();
        let mut category = String::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("---") {
                category = rest.trim().to_owned();
                continue;
            }
            let Some((code, label)) = line.split_once('*') else {
                continue;
            };
            let code = code.trim();
            let label = label.trim();
            if code.len() != 4 || !code.bytes().all(|b| b.is_ascii_digit()) || label.is_empty() {
                continue;
            }
            let key = code_key(code.as_bytes()).expect("validated code length");
            let definition = InterestDefinition {
                code: code.to_owned(),
                category: category.clone(),
                label: label.to_owned(),
            };
            by_code.insert(key, definitions.len());
            definitions.push(definition);
        }

        Self {
            definitions,
            by_code,
        }
    }

    /// Number of catalog entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Whether the catalog has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Iterate definitions in catalog order.
    pub fn definitions(&self) -> impl Iterator<Item = &InterestDefinition> {
        self.definitions.iter()
    }

    /// Resolve one four-digit code.
    #[must_use]
    pub fn lookup(&self, code: &str) -> Option<&InterestDefinition> {
        let key = code_key(code.as_bytes())?;
        self.by_code
            .get(&key)
            .and_then(|index| self.definitions.get(*index))
    }

    /// Parse a raw concatenated code string into four-digit codes.
    #[must_use]
    pub fn codes(raw: &str) -> Vec<String> {
        raw.as_bytes()
            .chunks_exact(4)
            .filter(|chunk| chunk.iter().all(|b| b.is_ascii_digit()))
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect()
    }

    /// Resolve all known codes from a raw concatenated code string.
    #[must_use]
    pub fn resolve_raw(&self, raw: &str) -> Vec<ResolvedInterest> {
        Self::codes(raw)
            .into_iter()
            .filter_map(|code| self.lookup(&code).map(ResolvedInterest::from))
            .collect()
    }
}

impl From<&InterestDefinition> for ResolvedInterest {
    fn from(value: &InterestDefinition) -> Self {
        Self {
            code: value.code.clone(),
            category: value.category.clone(),
            label: value.label.clone(),
        }
    }
}

fn code_key(bytes: &[u8]) -> Option<[u8; 4]> {
    let key: [u8; 4] = bytes.try_into().ok()?;
    key.iter().all(|b| b.is_ascii_digit()).then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_categories_and_codes() {
        let table = InterestTable::parse(
            "---Bands\r\n\
             0010*160 meters\r\n\
             ---Organizations\r\n\
             1590*ARRL\r\n",
        );

        assert_eq!(table.len(), 2);
        assert_eq!(table.lookup("0010").unwrap().category, "Bands");
        assert_eq!(table.lookup("1590").unwrap().label, "ARRL");
    }

    #[test]
    fn skips_malformed_rows_and_resolves_known_codes() {
        let table = InterestTable::parse(
            "---Bands\n\
             0010*160 meters\n\
             nope\n\
             002*bad\n\
             0030*\n",
        );

        assert_eq!(InterestTable::codes("00109999003"), vec!["0010", "9999"]);
        assert_eq!(table.resolve_raw("00109999")[0].label, "160 meters");
        assert!(table.lookup("9999").is_none());
    }
}
