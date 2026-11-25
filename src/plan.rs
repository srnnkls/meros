use std::collections::BTreeMap;

use crate::error::PlanError;
use crate::record::Record;
use crate::unit::UnitKey;
use crate::{Snapshot, TargetSnapshot};

/// Whether planning overrides value conflicts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Force {
    /// Value conflicts stay conflicts.
    #[default]
    Off,
    /// Value conflicts are written and reported as `Outcome::Forced`.
    On,
}

/// A conflict cause that force overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCause {
    /// The unit holds a value that is not mine and differs from the declared one.
    ForeignEdit,
    /// Another writer deleted a recorded value.
    ForeignDeletion,
    /// An owned compound value to replace or remove holds a comment.
    InnerComment,
}

/// Why a unit cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// A value conflict, which force overrides.
    Value(ValueCause),
    /// Several elements or members share the unit's identity.
    Duplicate,
    /// A parent holds a non-table value.
    ScalarParent,
    /// The container is not the kind the unit's identity needs.
    WrongContainerKind,
    /// The current value has no canonical projection.
    Unhashable,
}

/// A unit that cannot be written, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The conflicting unit.
    pub unit: UnitKey,
    /// Why it conflicts.
    pub cause: Cause,
}

/// What a plan does to one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The unit's value stays as it is; its record entry is kept only while
    /// the unit is mine or seeded.
    Unchanged,
    /// The unit is written where nothing was.
    Created,
    /// The unit's owned value is replaced.
    Replaced,
    /// The unit's owned value is removed.
    Removed,
    /// The record entry is dropped; the file keeps the value.
    Released,
    /// A value conflict written under force.
    Forced(ValueCause),
    /// The unit is not written.
    Conflict(Conflict),
}

/// A non-blocking report on a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The record's scheme is missing or unknown, so it counts as no record.
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

    /// The target's non-blocking reports.
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
#[expect(clippy::todo, reason = "filled by MRS-007")]
pub fn plan(snapshot: &Snapshot, force: Force) -> Result<Plan, PlanError> {
    let _ = (snapshot, force);
    todo!()
}
