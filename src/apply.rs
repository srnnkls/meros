use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::ApplyError;
use crate::plan::{Notice, Outcome, Plan};
use crate::record::Record;
use crate::unit::UnitKey;

/// The result of an apply.
#[derive(Debug)]
pub struct Applied {
    /// Every target in plan order.
    pub targets: Vec<AppliedTarget>,
}

/// A settled target: its new record, also when its record changed without a write.
#[derive(Debug)]
pub struct AppliedTarget {
    /// The target's file as declared.
    pub file: PathBuf,
    /// The record for the caller to store.
    pub record: Record,
    /// Every unit's outcome.
    pub outcomes: BTreeMap<UnitKey, Outcome>,
    /// The target's non-blocking reports.
    pub notices: Vec<Notice>,
}

/// Writes every changed target atomically after rechecking that none changed since read.
///
/// # Errors
///
/// Refuses a plan with an unforced conflict or a failed verification before any write;
/// stops at the first target found changed or failing to write, carrying the records of
/// the targets already settled.
#[expect(clippy::todo, reason = "filled by MRS-011")]
pub fn apply(plan: Plan) -> Result<Applied, ApplyError> {
    drop(plan);
    todo!()
}
