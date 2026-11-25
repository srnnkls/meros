use std::collections::{BTreeMap, BTreeSet};

use crate::error::PlanError;
use crate::format::{self, Document, Edit, Format, Step};
use crate::path::{Literal, Path, Segment};
use crate::record::{Entry, Record, Scheme};
use crate::schema::{Identity, Schema};
use crate::unit::{Current, Declared, Policy, Unit, UnitKey, derive, element_key, identify};
use crate::value::{Value, canonical_hash, canonical_json};
use crate::{FileState, Snapshot, TargetSnapshot};

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
    pub(crate) verification: Verification,
    pub(crate) record: Record,
}

/// Whether the re-parsed new document differs from the old one only in planned units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub fn plan(snapshot: &Snapshot, force: Force) -> Result<Plan, PlanError> {
    let targets = snapshot
        .targets
        .iter()
        .map(|read| plan_target(read, force))
        .collect::<Result<_, _>>()?;
    Ok(Plan { targets })
}

fn plan_target(read: &TargetSnapshot, force: Force) -> Result<TargetPlan, PlanError> {
    let target = &read.target;
    let absent = Document {
        value: Value::Map(BTreeMap::new()),
        commented: BTreeSet::new(),
        duplicated: BTreeSet::new(),
    };
    let (bytes, document) = match &read.file {
        FileState::Present {
            bytes, document, ..
        } => (bytes.as_slice(), document),
        FileState::Absent => (&[][..], &absent),
    };
    let no_record = BTreeMap::new();
    let (recorded, notices) = match target.record.scheme {
        Scheme::Meros1 => (&target.record.entries, Vec::new()),
        Scheme::Unknown => (&no_record, vec![Notice::RecordSchemeUnknown]),
    };

    let units = derive(&target.schema, &target.declaration, document, target.format);
    let current: Vec<(&Unit, Option<Vec<Hop>>)> =
        units.iter().map(|unit| (unit, hops(&unit.key))).collect();
    let mut planner = Planner::new(document, target.format, force);
    for (unit, hops) in &current {
        let entry = recorded.get(&unit.key).copied();
        planner.unit(unit, hops.as_deref(), entry);
    }
    for (key, entry) in recorded {
        if current.iter().all(|(unit, _)| unit.key != *key) {
            planner.recorded(key, *entry, &target.schema, &current);
        }
    }

    let edits = planner.edits();
    let (write, verification) = if edits.is_empty() {
        (None, Verification::Verified)
    } else {
        let written =
            format::edit(target.format, bytes, &edits).map_err(|source| PlanError::Edit {
                file: target.file.clone(),
                source,
            })?;
        let verified = !planner.unresolved
            && expected(&document.value, &edits).is_some_and(|expected| {
                format::parse(target.format, &written)
                    .is_ok_and(|parsed| same(&expected, &parsed.value, target.format))
            });
        let verification = if verified {
            Verification::Verified
        } else {
            Verification::Failed
        };
        (Some(written), verification)
    };
    Ok(TargetPlan {
        read: read.clone(),
        outcomes: planner.outcomes,
        notices,
        write,
        verification,
        record: Record {
            scheme: Scheme::Meros1,
            entries: planner.entries,
        },
    })
}

/// One step of a unit key: a map key, a keyed element, or a set element by its canonical JSON.
#[derive(Debug, Clone, PartialEq)]
enum Hop {
    Key(String),
    Select { field: String, literal: Literal },
    Element(String),
}

impl Hop {
    fn in_list(&self) -> bool {
        !matches!(self, Self::Key(_))
    }

    fn matches(&self, element: &Value, format: Format) -> bool {
        match self {
            Self::Select { field, literal } => identify(element, field).as_ref() == Some(literal),
            Self::Element(json) => element_key(element, format).as_ref() == Some(json),
            Self::Key(_) => false,
        }
    }
}

/// Parses a unit key: a path, optionally ending in a set element `[=<canonical JSON>]`.
fn hops(key: &UnitKey) -> Option<Vec<Hop>> {
    let text = key.as_str();
    let (path, element) = text
        .strip_suffix(']')
        .and_then(|inner| {
            inner.rmatch_indices("[=").find_map(|(at, _)| {
                let path = Path::parse(&text[..at]).ok()?;
                Some((path, Some(Hop::Element(inner[at + 2..].to_owned()))))
            })
        })
        .or_else(|| Some((Path::parse(text).ok()?, None)))?;
    path.segments
        .into_iter()
        .map(|segment| match segment {
            Segment::Key(key) => Some(Hop::Key(key)),
            Segment::Select { field, literal } => Some(Hop::Select { field, literal }),
            Segment::Any => None,
        })
        .chain(element.map(Some))
        .collect()
}

