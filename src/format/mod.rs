mod json;
mod toml;
mod yaml;

use std::collections::BTreeSet;

use crate::error::FormatError;
use crate::value::Value;

/// A target file's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// TOML.
    Toml,
    /// JSON and JSONC.
    Json,
    /// YAML 1.2, core schema.
    Yaml,
}

/// A parsed target document, the compound values holding a comment inside them, and the
/// map keys appearing more than once, which `value` holds only once.
#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) value: Value,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by `plan`, whose body is `todo!()`")
    )]
    pub(crate) commented: BTreeSet<Vec<Step>>,
    pub(crate) duplicated: BTreeSet<Vec<Step>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Key(String),
    Index(usize),
}

/// One change to a document; every `at` names a position in the document as read.
/// A created list element sits at the list's original length, so creates into one list
/// append in edit order.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "built by `plan`, whose body is `todo!()`")
)]
pub(crate) enum Edit {
    Replace { at: Vec<Step>, value: Value },
    Create { at: Vec<Step>, value: Value },
    Remove { at: Vec<Step> },
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called by `read`, whose body is `todo!()`")
)]
pub(crate) fn parse(format: Format, bytes: &[u8]) -> Result<Document, FormatError> {
    match format {
        Format::Toml => toml::parse(bytes),
        Format::Json => json::parse(bytes),
        Format::Yaml => yaml::parse(bytes),
    }
}

/// Applies `edits` in an order that keeps every `at` as read: changes in the given order,
/// then removals from the highest position down.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called by `plan`, whose body is `todo!()`")
)]
pub(crate) fn edit(format: Format, bytes: &[u8], edits: &[Edit]) -> Result<Vec<u8>, FormatError> {
    let read = parse(format, bytes)?.value;
    let creates_fit = edits.iter().all(|edit| match edit {
        Edit::Create { at, .. } => appends(&read, at),
        Edit::Replace { .. } | Edit::Remove { .. } => true,
    });
    if !creates_fit {
        return Err(FormatError::UnfitEdit);
    }
    let mut removals: Vec<(&Vec<Step>, &Edit)> = edits
        .iter()
        .filter_map(|edit| match edit {
            Edit::Remove { at } => Some((at, edit)),
            Edit::Replace { .. } | Edit::Create { .. } => None,
        })
        .collect();
    removals.sort_unstable_by(|(a, _), (b, _)| b.cmp(a));
    let ordered: Vec<&Edit> = edits
        .iter()
        .filter(|edit| !matches!(edit, Edit::Remove { .. }))
        .chain(removals.into_iter().map(|(_, edit)| edit))
        .collect();
    match format {
        Format::Toml => toml::edit(bytes, ordered),
        Format::Json => json::edit(bytes, ordered),
        Format::Yaml => yaml::edit(bytes, ordered),
    }
}

/// Whether a create at `at` lands at the end of every list `read` lacks it in: list steps
/// the document holds lead through it, the first one it lacks sits at the list's length.
fn appends(read: &Value, at: &[Step]) -> bool {
    let mut node = Some(read);
    for (depth, step) in at.iter().enumerate() {
        node = match step {
            Step::Key(key) => match node {
                Some(Value::Map(members)) => members.get(key),
                _ => None,
            },
            Step::Index(index) => {
                let elements = match node {
                    Some(Value::List(elements)) => elements.as_slice(),
                    _ => &[],
                };
                let last = depth + 1 == at.len();
                if *index > elements.len() || (last && *index < elements.len()) {
                    return false;
                }
                elements.get(*index)
            }
        };
    }
    true
}

/// Whether a document of `format` can hold `value`.
pub(crate) fn encodable(format: Format, value: &Value) -> bool {
    match format {
        Format::Toml => toml::encodable(value),
        Format::Json => json::encodable(value),
        Format::Yaml => yaml::encodable(value),
    }
}
