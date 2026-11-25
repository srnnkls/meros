use crate::path::Path;
use crate::value::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    Enforce,
    CreateOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    pub value: Value,
    pub policy: Policy,
}

/// Desired content for one file, expanded into units through its schema.
#[derive(Debug, Clone, Default)]
pub struct Declaration {
    #[expect(dead_code, reason = "read by unit derivation, whose body is `todo!()`")]
    entries: Vec<(Path, Declared)>,
}

impl FromIterator<(Path, Value, Policy)> for Declaration {
    fn from_iter<I: IntoIterator<Item = (Path, Value, Policy)>>(entries: I) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(path, value, policy)| (path, Declared { value, policy }))
                .collect(),
        }
    }
}

/// A unit's canonical address: its path, plus `[field=literal]` for keyed elements
/// or `[=<typed canonical JSON>]` for set elements.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitKey(String);

impl UnitKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One position in a document that the tool may own.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub key: UnitKey,
    pub current: Current,
    pub declared: Option<Declared>,
}

/// What a document holds at a unit's position.
#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    Absent,
    Present(Value),
    /// Several elements or members share the unit's identity.
    Duplicated,
    /// A parent holds a non-table value.
    UnderScalar,
    /// The container is not the kind its identity needs.
    InWrongContainer,
}