/// Where a unit's hops lead in a document.
enum Located<'a> {
    Present(Vec<Step>, &'a Value),
    /// The first `found` hops lead to `steps`; `length` is the list the next hop selects in.
    Missing {
        steps: Vec<Step>,
        found: usize,
        length: usize,
    },
    Duplicated,
}

fn locate<'a>(root: &'a Value, hops: &[Hop], format: Format) -> Located<'a> {
    let mut node = root;
    let mut steps = Vec::new();
    for (found, hop) in hops.iter().enumerate() {
        let next = match (hop, node) {
            (Hop::Key(key), Value::Map(members)) => members
                .get(key)
                .map(|member| (Step::Key(key.clone()), member)),
            (Hop::Select { .. } | Hop::Element(_), Value::List(elements)) => {
                let mut matching = elements
                    .iter()
                    .enumerate()
                    .filter(|(_, element)| hop.matches(element, format));
                match (matching.next(), matching.next()) {
                    (Some(_), Some(_)) => return Located::Duplicated,
                    (first, _) => first.map(|(index, element)| (Step::Index(index), element)),
                }
            }
            _ => None,
        };
        let Some((step, value)) = next else {
            let length = match node {
                Value::List(elements) => elements.len(),
                _ => 0,
            };
            return Located::Missing {
                steps,
                found,
                length,
            };
        };
        steps.push(step);
        node = value;
    }
    Located::Present(steps, node)
}

/// Whether a record entry outside the derived units is an atomic unit of the current schema:
/// neither a container, nor inside one other than through a refined element or member.
fn atomic(hops: &[Hop], schema: &Schema) -> bool {
    let mut form: Vec<Segment> = Vec::new();
    for hop in hops {
        let identity = schema
            .entries
            .iter()
            .find(|(path, _)| path.segments == form)
            .map(|(_, identity)| identity);
        match (hop, identity) {
            (Hop::Key(_), Some(Identity::Members)) => form.push(Segment::Any),
            (Hop::Select { field, .. }, Some(Identity::Keyed(keyed))) if field == keyed => {
                form.push(Segment::Any);
            }
            (Hop::Key(key), _) => form.push(Segment::Key(key.clone())),
            _ => return false,
        }
    }
    form.last() != Some(&Segment::Any)
        && !schema.entries.iter().any(|(path, _)| {
            path.segments.starts_with(&form)
                || (form.starts_with(&path.segments)
                    && form.get(path.segments.len()) != Some(&Segment::Any))
        })
}

/// A write a decision asks for.
enum Write {
    Replace(Value),
    Create(Value),
    Remove,
}

#[derive(Default)]
struct Decision {
    outcome: Option<Outcome>,
    entry: Option<Entry>,
    write: Option<Write>,
}

/// One unit's inputs to the decision table.
struct Decide<'a> {
    unit: &'a Unit,
    entry: Option<Entry>,
    commented: bool,
    force: Force,
    format: Format,
}

