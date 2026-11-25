use std::path::PathBuf;

use thiserror::Error;

use crate::apply::AppliedTarget;
use crate::path::Path;
use crate::schema::Identity;

/// Why a string is not a valid taxis path.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    /// The path selects a list element by index, which taxis paths do not allow.
    #[error("path `{path}`: index selector at byte {at}")]
    IndexSelector {
        /// The rejected input.
        path: String,
        /// Byte offset of the index selector.
        at: usize,
    },
    /// The path breaks taxis's grammar.
    #[error("path `{path}`: invalid syntax at byte {at}")]
    Syntax {
        /// The rejected input.
        path: String,
        /// Byte offset of the first invalid character.
        at: usize,
    },
}

/// Why a schema is invalid.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchemaError {
    /// A schema entry lies inside a path that is itself one atomic unit.
    #[error("schema entry {path} lies inside the atomic unit {atomic}")]
    NestedInAtomic {
        /// The nested schema entry.
        path: Path,
        /// The atomic unit containing it.
        atomic: Path,
    },
    /// A schema entry ends in `[]`, but a refined element takes no identity of its own.
    #[error("schema entry {path} names a refined element, which takes no identity of its own")]
    RefinedElement {
        /// The schema entry ending in `[]`.
        path: Path,
    },
}

/// Why a declaration is invalid.
#[derive(Debug, Error, PartialEq)]
pub enum DeclarationError {
    /// Two declared paths address overlapping parts of the document.
    #[error("declared paths {path} and {other} overlap")]
    Overlap {
        /// One of the overlapping paths.
        path: Path,
        /// The path it overlaps.
        other: Path,
    },
    /// A declared container holds two elements or members with one identity.
    #[error("declaration {path} holds the identity `{identity}` twice")]
    DuplicateIdentity {
        /// The declared container.
        path: Path,
        /// The duplicated identity.
        identity: String,
    },
    /// A declared value is not the kind its identity takes.
    #[error("declaration {path} does not fit its identity {identity}")]
    KindMismatch {
        /// The declared path.
        path: Path,
        /// The identity the schema gives the path.
        identity: Identity,
    },
    /// A declared keyed element lacks its key field.
    #[error("declaration {path} has an element without its key field `{field}`")]
    MissingKeyField {
        /// The declared keyed container.
        path: Path,
        /// The missing key field.
        field: String,
    },
    /// A declared path uses a `[field=literal]` selector.
    #[error("declaration {path} uses a selector")]
    Selector {
        /// The declared path.
        path: Path,
    },
    /// A declared value cannot be encoded in the target's format.
    #[error("declaration {path} holds a value its format cannot encode")]
    Unencodable {
        /// The declared path.
        path: Path,
    },
    /// A declared value has no canonical projection.
    #[error("declaration {path} holds NaN, an infinity or an integer beyond 2^53")]
    Unprojectable {
        /// The declared path.
        path: Path,
    },
}

/// Why a format adapter cannot parse or edit a document.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    /// The document is not valid in its format.
    #[error("{message}")]
    Syntax {
        /// The parser's description of the error.
        message: String,
    },
    /// An edit names a position or value the document cannot take.
    #[error("the document cannot take the edit")]
    UnfitEdit,
    /// The YAML document uses a feature meros cannot edit safely.
    #[error("unsupported YAML: {0:?}")]
    UnsupportedYaml(YamlFeature),
}

/// A YAML feature that makes a target unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlFeature {
    /// A comment.
    Comment,
    /// An anchor (`&name`).
    Anchor,
    /// An alias (`*name`).
    Alias,
    /// An explicit tag (`!tag`).
    Tag,
    /// A map key appearing more than once.
    DuplicateKey,
    /// More than one document in the stream.
    MultipleDocuments,
}

/// Why `read` failed.
#[derive(Debug, Error)]
pub enum ReadError {
    /// A target could not be resolved or read.
    #[error("cannot read {file}")]
    Io {
        /// The target's file as declared.
        file: PathBuf,
        /// The I/O failure.
        source: std::io::Error,
    },
    /// A target could not be parsed.
    #[error("cannot parse {file}")]
    Parse {
        /// The target's file as declared.
        file: PathBuf,
        /// The adapter's failure.
        source: FormatError,
    },
    /// Two targets resolve to one real file.
    #[error("{file} and {other} are the same file {real}")]
    SameFile {
        /// The first target's file as declared.
        file: PathBuf,
        /// The second target's file as declared.
        other: PathBuf,
        /// The real file both resolve to.
        real: PathBuf,
    },
    /// A target's schema is invalid.
    #[error("invalid schema for {file}")]
    Schema {
        /// The target's file as declared.
        file: PathBuf,
        /// The schema's defect.
        source: SchemaError,
    },
    /// A target's declaration is invalid.
    #[error("invalid declaration for {file}")]
    Declaration {
        /// The target's file as declared.
        file: PathBuf,
        /// The declaration's defect.
        source: DeclarationError,
    },
}

/// Why `plan` failed.
#[derive(Debug, Error)]
pub enum PlanError {
    /// An adapter could not produce a target's new document.
    #[error("cannot edit {file}")]
    Edit {
        /// The target's file as declared.
        file: PathBuf,
        /// The adapter's failure.
        source: FormatError,
    },
}

/// Why `apply` failed.
#[derive(Debug, Error)]
pub enum ApplyError {
    /// The plan has unforced conflicts; nothing was written.
    #[error("the plan has {count} unforced conflicts")]
    Conflicted {
        /// Number of unforced conflicts.
        count: usize,
    },
    /// A target's new document failed verification; nothing was written.
    #[error("the new document for {file} changes more than its planned units")]
    Unverified {
        /// The target's file as declared.
        file: PathBuf,
    },
    /// A target changed since it was read; the apply stopped before it.
    #[error("{file} changed since it was read")]
    Changed {
        /// The changed target's file as declared.
        file: PathBuf,
        /// The targets replaced before the stop and every target without a write, in plan
        /// order, with their new records.
        settled: Vec<AppliedTarget>,
    },
    /// Writing a target failed; the apply stopped there.
    #[error("cannot write {file}")]
    Io {
        /// The target's file as declared.
        file: PathBuf,
        /// The I/O failure.
        source: std::io::Error,
        /// The targets replaced before the stop and every target without a write, in plan
        /// order, with their new records.
        settled: Vec<AppliedTarget>,
    },
}
