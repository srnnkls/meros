//! Ownership of declared values inside structured files that other writers share.

mod apply;
mod error;
mod format;
mod path;
mod plan;
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
pub use path::{Literal, Path, Segment};
pub use plan::{Cause, Conflict, Force, Notice, Outcome, Plan, TargetPlan, ValueCause, plan};
pub use record::{Entry, Record, Scheme};
pub use schema::{Identity, Schema};
pub use unit::{Current, Declaration, Declared, Policy, Unit, UnitKey};
pub use value::{Datetime, DatetimeKind, Value};

use format::Document;

#[derive(Debug, Clone)]
pub struct Target {
    pub file: PathBuf,
    pub format: Format,
    pub schema: Schema,
    pub declaration: Declaration,
    pub record: Record,
}

/// Targets as read, resolved and validated.
#[derive(Debug)]
pub struct Snapshot {
    #[expect(dead_code, reason = "read by `plan`, whose body is `todo!()`")]
    targets: Vec<TargetSnapshot>,
}

#[derive(Debug)]
struct TargetSnapshot {
    target: Target,
    #[expect(dead_code, reason = "read by `apply`, whose body is `todo!()`")]
    real: PathBuf,
    #[expect(
        dead_code,
        reason = "read by `plan` and `apply`, whose bodies are `todo!()`"
    )]
    file: FileState,
}

#[derive(Debug)]
#[expect(dead_code, reason = "built by `read`, whose body is `todo!()`")]
enum FileState {
    Absent,
    Present {
        bytes: Vec<u8>,
        permissions: Permissions,
        document: Document,
    },
}

/// Resolves, reads and parses every target and validates its schema and declaration.
///
/// # Errors
///
/// Fails when a target cannot be read or parsed, two targets resolve to one real file,
/// or a schema or declaration is invalid.
pub fn read(targets: Vec<Target>) -> Result<Snapshot, ReadError> {
    drop(targets);
    todo!()
}