impl Decide<'_> {
    fn hash(&self, value: &Value) -> Option<[u8; 32]> {
        canonical_hash(value, self.format).ok()
    }

    fn mine(&self) -> bool {
        match (&self.entry, &self.unit.current) {
            (Some(entry), Current::Present(value)) => {
                self.hash(value).as_ref() == Some(entry.hash())
            }
            _ => false,
        }
    }

    fn decide(&self) -> Decision {
        match &self.unit.declared {
            None => self.undeclared(),
            Some(Declared {
                value,
                policy: Policy::CreateOnly,
            }) => self.create_only(value),
            Some(Declared {
                value,
                policy: Policy::Enforce,
            }) => self.enforce(value),
        }
    }

    fn undeclared(&self) -> Decision {
        match self.entry {
            None => Decision::default(),
            Some(Entry::Owned { .. }) if self.mine() => {
                if self.commented {
                    self.value_conflict(ValueCause::InnerComment, Write::Remove, None)
                } else {
                    decision(Outcome::Removed, None, Some(Write::Remove))
                }
            }
            Some(_) => decision(Outcome::Released, None, None),
        }
    }

    fn create_only(&self, value: &Value) -> Decision {
        if let Some(entry) = self.entry {
            let seeded = Entry::Seeded {
                hash: *entry.hash(),
            };
            return decision(Outcome::Unchanged, Some(seeded), None);
        }
        match &self.unit.current {
            Current::Absent => match self.hash(value) {
                Some(hash) => decision(
                    Outcome::Created,
                    Some(Entry::Seeded { hash }),
                    Some(Write::Create(value.clone())),
                ),
                None => self.conflict(Cause::Unhashable),
            },
            Current::UnderScalar => self.conflict(Cause::ScalarParent),
            Current::InWrongContainer => self.conflict(Cause::WrongContainerKind),
            Current::Present(_) | Current::Duplicated => decision(Outcome::Unchanged, None, None),
        }
    }

    fn enforce(&self, value: &Value) -> Decision {
        let Some(desired) = self.hash(value) else {
            return self.conflict(Cause::Unhashable);
        };
        let owned = Some(Entry::Owned { hash: desired });
        match &self.unit.current {
            Current::Duplicated => self.conflict(Cause::Duplicate),
            Current::UnderScalar => self.conflict(Cause::ScalarParent),
            Current::InWrongContainer => self.conflict(Cause::WrongContainerKind),
            Current::Present(current) => match self.hash(current) {
                None => self.conflict(Cause::Unhashable),
                Some(hash) if hash == desired => {
                    let entry = self.mine().then_some(Entry::Owned { hash });
                    decision(Outcome::Unchanged, entry, None)
                }
                Some(_) => {
                    let replace = Write::Replace(value.clone());
                    match (self.mine(), self.commented) {
                        (true, false) => decision(Outcome::Replaced, owned, Some(replace)),
                        (true, true) => {
                            self.value_conflict(ValueCause::InnerComment, replace, owned)
                        }
                        (false, _) => self.value_conflict(ValueCause::ForeignEdit, replace, owned),
                    }
                }
            },
            Current::Absent => {
                let create = Write::Create(value.clone());
                if self.entry.is_some() {
                    self.value_conflict(ValueCause::ForeignDeletion, create, owned)
                } else {
                    decision(Outcome::Created, owned, Some(create))
                }
            }
        }
    }

    /// A conflict keeps the unit's entry under the kind its current policy gives it.
    fn conflict(&self, cause: Cause) -> Decision {
        let entry = self.entry.map(|entry| match self.unit.declared {
            Some(Declared {
                policy: Policy::CreateOnly,
                ..
            }) => Entry::Seeded {
                hash: *entry.hash(),
            },
            _ => Entry::Owned {
                hash: *entry.hash(),
            },
        });
        let conflict = Conflict {
            unit: self.unit.key.clone(),
            cause,
        };
        decision(Outcome::Conflict(conflict), entry, None)
    }

    fn value_conflict(&self, cause: ValueCause, write: Write, written: Option<Entry>) -> Decision {
        match self.force {
            Force::On => decision(Outcome::Forced(cause), written, Some(write)),
            Force::Off => self.conflict(Cause::Value(cause)),
        }
    }
}

fn decision(outcome: Outcome, entry: Option<Entry>, write: Option<Write>) -> Decision {
    Decision {
        outcome: Some(outcome),
        entry,
        write,
    }
}

/// A list element created with every unit planned inside it.
struct NewElement {
    at: Vec<Step>,
    element: Option<Hop>,
    value: Value,
}

struct Planner<'a> {
    document: &'a Document,
    format: Format,
    force: Force,
    outcomes: BTreeMap<UnitKey, Outcome>,
    entries: BTreeMap<UnitKey, Entry>,
    replaces: Vec<(Vec<Step>, Value)>,
    creates: Vec<NewElement>,
    removals: Vec<(Vec<Hop>, Vec<Step>)>,
    unresolved: bool,
}

impl<'a> Planner<'a> {
    fn new(document: &'a Document, format: Format, force: Force) -> Self {
        Self {
            document,
            format,
            force,
            outcomes: BTreeMap::new(),
            entries: BTreeMap::new(),
            replaces: Vec::new(),
            creates: Vec::new(),
            removals: Vec::new(),
            unresolved: false,
        }
    }

    fn unit(&mut self, unit: &Unit, hops: Option<&[Hop]>, entry: Option<Entry>) {
        let located = hops.map(|hops| locate(&self.document.value, hops, self.format));
        let commented = matches!(
            &located,
            Some(Located::Present(steps, _)) if self.document.commented.contains(steps)
        );
        let decision = Decide {
            unit,
            entry,
            commented,
            force: self.force,
            format: self.format,
        }
        .decide();
        if let Some(outcome) = decision.outcome {
            self.outcomes.insert(unit.key.clone(), outcome);
        }
        if let Some(entry) = decision.entry {
            self.entries.insert(unit.key.clone(), entry);
        }
        let Some(write) = decision.write else {
            return;
        };
        match (write, located, hops) {
            (Write::Replace(value), Some(Located::Present(at, _)), _) => {
                self.replaces.push((at, value));
            }
            (Write::Remove, Some(Located::Present(at, _)), Some(hops)) => {
                self.removals.push((hops.to_vec(), at));
            }
            (
                Write::Create(value),
                Some(Located::Missing {
                    steps,
                    found,
                    length,
                }),
                Some(hops),
            ) => self.create(steps, &hops[found..], length, value),
            _ => self.unresolved = true,
        }
    }

