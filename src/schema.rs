use crate::path::Path;

/// How a container path's children are identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Atomic,
    Set,
    /// Elements identified by the named field.
    Keyed(String),
    Members,
}

/// Identity per container path for one file; paths without an entry are `Atomic`.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    #[expect(dead_code, reason = "read by unit derivation, whose body is `todo!()`")]
    entries: Vec<(Path, Identity)>,
}

impl FromIterator<(Path, Identity)> for Schema {
    fn from_iter<I: IntoIterator<Item = (Path, Identity)>>(entries: I) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }
}
