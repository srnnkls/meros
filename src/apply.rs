use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::ApplyError;
use crate::plan::{Notice, Outcome, Plan};
use crate::record::Record;
use crate::unit::UnitKey;

#[derive(Debug)]
pub struct Applied {
    pub targets: Vec<AppliedTarget>,
}

/// A settled target: its new record, also when its record changed without a write.
#[derive(Debug)]
pub struct AppliedTarget {
    pub file: PathBuf,
    pub record: Record,
    pub outcomes: BTreeMap<UnitKey, Outcome>,
    pub notices: Vec<Notice>,
}

/// Writes every changed target atomically after rechecking that none changed since read.
///
/// # Errors
///
/// Refuses a plan with an unforced conflict or a failed verification before any write;
/// stops at the first target found changed or failing to write, carrying the records of
/// the targets already settled.
pub fn apply(plan: Plan) -> Result<Applied, ApplyError> {
    drop(plan);
    todo!()
}