    /// Plans a record entry that no derived unit matches: an atomic unit of the current
    /// schema is decided as undeclared; any other entry is stale and released.
    fn recorded(
        &mut self,
        key: &UnitKey,
        entry: Entry,
        schema: &Schema,
        current: &[(&Unit, Option<Vec<Hop>>)],
    ) {
        let hops = hops(key).filter(|hops| {
            atomic(hops, schema)
                && current.iter().all(|(_, other)| {
                    other
                        .as_ref()
                        .is_none_or(|other| !(other.starts_with(hops) || hops.starts_with(other)))
                })
        });
        let Some(hops) = hops else {
            self.outcomes.insert(key.clone(), Outcome::Released);
            return;
        };
        let current = match locate(&self.document.value, &hops, self.format) {
            Located::Present(steps, _)
                if (1..=steps.len())
                    .any(|depth| self.document.duplicated.contains(&steps[..depth])) =>
            {
                Current::Duplicated
            }
            Located::Present(_, value) => Current::Present(value.clone()),
            Located::Duplicated => Current::Duplicated,
            Located::Missing { .. } => Current::Absent,
        };
        let unit = Unit {
            key: key.clone(),
            current,
            declared: None,
        };
        self.unit(&unit, Some(&hops), Some(entry));
    }

    /// Creates `value` at `rest`, the hops missing below `steps`: missing keys become
    /// parents, and a missing list element is appended once with every unit inside it.
    fn create(&mut self, steps: Vec<Step>, rest: &[Hop], length: usize, value: Value) {
        let Some(split) = rest.iter().position(Hop::in_list) else {
            let at = steps
                .into_iter()
                .chain(rest.iter().filter_map(key_step))
                .collect();
            self.creates.push(NewElement {
                at,
                element: None,
                value,
            });
            return;
        };
        let index = if split == 0 { length } else { 0 };
        let at: Vec<Step> = steps
            .into_iter()
            .chain(rest[..split].iter().filter_map(key_step))
            .chain([Step::Index(index)])
            .collect();
        let element = &rest[split];
        let Hop::Select { field, literal } = element else {
            self.creates.push(NewElement {
                at,
                element: None,
                value,
            });
            return;
        };
        let index = self
            .creates
            .iter()
            .position(|create| create.at == at && create.element.as_ref() == Some(element))
            .unwrap_or_else(|| {
                self.creates.push(NewElement {
                    at,
                    element: Some(element.clone()),
                    value: Value::Map(BTreeMap::from([(field.clone(), literal_value(literal))])),
                });
                self.creates.len() - 1
            });
        insert(&mut self.creates[index].value, &rest[split + 1..], value);
    }

    /// Every planned edit: replaces, creates, then removals, where a refined keyed element
    /// whose units are all removed is removed whole.
    fn edits(&self) -> Vec<Edit> {
        let mut changes: Vec<Edit> = self
            .replaces
            .iter()
            .map(|(at, value)| Edit::Replace {
                at: at.clone(),
                value: value.clone(),
            })
            .chain(self.creates.iter().map(|create| Edit::Create {
                at: create.at.clone(),
                value: create.value.clone(),
            }))
            .collect();
        let touched = |element: &[Step]| {
            self.replaces
                .iter()
                .map(|(at, _)| at)
                .chain(self.creates.iter().map(|create| &create.at))
                .any(|at| at.starts_with(element))
        };

        let mut elements: Vec<(Vec<Step>, &str)> = self
            .removals
            .iter()
            .flat_map(|(hops, at)| {
                hops.iter()
                    .enumerate()
                    .take(hops.len().saturating_sub(1))
                    .filter_map(|(depth, hop)| match hop {
                        Hop::Select { field, .. } => Some((at[..=depth].to_vec(), field.as_str())),
                        _ => None,
                    })
            })
            .collect();
        elements.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        elements.dedup();
        let mut removed: Vec<Vec<Step>> = self.removals.iter().map(|(_, at)| at.clone()).collect();
        for (element, field) in elements {
            if !touched(&element)
                && value_at(&self.document.value, &element)
                    .is_some_and(|value| bare(value, field, &element, &removed))
            {
                removed.retain(|at| !at.starts_with(&element));
                removed.push(element);
            }
        }
        changes.extend(removed.into_iter().map(|at| Edit::Remove { at }));
        changes
    }
}

