use crate::error::SchemaError;
use crate::path::{Path, Segment};

/// How a container path's children are identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// The whole value is one unit.
    Atomic,
    /// Each scalar element of a list is a unit.
    Set,
    /// Each map element of a list is a unit, identified by the named field.
    Keyed(String),
    /// Each member of a map is a unit.
    Members,
}

impl Identity {
    /// Whether a `[]` schema entry may refine the container's elements or members.
    pub(crate) fn refinable(&self) -> bool {
        matches!(self, Self::Keyed(_) | Self::Members)
    }
}

/// Identity per container path for one file; paths without an entry are `Atomic`.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub(crate) entries: Vec<(Path, Identity)>,
}

impl Schema {
    /// Fails on an entry ending in `[]`, since a refined element takes no identity of its own,
    /// and on an entry nested inside an atomic unit: below another entry other than through
    /// a `[]` refining its keyed elements or members, or below a `[]` with no such entry.
    pub(crate) fn validate(&self) -> Result<(), SchemaError> {
        for (path, _) in &self.entries {
            let segments = &path.segments;
            let nested = |atomic: &[Segment]| SchemaError::NestedInAtomic {
                path: path.clone(),
                atomic: Path {
                    segments: atomic.to_vec(),
                },
            };
            if segments.last() == Some(&Segment::Any) {
                return Err(SchemaError::RefinedElement { path: path.clone() });
            }
            for (container, identity) in &self.entries {
                let depth = container.segments.len();
                if depth < segments.len()
                    && segments.starts_with(&container.segments)
                    && !(identity.refinable() && segments[depth] == Segment::Any)
                {
                    return Err(nested(&container.segments));
                }
            }
            for (depth, _) in segments
                .iter()
                .enumerate()
                .filter(|(_, segment)| **segment == Segment::Any)
            {
                if !self.entries.iter().any(|(container, identity)| {
                    container.segments == segments[..depth] && identity.refinable()
                }) {
                    return Err(nested(&segments[..depth]));
                }
            }
        }
        Ok(())
    }
}

impl FromIterator<(Path, Identity)> for Schema {
    fn from_iter<I: IntoIterator<Item = (Path, Identity)>>(entries: I) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Identity, Schema};
    use crate::error::SchemaError;
    use crate::path::Path;

    fn schema(entries: &[(&str, Identity)]) -> Schema {
        entries
            .iter()
            .map(|(text, identity)| (Path::parse(text).unwrap(), identity.clone()))
            .collect()
    }

    #[test]
    fn validate_rejects_entries_nested_in_atomic_units() {
        assert_eq!(
            schema(&[
                ("lint.select", Identity::Set),
                ("repos", Identity::Keyed("id".to_owned())),
                ("repos[].args", Identity::Set),
                ("mcpServers", Identity::Members),
                ("mcpServers[].env", Identity::Members),
            ])
            .validate(),
            Ok(()),
        );

        for (entries, nested, atomic) in [
            (
                vec![("tool", Identity::Atomic), ("tool.x.y", Identity::Set)],
                "tool.x.y",
                "tool",
            ),
            (
                vec![
                    ("repos", Identity::Keyed("id".to_owned())),
                    ("repos[].args", Identity::Atomic),
                    ("repos[].args.x", Identity::Members),
                ],
                "repos[].args.x",
                "repos[].args",
            ),
            (
                vec![("a", Identity::Members), ("a.b", Identity::Set)],
                "a.b",
                "a",
            ),
            (
                vec![("a", Identity::Members), ("a.b.c", Identity::Set)],
                "a.b.c",
                "a",
            ),
            (vec![("x[].y", Identity::Set)], "x[].y", "x"),
            (
                vec![("x", Identity::Set), ("x[].y", Identity::Set)],
                "x[].y",
                "x",
            ),
            (
                vec![("repos[].args", Identity::Set)],
                "repos[].args",
                "repos",
            ),
        ] {
            assert_eq!(
                schema(&entries).validate(),
                Err(SchemaError::NestedInAtomic {
                    path: Path::parse(nested).unwrap(),
                    atomic: Path::parse(atomic).unwrap(),
                }),
            );
        }

        assert_eq!(
            schema(&[("x", Identity::Members), ("x[]", Identity::Set)]).validate(),
            Err(SchemaError::RefinedElement {
                path: Path::parse("x[]").unwrap(),
            }),
        );
    }
}
