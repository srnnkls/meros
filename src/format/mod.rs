use std::collections::BTreeSet;

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
#[expect(
    dead_code,
    reason = "built by the format adapters, which are not implemented"
)]
pub(crate) struct Document {
    pub(crate) value: Value,
    pub(crate) commented: BTreeSet<Vec<Step>>,
    pub(crate) duplicated: BTreeSet<Vec<Step>>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
#[expect(
    dead_code,
    reason = "built by the format adapters, which are not implemented"
)]
pub(crate) enum Step {
    Key(String),
    Index(usize),
}