fn key_step(hop: &Hop) -> Option<Step> {
    match hop {
        Hop::Key(key) => Some(Step::Key(key.clone())),
        Hop::Select { .. } | Hop::Element(_) => None,
    }
}

/// Builds `value` into a new element along `hops`, adding the containers they need.
fn insert(node: &mut Value, hops: &[Hop], value: Value) {
    let Some((hop, rest)) = hops.split_first() else {
        *node = value;
        return;
    };
    match (hop, node) {
        (Hop::Key(key), Value::Map(members)) => {
            let empty = if rest.first().is_some_and(Hop::in_list) {
                Value::List(Vec::new())
            } else {
                Value::Map(BTreeMap::new())
            };
            insert(members.entry(key.clone()).or_insert(empty), rest, value);
        }
        (Hop::Select { field, literal }, Value::List(elements)) => {
            let index = elements
                .iter()
                .position(|element| identify(element, field).as_ref() == Some(literal))
                .unwrap_or_else(|| {
                    elements.push(Value::Map(BTreeMap::from([(
                        field.clone(),
                        literal_value(literal),
                    )])));
                    elements.len() - 1
                });
            insert(&mut elements[index], rest, value);
        }
        (Hop::Element(_), Value::List(elements)) => elements.push(value),
        _ => {}
    }
}

fn literal_value(literal: &Literal) -> Value {
    match literal {
        Literal::String(text) => Value::String(text.clone()),
        Literal::Integer(integer) => Value::Integer(*integer),
        Literal::Boolean(boolean) => Value::Boolean(*boolean),
    }
}

fn value_at<'a>(mut value: &'a Value, steps: &[Step]) -> Option<&'a Value> {
    for step in steps {
        value = match (step, value) {
            (Step::Key(key), Value::Map(members)) => members.get(key)?,
            (Step::Index(index), Value::List(elements)) => elements.get(*index)?,
            _ => return None,
        };
    }
    Some(value)
}

/// Whether a keyed element at `at` keeps only its identity field once `removed` is gone,
/// counting containers the removals empty as gone.
fn bare(element: &Value, field: &str, at: &[Step], removed: &[Vec<Step>]) -> bool {
    let below: Vec<&[Step]> = removed
        .iter()
        .filter_map(|removed| removed.strip_prefix(at))
        .collect();
    let Value::Map(members) = element else {
        return false;
    };
    members
        .iter()
        .filter(|(name, _)| name.as_str() != field)
        .all(|(name, member)| !remains(member, &within(&below, &Step::Key(name.clone()))))
}

fn within<'a>(removed: &[&'a [Step]], step: &Step) -> Vec<&'a [Step]> {
    removed
        .iter()
        .filter_map(|removed| match removed.split_first() {
            Some((first, rest)) if first == step => Some(rest),
            _ => None,
        })
        .collect()
}

fn remains(value: &Value, removed: &[&[Step]]) -> bool {
    if removed.is_empty() {
        return true;
    }
    if removed.iter().any(|removed| removed.is_empty()) {
        return false;
    }
    match value {
        Value::Map(members) => members
            .iter()
            .any(|(name, member)| remains(member, &within(removed, &Step::Key(name.clone())))),
        Value::List(elements) => elements
            .iter()
            .enumerate()
            .any(|(index, element)| remains(element, &within(removed, &Step::Index(index)))),
        _ => true,
    }
}

/// The document `edits` should produce, applied in `format::edit`'s order.
fn expected(read: &Value, edits: &[Edit]) -> Option<Value> {
    let mut document = read.clone();
    for edit in format::ordered(edits) {
        match edit {
            Edit::Replace { at, value } => {
                let (last, parent) = at.split_last()?;
                *child(slot(&mut document, parent)?, last)? = value.clone();
            }
            Edit::Create { at, value } => {
                let (last, parent) = at.split_last()?;
                match (last, slot_creating(&mut document, parent, last)?) {
                    (Step::Key(key), Value::Map(members)) => {
                        members.insert(key.clone(), value.clone());
                    }
                    (Step::Index(_), Value::List(elements)) => elements.push(value.clone()),
                    _ => return None,
                }
            }
            Edit::Remove { at } => {
                let (last, parent) = at.split_last()?;
                match (last, slot(&mut document, parent)?) {
                    (Step::Key(key), Value::Map(members)) => {
                        members.remove(key)?;
                    }
                    (Step::Index(index), Value::List(elements)) if *index < elements.len() => {
                        elements.remove(*index);
                    }
                    _ => return None,
                }
            }
        }
    }
    Some(document)
}

