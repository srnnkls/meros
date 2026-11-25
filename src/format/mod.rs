mod toml;

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

#[expect(dead_code, reason = "called by `read`, whose body is `todo!()`")]
#[expect(clippy::todo, reason = "filled by MRS-009/MRS-010")]
pub(crate) fn parse(format: Format, bytes: &[u8]) -> Result<Document, FormatError> {
    match format {
        Format::Toml => toml::parse(bytes),
        Format::Json | Format::Yaml => todo!(),
    }
}

#[expect(dead_code, reason = "called by `plan`, whose body is `todo!()`")]
#[expect(clippy::todo, reason = "filled by MRS-009/MRS-010")]
pub(crate) fn edit(format: Format, bytes: &[u8], edits: &[Edit]) -> Result<Vec<u8>, FormatError> {
    match format {
        Format::Toml => toml::edit(bytes, edits),
        Format::Json | Format::Yaml => todo!(),
    }
}

/// Whether a document of `format` can hold `value`.
pub(crate) fn encodable(format: Format, value: &Value) -> bool {
    match format {
        Format::Toml => toml::encodable(value),
        Format::Json | Format::Yaml => match value {
            Value::Map(members) => members.values().all(|member| encodable(format, member)),
            Value::List(elements) => elements.iter().all(|element| encodable(format, element)),
            Value::Datetime(_) => false,
            _ => true,
        },
    }
}
