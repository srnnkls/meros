use crate::path::Path;
use crate::value::Value;

/// How a declared unit is enforced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    /// The unit holds the declared value while it stays mine.
    Enforce,
    /// The unit is created once when absent and never changed afterwards.
    CreateOnly,
}

/// A unit's desired value and policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    /// The desired value.
    pub value: Value,
    /// How the value is enforced.
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
    /// The key's canonical text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One position in a document that the tool may own.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// The unit's address.
    pub key: UnitKey,
    /// What the document holds there.
    pub current: Current,
    /// The desired value, when the unit is declared.
    pub declared: Option<Declared>,
}

/// What a document holds at a unit's position.
#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    /// Nothing is there.
    Absent,
    /// The document holds this value.
    Present(Value),
    /// Several elements or members share the unit's identity.
    Duplicated,
    /// A parent holds a non-table value.
    UnderScalar,
    /// The container is not the kind its identity needs.
    InWrongContainer,
}