fn child<'a>(value: &'a mut Value, step: &Step) -> Option<&'a mut Value> {
    match (step, value) {
        (Step::Key(key), Value::Map(members)) => members.get_mut(key),
        (Step::Index(index), Value::List(elements)) => elements.get_mut(*index),
        _ => None,
    }
}

fn slot<'a>(mut value: &'a mut Value, steps: &[Step]) -> Option<&'a mut Value> {
    for step in steps {
        value = child(value, step)?;
    }
    Some(value)
}

/// Like `slot`, adding the maps and lists missing along `steps`.
fn slot_creating<'a>(
    mut value: &'a mut Value,
    steps: &[Step],
    last: &Step,
) -> Option<&'a mut Value> {
    for (depth, step) in steps.iter().enumerate() {
        let next = steps.get(depth + 1).unwrap_or(last);
        let empty = match next {
            Step::Key(_) => Value::Map(BTreeMap::new()),
            Step::Index(_) => Value::List(Vec::new()),
        };
        value = match (step, value) {
            (Step::Key(key), Value::Map(members)) => members.entry(key.clone()).or_insert(empty),
            (Step::Index(index), Value::List(elements)) => elements.get_mut(*index)?,
            _ => return None,
        };
    }
    Some(value)
}

/// Whether two documents hold the same values by canonical value.
fn same(a: &Value, b: &Value, format: Format) -> bool {
    match (a, b) {
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ka, va), (kb, vb))| ka == kb && same(va, vb, format))
        }
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b, format))
        }
        (Value::Float(a), Value::Float(b)) if a.is_nan() => b.is_nan(),
        _ => match (canonical_json(a, format), canonical_json(b, format)) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::{Cause, Conflict, Force, Notice, Outcome, ValueCause, plan};
    use crate::format::{self, Format};
    use crate::path::Path;
    use crate::record::{Entry, Record, Scheme};
    use crate::schema::{Identity, Schema};
    use crate::unit::{Declaration, Policy, UnitKey};
    use crate::value::{Value, canonical_hash};
    use crate::{FileState, Snapshot, Target, TargetSnapshot};

    fn text(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn toml(source: &str) -> Value {
        format::parse(Format::Toml, source.as_bytes())
            .unwrap()
            .value
    }

    fn toml_value(source: &str) -> Value {
        let Value::Map(mut members) = toml(&format!("v = {source}")) else {
            panic!("a TOML document is a map");
        };
        members.remove("v").unwrap()
    }

    fn key(text: &str) -> UnitKey {
        serde_json::from_value(serde_json::Value::String(text.to_owned())).unwrap()
    }

    fn owned(value: &Value) -> Entry {
        Entry::Owned {
            hash: canonical_hash(value, Format::Toml).unwrap(),
        }
    }

    fn seeded(value: &Value) -> Entry {
        Entry::Seeded {
            hash: canonical_hash(value, Format::Toml).unwrap(),
        }
    }

    fn record<const N: usize>(entries: [(&str, Entry); N]) -> Record {
        Record {
            scheme: Scheme::Meros1,
            entries: entries
                .into_iter()
                .map(|(text, entry)| (key(text), entry))
                .collect(),
        }
    }

    fn snapshot(
        schema: &[(&str, Identity)],
        declared: Vec<(&str, Value, Policy)>,
        record: Record,
        source: &str,
    ) -> Snapshot {
        let bytes = source.as_bytes().to_vec();
        let document = format::parse(Format::Toml, &bytes).unwrap();
        let target = Target {
            file: PathBuf::from("target.toml"),
            format: Format::Toml,
            schema: schema
                .iter()
                .map(|(text, identity)| (Path::parse(text).unwrap(), identity.clone()))
                .collect::<Schema>(),
            declaration: declared
                .into_iter()
                .map(|(text, value, policy)| (Path::parse(text).unwrap(), value, policy))
                .collect::<Declaration>(),
            record,
        };
        Snapshot {
            targets: vec![TargetSnapshot {
                target,
                real: PathBuf::from("/work/target.toml"),
                file: FileState::Present {
                    bytes,
                    permissions: PermissionsExt::from_mode(0o644),
                    document,
                },
            }],
        }
    }

    fn outcomes(plan: &super::Plan) -> BTreeMap<String, Outcome> {
        let target = plan.targets().next().unwrap();
        target
            .outcomes()
            .iter()
            .map(|(unit, outcome)| (unit.as_str().to_owned(), outcome.clone()))
            .collect()
    }

    fn entries(plan: &super::Plan) -> BTreeMap<String, Entry> {
        let target = plan.targets().next().unwrap();
        target
            .record
            .entries
            .iter()
            .map(|(unit, entry)| (unit.as_str().to_owned(), *entry))
            .collect()
    }

    fn written(plan: &super::Plan) -> Value {
        let (_, bytes) = plan.documents().next().unwrap();
        format::parse(Format::Toml, bytes).unwrap().value
    }

    fn conflict(unit: &str, cause: Cause) -> Outcome {
        Outcome::Conflict(Conflict {
            unit: key(unit),
            cause,
        })
    }

    const AS_READ: &str = r#"
equal = "same"
mine = "old"
owned_gone = "kept"
edited_gone = "user"
user = "x"
seed = "user"
relabel = "w"
seeded_gone = "s"
list = ["a"]
tool = { x = 1 }

[[repos]]
id = "ruff"
args = ["--fix"]
"#;

    const AS_PLANNED: &str = r#"
equal = "same"
mine = "new"
fresh = "f"
edited_gone = "user"
user = "x"
seed = "user"
seed_new = "n"
relabel = "w"
seeded_gone = "s"
list = ["a"]
tool = { x = 1 }
repos = [{ id = "mypy", args = ["-v"] }]
"#;

    #[test]
    fn plan_decides_declared_undeclared_stale_and_refined_units() {
        let snapshot = snapshot(
            &[
                ("repos", Identity::Keyed("id".to_owned())),
                ("repos[].args", Identity::Set),
                ("list", Identity::Set),
                ("tool", Identity::Atomic),
            ],
            vec![
                ("equal", text("same"), Policy::Enforce),
                ("mine", text("new"), Policy::Enforce),
                ("fresh", text("f"), Policy::Enforce),
                ("seed", text("s"), Policy::CreateOnly),
                ("seed_new", text("n"), Policy::CreateOnly),
                ("relabel", text("z"), Policy::CreateOnly),
                (
                    "repos",
                    toml_value(r#"[{ id = "mypy", args = ["-v"] }]"#),
                    Policy::Enforce,
                ),
            ],
            record([
                ("mine", owned(&text("old"))),
                ("owned_gone", owned(&text("kept"))),
                ("edited_gone", owned(&text("orig"))),
                ("seed", seeded(&text("s"))),
                ("relabel", owned(&text("w"))),
                ("seeded_gone", seeded(&text("s"))),
                ("list", owned(&Value::List(vec![text("a")]))),
                ("tool.x", owned(&Value::Integer(1))),
                (r#"repos[id="ruff"].args[="--fix"]"#, owned(&text("--fix"))),
            ]),
            AS_READ,
        );

        let planned = plan(&snapshot, Force::Off).unwrap();

        assert_eq!(
            outcomes(&planned),
            BTreeMap::from([
                ("equal".to_owned(), Outcome::Unchanged),
                ("mine".to_owned(), Outcome::Replaced),
                ("fresh".to_owned(), Outcome::Created),
                ("owned_gone".to_owned(), Outcome::Removed),
                ("edited_gone".to_owned(), Outcome::Released),
                ("seed".to_owned(), Outcome::Unchanged),
                ("seed_new".to_owned(), Outcome::Created),
                ("relabel".to_owned(), Outcome::Unchanged),
                ("seeded_gone".to_owned(), Outcome::Released),
                ("list".to_owned(), Outcome::Released),
                ("tool.x".to_owned(), Outcome::Released),
                (
                    r#"repos[id="ruff"].args[="--fix"]"#.to_owned(),
                    Outcome::Removed
                ),
                (
                    r#"repos[id="mypy"].args[="-v"]"#.to_owned(),
                    Outcome::Created
                ),
            ]),
        );
        assert_eq!(
            entries(&planned),
            BTreeMap::from([
                ("mine".to_owned(), owned(&text("new"))),
                ("fresh".to_owned(), owned(&text("f"))),
                ("seed".to_owned(), seeded(&text("s"))),
                ("seed_new".to_owned(), seeded(&text("n"))),
                ("relabel".to_owned(), seeded(&text("w"))),
                (
                    r#"repos[id="mypy"].args[="-v"]"#.to_owned(),
                    owned(&text("-v"))
                ),
            ]),
        );
        assert_eq!(written(&planned), toml(AS_PLANNED),);

        let again = plan(&snapshot, Force::Off).unwrap();
        assert!(planned.documents().eq(again.documents()));
        assert_eq!(
            serde_json::to_string(&planned.targets[0].record).unwrap(),
            serde_json::to_string(&again.targets[0].record).unwrap(),
        );
    }

    #[test]
    fn plan_lists_conflicts_and_forces_only_value_conflicts() {
        let snapshot = snapshot(
            &[],
            vec![
                ("foreign", text("mine"), Policy::Enforce),
                ("deleted", text("v"), Policy::Enforce),
                ("a.b", text("y"), Policy::Enforce),
            ],
            record([("deleted", owned(&text("v")))]),
            "foreign = \"user\"\na = \"x\"\n",
        );
        let scalar_parent = conflict("a.b", Cause::ScalarParent);

        let unforced = plan(&snapshot, Force::Off).unwrap();
        assert_eq!(
            outcomes(&unforced),
            BTreeMap::from([
                (
                    "foreign".to_owned(),
                    conflict("foreign", Cause::Value(ValueCause::ForeignEdit)),
                ),
                (
                    "deleted".to_owned(),
                    conflict("deleted", Cause::Value(ValueCause::ForeignDeletion)),
                ),
                ("a.b".to_owned(), scalar_parent.clone()),
            ]),
        );
        let mut listed: Vec<&str> = unforced
            .conflicts()
            .map(|conflict| conflict.unit.as_str())
            .collect();
        listed.sort_unstable();
        assert_eq!(listed, ["a.b", "deleted", "foreign"]);

        let forced = plan(&snapshot, Force::On).unwrap();
        assert_eq!(
            outcomes(&forced),
            BTreeMap::from([
                (
                    "foreign".to_owned(),
                    Outcome::Forced(ValueCause::ForeignEdit)
                ),
                (
                    "deleted".to_owned(),
                    Outcome::Forced(ValueCause::ForeignDeletion),
                ),
                ("a.b".to_owned(), scalar_parent),
            ]),
        );
        assert_eq!(
            forced
                .conflicts()
                .map(|conflict| (conflict.unit.as_str(), conflict.cause))
                .collect::<Vec<_>>(),
            [("a.b", Cause::ScalarParent)],
        );
        assert_eq!(
            written(&forced),
            toml("foreign = \"mine\"\ndeleted = \"v\"\na = \"x\"\n"),
        );
        assert_eq!(
            entries(&forced),
            BTreeMap::from([
                ("foreign".to_owned(), owned(&text("mine"))),
                ("deleted".to_owned(), owned(&text("v"))),
            ]),
        );
    }

    #[test]
    fn plan_treats_a_record_of_unknown_scheme_as_no_record() {
        let mut unknown = record([("a", owned(&text("x"))), ("b", owned(&text("old")))]);
        unknown.scheme = Scheme::Unknown;
        let snapshot = snapshot(
            &[],
            vec![("b", text("new"), Policy::Enforce)],
            unknown,
            "a = \"x\"\nb = \"old\"\n",
        );

        let planned = plan(&snapshot, Force::Off).unwrap();

        assert_eq!(
            planned.targets().next().unwrap().notices(),
            [Notice::RecordSchemeUnknown],
        );
        assert_eq!(
            outcomes(&planned),
            BTreeMap::from([(
                "b".to_owned(),
                conflict("b", Cause::Value(ValueCause::ForeignEdit)),
            )]),
        );
        assert!(entries(&planned).is_empty());
        assert_eq!(planned.documents().count(), 0);
    }

    #[test]
    fn plan_releases_a_record_entry_above_a_declared_unit() {
        let snapshot = snapshot(
            &[],
            vec![("a.b", Value::Integer(2), Policy::Enforce)],
            record([("a", owned(&toml_value("{ b = 1 }")))]),
            "a = { b = 1 }\n",
        );

        let planned = plan(&snapshot, Force::On).unwrap();

        assert_eq!(
            outcomes(&planned),
            BTreeMap::from([
                ("a".to_owned(), Outcome::Released),
                ("a.b".to_owned(), Outcome::Forced(ValueCause::ForeignEdit)),
            ]),
        );
        assert_eq!(
            entries(&planned),
            BTreeMap::from([("a.b".to_owned(), owned(&Value::Integer(2)))]),
        );
        assert_eq!(written(&planned), toml("a = { b = 2 }\n"));
    }
}
