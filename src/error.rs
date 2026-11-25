use std::path::PathBuf;

use thiserror::Error;

use crate::apply::AppliedTarget;
use crate::path::Path;
use crate::schema::Identity;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    #[error("path `{path}`: index selector at byte {at}")]
    IndexSelector { path: String, at: usize },
    #[error("path `{path}`: invalid syntax at byte {at}")]
    Syntax { path: String, at: usize },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchemaError {
    #[error("schema entry {path:?} lies inside the atomic unit {atomic:?}")]
    NestedInAtomic { path: Path, atomic: Path },
}

#[derive(Debug, Error, PartialEq)]
pub enum DeclarationError {
    #[error("declared paths {path:?} and {other:?} overlap")]
    Overlap { path: Path, other: Path },
    #[error("declaration {path:?} holds the identity `{identity}` twice")]
    DuplicateIdentity { path: Path, identity: String },
    #[error("declaration {path:?} does not fit its identity {identity:?}")]
    KindMismatch { path: Path, identity: Identity },
    #[error("declaration {path:?} has an element without its key field `{field}`")]
    MissingKeyField { path: Path, field: String },
    #[error("declaration {path:?} uses a selector")]
    Selector { path: Path },
    #[error("declaration {path:?} holds a value its format cannot encode")]
    Unencodable { path: Path },
    #[error("declaration {path:?} holds NaN, an infinity or an integer beyond 2^53")]
    Unprojectable { path: Path },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("{message}")]
    Syntax { message: String },
    #[error("unsupported YAML: {0:?}")]
    UnsupportedYaml(YamlFeature),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlFeature {
    Comment,
    Anchor,
    Alias,
    Tag,
    DuplicateKey,
    MultipleDocuments,
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("cannot read {file}: {source}")]
    Io {
        file: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse {file}: {source}")]
    Parse { file: PathBuf, source: FormatError },
    #[error("{file} and {other} are the same file {real}")]
    SameFile {
        file: PathBuf,
        other: PathBuf,
        real: PathBuf,
    },
    #[error("invalid schema for {file}: {source}")]
    Schema { file: PathBuf, source: SchemaError },
    #[error("invalid declaration for {file}: {source}")]
    Declaration {
        file: PathBuf,
        source: DeclarationError,
    },
}

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("cannot edit {file}: {source}")]
    Edit { file: PathBuf, source: FormatError },
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("the plan has {count} unforced conflicts")]
    Conflicted { count: usize },
    #[error("the new document for {file} changes more than its planned units")]
    Unverified { file: PathBuf },
    #[error("{file} changed since it was read")]
    Changed {
        file: PathBuf,
        settled: Vec<AppliedTarget>,
    },
    #[error("cannot write {file}: {source}")]
    Io {
        file: PathBuf,
        source: std::io::Error,
        settled: Vec<AppliedTarget>,
    },
}
