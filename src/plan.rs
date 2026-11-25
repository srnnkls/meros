use std::collections::BTreeMap;

use crate::error::PlanError;
use crate::record::Record;
use crate::unit::UnitKey;
use crate::{Snapshot, TargetSnapshot};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Force {
    #[default]
    Off,
    /// Value conflicts are written and reported as `Outcome::Forced`.
    On,
}

/// A conflict cause that force overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCause {
    ForeignEdit,
    ForeignDeletion,
    InnerComment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Value(ValueCause),
    Duplicate,
    ScalarParent,
    WrongContainerKind,
    Unhashable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub unit: UnitKey,
    pub cause: Cause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Created,
    Replaced,
    Removed,
    /// The record entry is dropped; the file keeps the value.
    Released,
    Forced(ValueCause),
    Conflict(Conflict),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    RecordSchemeUnknown,
}

/// The whole apply, built without I/O even when conflicts exist.
#[derive(Debug)]
pub struct Plan {
    pub(crate) targets: Vec<TargetPlan>,
}

/// One target's part of a plan.
#[derive(Debug)]
pub struct TargetPlan {
    pub(crate) read: TargetSnapshot,
    pub(crate) outcomes: BTreeMap<UnitKey, Outcome>,
    pub(crate) notices: Vec<Notice>,
    pub(crate) write: Option<Vec<u8>>,
    #[expect(dead_code, reason = "read by `apply`, whose body is `todo!()`")]
    pub(crate) verification: Verification,
    #[expect(dead_code, reason = "read by `apply`, whose body is `todo!()`")]
    pub(crate) record: Record,
}

/// Whether the re-parsed new document differs from the old one only in planned units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(dead_code, reason = "built by `plan`, whose body is `todo!()`")]
pub(crate) enum Verification {
    Verified,
    Failed,
}

impl Plan {
    /// Every target's outcomes and notices, in snapshot order.
    pub fn targets(&self) -> impl Iterator<Item = &TargetPlan> {
        self.targets.iter()
    }

    /// New contents of every target the apply writes.
    pub fn documents(&self) -> impl Iterator<Item = (&std::path::Path, &[u8])> {
        self.targets.iter().filter_map(|target| {
            target
                .write
                .as_deref()
                .map(|bytes| (target.read.target.file.as_path(), bytes))
        })
    }

    /// Every unforced conflict across all targets.
    pub fn conflicts(&self) -> impl Iterator<Item = &Conflict> {
        self.targets
            .iter()
            .flat_map(|target| target.outcomes.values())
            .filter_map(|outcome| match outcome {
                Outcome::Conflict(conflict) => Some(conflict),
                _ => None,
            })
    }
}

impl TargetPlan {
    /// The target's file as declared.
    #[must_use]
    pub fn file(&self) -> &std::path::Path {
        &self.read.target.file
    }

    /// Every unit's outcome, conflicts included.
    #[must_use]
    pub fn outcomes(&self) -> &BTreeMap<UnitKey, Outcome> {
        &self.outcomes
    }

    #[must_use]
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }
}

/// Decides every unit's outcome and builds each target's new document and record.
///
/// # Errors
///
/// Fails when an adapter cannot produce a target's new document.
pub fn plan(snapshot: &Snapshot, force: Force) -> Result<Plan, PlanError> {
    let _ = (snapshot, force);
    todo!()
}
