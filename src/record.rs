use std::collections::BTreeMap;

use crate::unit::UnitKey;

/// What the tool last wrote in one file; the caller stores it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    pub scheme: Scheme,
    pub entries: BTreeMap<UnitKey, Entry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    Meros1,
    /// A missing or unrecognized scheme: the record counts as no record.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub hash: [u8; 32],
    /// Marks a `CreateOnly` write.
    pub seeded: bool,
}
