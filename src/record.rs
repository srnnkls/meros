use std::collections::BTreeMap;

use crate::unit::UnitKey;

/// What the tool last wrote in one file; the caller stores it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    /// The hash scheme the entries use.
    pub scheme: Scheme,
    /// The owned units, sorted by key.
    pub entries: BTreeMap<UnitKey, Entry>,
}

/// The hash scheme of a record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scheme {
    /// `meros-1`: SHA-256 over RFC 8785 JSON of the typed projection.
    #[default]
    Meros1,
    /// A missing or unrecognized scheme: the record counts as no record.
    Unknown,
}

/// What the tool last wrote at one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The canonical hash of the written value.
    pub hash: [u8; 32],
    /// Marks a `CreateOnly` write.
    pub seeded: bool,
}
