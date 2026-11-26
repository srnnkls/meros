#![doc = include_str!("../README.md")]

mod apply;
mod error;
mod format;
mod fs;
mod path;
mod plan;
mod read;
mod record;
mod schema;
mod unit;
mod value;

use std::fs::Permissions;
use std::path::PathBuf;

pub use apply::{Applied, AppliedTarget, apply};
pub use error::{
    ApplyError, DeclarationError, FormatError, PathError, PlanError, ReadError, SchemaError,
    YamlFeature,
};
pub use format::Format;
pub use path::Path;
pub use plan::{Cause, Conflict, Force, Notice, Outcome, Plan, TargetPlan, ValueCause, plan};
pub use read::read;
pub use record::{Entry, Record, Scheme};
pub use schema::{Identity, Schema};
pub use unit::{Current, Declaration, Declared, Policy, Unit, UnitKey};
pub use value::{Datetime, DatetimeKind, Value};

use format::Document;

/// One file to manage.
#[derive(Debug, Clone)]
pub struct Target {
    /// The file as the caller names it; a symlink is resolved when read.
    pub file: PathBuf,
    /// How the file is parsed and edited.
    pub format: Format,
    /// Identity per container path.
    pub schema: Schema,
    /// The desired units.
    pub declaration: Declaration,
    /// What the tool last wrote in the file.
    pub record: Record,
}

/// Targets as read, resolved and validated.
#[derive(Debug)]
pub struct Snapshot {
    targets: Vec<TargetSnapshot>,
}

#[derive(Debug, Clone)]
struct TargetSnapshot {
    target: Target,
    real: PathBuf,
    file: FileState,
}

#[derive(Debug, Clone)]
enum FileState {
    Absent,
    Present {
        bytes: Vec<u8>,
        permissions: Permissions,
        document: Document,
    },
}
