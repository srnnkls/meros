use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::DeclarationError;
use crate::format::{Document, Format, Step, encodable};
use crate::path::{Literal, Path, Segment};
use crate::schema::{Identity, Schema};
use crate::value::{Value, canonical_hash, canonical_json};

/// How a declared unit is enforced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    /// The unit holds the declared value while it stays mine.
    Enforce,
    /// The unit is created once when absent and never changed afterwards.
    CreateOnly,
}

/// A unit's desired value and policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    /// The desired value.
    pub value: Value,
    /// How the value is enforced.
    pub policy: Policy,
}

/// Desired content for one file, expanded into units through its schema.
#[derive(Debug, Clone, Default)]
pub struct Declaration {
    entries: Vec<(Path, Declared)>,
}

impl Declaration {
    /// Fails on a declaration the schema cannot expand into units or the format cannot hold.
    pub(crate) fn validate(&self, schema: &Schema, format: Format) -> Result<(), DeclarationError> {
        for (path, declared) in &self.entries {
            if keys(&path.segments).is_none() {
                return Err(DeclarationError::Selector { path: path.clone() });
            }
            if !encodable(format, &declared.value) {
                return Err(DeclarationError::Unencodable { path: path.clone() });
            }
            if canonical_hash(&declared.value, format).is_err() {
                return Err(DeclarationError::Unprojectable { path: path.clone() });
            }
        }
        for (index, (path, _)) in self.entries.iter().enumerate() {
            if let Some((other, _)) = self.entries[index + 1..].iter().find(|(other, _)| {
                path.segments.starts_with(&other.segments)
                    || other.segments.starts_with(&path.segments)
            }) {
                return Err(DeclarationError::Overlap {
                    path: path.clone(),
                    other: other.clone(),
                });
            }
        }
        let empty = Document {
            value: Value::Map(BTreeMap::new()),
            commented: BTreeSet::new(),
            duplicated: BTreeSet::new(),
        };
        let mut walk = Walk::new(schema, &empty, format);
        walk.root(self);
        walk.error.map_or(Ok(()), Err)
    }
}

impl FromIterator<(Path, Value, Policy)> for Declaration {
    fn from_iter<I: IntoIterator<Item = (Path, Value, Policy)>>(entries: I) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(path, value, policy)| (path, Declared { value, policy }))
                .collect(),
        }
    }
}

/// A unit's canonical address: its path, plus `[field=literal]` for keyed elements
/// or `[=<typed canonical JSON>]` for set elements.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnitKey(String);

impl UnitKey {
    /// The key's canonical text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One position in a document that the tool may own.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// The unit's address.
    pub key: UnitKey,
    /// What the document holds there.
    pub current: Current,
    /// The desired value, when the unit is declared.
    pub declared: Option<Declared>,
}

/// What a document holds at a unit's position.
#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    /// Nothing is there.
    Absent,
    /// The document holds this value.
    Present(Value),
    /// Several elements or members share the unit's identity.
    Duplicated,
    /// A parent holds a non-table value.
    UnderScalar,
    /// The container is not the kind its identity needs.
    InWrongContainer,
}

/// Derives every unit the schema addresses in the document or the declaration, each with
/// what the document holds there and its declared value.
pub(crate) fn derive(
    schema: &Schema,
    declaration: &Declaration,
    document: &Document,
    format: Format,
) -> Vec<Unit> {
    let mut walk = Walk::new(schema, document, format);
    walk.root(declaration);
    walk.units.into_values().collect()
}

/// Declared values by path relative to a scope.
type Fields = Vec<(Vec<String>, Declared)>;

/// The document value at a scope or container, or what every unit below it holds instead.
type Held<'a> = Result<&'a Value, Current>;

/// A region whose units are addressed relative to it: the document root, or a keyed element
/// or member that a `[]` schema entry refines.
struct Scope<'a> {
    schema: Vec<Segment>,
    address: Vec<Segment>,
    held: Held<'a>,
    duplicated: bool,
}

/// The document copies and the declared value sharing one keyed element's address.
struct KeyedElement<'a> {
    address: Vec<Segment>,
    copies: Vec<(usize, &'a Value)>,
    declared: Option<Declared>,
}

impl KeyedElement<'_> {
    fn new(address: Vec<Segment>) -> Self {
        Self {
            address,
            copies: Vec::new(),
            declared: None,
        }
    }
}

struct Walk<'a> {
    schema: &'a Schema,
    document: &'a Document,
    format: Format,
    steps: Vec<Step>,
    units: BTreeMap<UnitKey, Unit>,
    error: Option<DeclarationError>,
}

impl<'a> Walk<'a> {
    fn new(schema: &'a Schema, document: &'a Document, format: Format) -> Self {
        Self {
            schema,
            document,
            format,
            steps: Vec::new(),
            units: BTreeMap::new(),
            error: None,
        }
    }

    fn root(&mut self, declaration: &Declaration) {
        let fields = declaration
            .entries
            .iter()
            .filter_map(|(path, declared)| Some((keys(&path.segments)?, declared.clone())))
            .collect();
        let scope = Scope {
            schema: Vec::new(),
            address: Vec::new(),
            held: Ok(&self.document.value),
            duplicated: false,
        };
        self.scope(&scope, fields);
    }

    fn reject(&mut self, error: DeclarationError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn duplicated(&self) -> bool {
        (1..=self.steps.len()).any(|depth| self.document.duplicated.contains(&self.steps[..depth]))
    }

    fn add(&mut self, key: String, current: Current, declared: Option<Declared>, duplicated: bool) {
        let current = if duplicated {
            Current::Duplicated
        } else {
            current
        };
        match self.units.entry(UnitKey(key)) {
            Entry::Vacant(entry) => {
                let key = entry.key().clone();
                entry.insert(Unit {
                    key,
                    current,
                    declared,
                });
            }
            Entry::Occupied(mut entry) => {
                let unit = entry.get_mut();
                unit.current = Current::Duplicated;
                if unit.declared.is_none() {
                    unit.declared = declared;
                }
            }
        }
    }

    /// The scope's direct containers: entries reached from it by keys alone.
    fn containers(&self, prefix: &[Segment]) -> Vec<(Vec<String>, &'a Identity)> {
        let schema: &'a Schema = self.schema;
        schema
            .entries
            .iter()
            .filter_map(|(path, identity)| {
                let path = keys(path.segments.strip_prefix(prefix)?)?;
                (!path.is_empty()).then_some((path, identity))
            })
            .collect()
    }

    /// Whether a `[]` entry refines the container's elements or members.
    fn refined(&self, container: &[Segment]) -> bool {
        self.schema.entries.iter().any(|(path, _)| {
            path.segments.len() > container.len() + 1
                && path.segments.starts_with(container)
                && path.segments[container.len()] == Segment::Any
        })
    }

    fn scope(&mut self, scope: &Scope<'a>, fields: Fields) {
        let containers = self.containers(&scope.schema);
        let mut atomic = Vec::new();
        let mut placed = vec![Vec::new(); containers.len()];
        for (path, declared) in fields {
            self.place(
                &containers,
                &scope.address,
                path,
                declared,
                &mut atomic,
                &mut placed,
            );
        }
        for (path, declared) in atomic {
            let depth = self.steps.len();
            self.steps.extend(path.iter().cloned().map(Step::Key));
            let current = if self.duplicated() {
                Current::Duplicated
            } else {
                locate(scope.held.clone(), &path)
                    .map_or_else(|held| held, |value| Current::Present(value.clone()))
            };
            self.steps.truncate(depth);
            self.add(
                key(&scope.address, &path),
                current,
                Some(declared),
                scope.duplicated,
            );
        }
        for ((path, identity), declared) in containers.into_iter().zip(placed) {
            self.container(scope, &path, identity, declared);
        }
    }

    /// Routes a declared path to the container holding it, splits a declared map whose
    /// members hold containers, and keeps every other path as one atomic unit.
    fn place(
        &mut self,
        containers: &[(Vec<String>, &Identity)],
        address: &[Segment],
        path: Vec<String>,
        declared: Declared,
        atomic: &mut Fields,
        placed: &mut [Fields],
    ) {
        let holder = containers
            .iter()
            .enumerate()
            .filter(|(_, (container, _))| path.starts_with(container))
            .min_by_key(|(_, (container, _))| container.len());
        if let Some((index, (container, _))) = holder {
            placed[index].push((path[container.len()..].to_vec(), declared));
        } else if containers
            .iter()
            .any(|(container, _)| container.len() > path.len() && container.starts_with(&path))
        {
            let Value::Map(members) = declared.value else {
                self.reject(DeclarationError::KindMismatch {
                    path: Path {
                        segments: extend(address, &path),
                    },
                    identity: Identity::Members,
                });
                return;
            };
            for (name, value) in members {
                let mut member = path.clone();
                member.push(name);
                let declared = Declared {
                    value,
                    policy: declared.policy.clone(),
                };
                self.place(containers, address, member, declared, atomic, placed);
            }
        } else {
            atomic.push((path, declared));
        }
    }

    fn container(
        &mut self,
        scope: &Scope<'a>,
        path: &[String],
        identity: &Identity,
        fields: Fields,
    ) {
        let depth = self.steps.len();
        self.steps.extend(path.iter().cloned().map(Step::Key));
        let held = match (locate(scope.held.clone(), path), identity) {
            (Ok(value), Identity::Atomic)
            | (Ok(value @ Value::List(_)), Identity::Set | Identity::Keyed(_))
            | (Ok(value @ Value::Map(_)), Identity::Members) => Ok(value),
            (Ok(_), _) => Err(Current::InWrongContainer),
            (Err(current), _) => Err(current),
        };
        let container = Scope {
            schema: extend(&scope.schema, path),
            address: extend(&scope.address, path),
            held,
            duplicated: scope.duplicated || self.duplicated(),
        };
        match identity {
            Identity::Atomic => self.whole(
                container.address,
                container.held,
                fields,
                identity,
                container.duplicated,
            ),
            Identity::Set => {
                let elements = self.elements(&container, identity, fields);
                self.set(&container, elements);
            }
            Identity::Keyed(field) => {
                let elements = self.elements(&container, identity, fields);
                self.keyed(&container, field, elements);
            }
            Identity::Members => self.members(&container, fields),
        }
        self.steps.truncate(depth);
    }

    /// The declared elements of a list container.
    fn elements(
        &mut self,
        container: &Scope<'a>,
        identity: &Identity,
        fields: Fields,
    ) -> Vec<Declared> {
        let mut elements = Vec::new();
        for (rest, Declared { value, policy }) in fields {
            match value {
                Value::List(list) if rest.is_empty() => {
                    elements.extend(list.into_iter().map(|value| Declared {
                        value,
                        policy: policy.clone(),
                    }));
                }
                _ => self.reject(DeclarationError::KindMismatch {
                    path: Path {
                        segments: extend(&container.address, &rest),
                    },
                    identity: identity.clone(),
                }),
            }
        }
        elements
    }

    fn set(&mut self, container: &Scope<'a>, declared: Vec<Declared>) {
        let path = Path {
            segments: container.address.clone(),
        };
        let mut elements: BTreeMap<String, (Current, Option<Declared>)> = BTreeMap::new();
        if let Ok(Value::List(list)) = container.held {
            for element in list {
                if let Some(json) = element_key(element, self.format) {
                    elements
                        .entry(json)
                        .and_modify(|(current, _)| *current = Current::Duplicated)
                        .or_insert_with(|| (Current::Present(element.clone()), None));
                }
            }
        }
        for element in declared {
            let Some(json) = element_key(&element.value, self.format) else {
                self.reject(DeclarationError::KindMismatch {
                    path: path.clone(),
                    identity: Identity::Set,
                });
                continue;
            };
            match elements.entry(json) {
                Entry::Vacant(entry) => {
                    entry.insert((absent(&container.held), Some(element)));
                }
                Entry::Occupied(mut entry) if entry.get().1.is_none() => {
                    entry.get_mut().1 = Some(element);
                }
                Entry::Occupied(entry) => self.reject(DeclarationError::DuplicateIdentity {
                    path: path.clone(),
                    identity: format!("{path}[={}]", entry.key()),
                }),
            }
        }
        for (json, (current, declared)) in elements {
            self.add(
                format!("{path}[={json}]"),
                current,
                declared,
                container.duplicated,
            );
        }
    }

    fn keyed(&mut self, container: &Scope<'a>, field: &str, declared: Vec<Declared>) {
        let select = |literal| {
            let mut address = container.address.clone();
            address.push(Segment::Select {
                field: field.to_owned(),
                literal,
            });
            let key = Path {
                segments: address.clone(),
            }
            .to_string();
            (key, address)
        };
        let mut elements: BTreeMap<String, KeyedElement<'a>> = BTreeMap::new();
        if let Ok(held) = container.held
            && let Value::List(list) = held
        {
            for (index, element) in list.iter().enumerate() {
                if let Some(literal) = identify(element, field) {
                    let (key, address) = select(literal);
                    elements
                        .entry(key)
                        .or_insert_with(|| KeyedElement::new(address))
                        .copies
                        .push((index, element));
                }
            }
        }
        for element in declared {
            let Some(literal) = self.declared_literal(container, field, &element) else {
                continue;
            };
            let (key, address) = select(literal);
            let entry = elements
                .entry(key.clone())
                .or_insert_with(|| KeyedElement::new(address));
            if entry.declared.is_some() {
                self.reject(DeclarationError::DuplicateIdentity {
                    path: Path {
                        segments: container.address.clone(),
                    },
                    identity: key,
                });
            } else {
                entry.declared = Some(element);
            }
        }

        let refined = self.refined(&container.schema);
        for (
            key,
            KeyedElement {
                address,
                copies,
                declared,
            },
        ) in elements
        {
            let duplicated = container.duplicated || copies.len() > 1;
            if !refined {
                let current = match copies.as_slice() {
                    [] => absent(&container.held),
                    [(_, value)] => Current::Present((*value).clone()),
                    _ => Current::Duplicated,
                };
                self.add(key, current, declared, duplicated);
                continue;
            }
            let mut fields = declared.map_or_else(Vec::new, |declared| {
                self.fields(
                    &address,
                    declared,
                    Some(field),
                    &Identity::Keyed(field.to_owned()),
                )
            });
            let mut element = Scope {
                schema: extend_any(&container.schema),
                address,
                held: Err(absent(&container.held)),
                duplicated,
            };
            if copies.is_empty() {
                self.scope(&element, fields);
                continue;
            }
            for (index, value) in copies {
                element.held = Ok(value);
                self.steps.push(Step::Index(index));
                self.scope(&element, std::mem::take(&mut fields));
                self.steps.pop();
            }
        }
    }

    /// A declared keyed element's identity literal; `None`, rejected, when it has none.
    fn declared_literal(
        &mut self,
        container: &Scope<'a>,
        field: &str,
        element: &Declared,
    ) -> Option<Literal> {
        let path = || Path {
            segments: container.address.clone(),
        };
        let kind_mismatch = || DeclarationError::KindMismatch {
            path: path(),
            identity: Identity::Keyed(field.to_owned()),
        };
        let Value::Map(members) = &element.value else {
            self.reject(kind_mismatch());
            return None;
        };
        let Some(value) = members.get(field) else {
            self.reject(DeclarationError::MissingKeyField {
                path: path(),
                field: field.to_owned(),
            });
            return None;
        };
        let literal = literal(value);
        if literal.is_none() {
            self.reject(kind_mismatch());
        }
        literal
    }

    fn members(&mut self, container: &Scope<'a>, fields: Fields) {
        let mut members: BTreeMap<String, (Option<&'a Value>, Fields)> = BTreeMap::new();
        if let Ok(held) = container.held
            && let Value::Map(map) = held
        {
            for (name, value) in map {
                members.entry(name.clone()).or_default().0 = Some(value);
            }
        }
        for (rest, declared) in fields {
            if let Some((name, rest)) = rest.split_first() {
                members
                    .entry(name.clone())
                    .or_default()
                    .1
                    .push((rest.to_vec(), declared));
            } else if let Value::Map(map) = declared.value {
                for (name, value) in map {
                    let declared = Declared {
                        value,
                        policy: declared.policy.clone(),
                    };
                    members
                        .entry(name)
                        .or_default()
                        .1
                        .push((Vec::new(), declared));
                }
            } else {
                self.reject(DeclarationError::KindMismatch {
                    path: Path {
                        segments: container.address.clone(),
                    },
                    identity: Identity::Members,
                });
            }
        }

        let refined = self.refined(&container.schema);
        for (name, (value, declared)) in members {
            let address = extend(&container.address, std::slice::from_ref(&name));
            self.steps.push(Step::Key(name));
            let duplicated = container.duplicated || self.duplicated();
            let held = value.ok_or_else(|| absent(&container.held));
            if refined {
                let mut fields = Vec::new();
                for (rest, declared) in declared {
                    if rest.is_empty() {
                        fields.extend(self.fields(&address, declared, None, &Identity::Members));
                    } else {
                        fields.push((rest, declared));
                    }
                }
                let member = Scope {
                    schema: extend_any(&container.schema),
                    address,
                    held,
                    duplicated,
                };
                self.scope(&member, fields);
            } else {
                self.whole(address, held, declared, &Identity::Members, duplicated);
            }
            self.steps.pop();
        }
    }

    /// One unit for the whole value at an address; a declared path below it is rejected.
    fn whole(
        &mut self,
        address: Vec<Segment>,
        held: Held<'a>,
        fields: Fields,
        identity: &Identity,
        duplicated: bool,
    ) {
        let mut whole = None;
        for (rest, declared) in fields {
            if rest.is_empty() {
                whole.get_or_insert(declared);
            } else {
                self.reject(DeclarationError::KindMismatch {
                    path: Path {
                        segments: extend(&address, &rest),
                    },
                    identity: identity.clone(),
                });
            }
        }
        if whole.is_none() && held.is_err() {
            return;
        }
        let current = held.map_or_else(|held| held, |value| Current::Present(value.clone()));
        self.add(
            Path { segments: address }.to_string(),
            current,
            whole,
            duplicated,
        );
    }

    /// A refined element's or member's declared fields, without its identity field.
    fn fields(
        &mut self,
        address: &[Segment],
        declared: Declared,
        identity_field: Option<&str>,
        identity: &Identity,
    ) -> Fields {
        let Value::Map(members) = declared.value else {
            self.reject(DeclarationError::KindMismatch {
                path: Path {
                    segments: address.to_vec(),
                },
                identity: identity.clone(),
            });
            return Vec::new();
        };
        members
            .into_iter()
            .filter(|(name, _)| Some(name.as_str()) != identity_field)
            .map(|(name, value)| {
                let declared = Declared {
                    value,
                    policy: declared.policy.clone(),
                };
                (vec![name], declared)
            })
            .collect()
    }
}

fn keys(segments: &[Segment]) -> Option<Vec<String>> {
    segments
        .iter()
        .map(|segment| match segment {
            Segment::Key(key) => Some(key.clone()),
            Segment::Select { .. } | Segment::Any => None,
        })
        .collect()
}

fn extend(address: &[Segment], keys: &[String]) -> Vec<Segment> {
    address
        .iter()
        .cloned()
        .chain(keys.iter().cloned().map(Segment::Key))
        .collect()
}

fn extend_any(schema: &[Segment]) -> Vec<Segment> {
    schema.iter().cloned().chain([Segment::Any]).collect()
}

fn key(address: &[Segment], keys: &[String]) -> String {
    Path {
        segments: extend(address, keys),
    }
    .to_string()
}

fn locate<'a>(mut held: Held<'a>, keys: &[String]) -> Held<'a> {
    for key in keys {
        held = match held? {
            Value::Map(members) => members.get(key).ok_or(Current::Absent),
            _ => Err(Current::UnderScalar),
        };
    }
    held
}

/// What an element or member missing from its container holds.
fn absent(container: &Held<'_>) -> Current {
    container.clone().err().unwrap_or(Current::Absent)
}

/// A set element's typed canonical JSON; `None` for compound or unprojectable values.
pub(crate) fn element_key(value: &Value, format: Format) -> Option<String> {
    if matches!(value, Value::Map(_) | Value::List(_)) {
        return None;
    }
    canonical_json(value, format).ok()
}

pub(crate) fn identify(element: &Value, field: &str) -> Option<Literal> {
    match element {
        Value::Map(members) => literal(members.get(field)?),
        _ => None,
    }
}

fn literal(value: &Value) -> Option<Literal> {
    match value {
        Value::String(text) => Some(Literal::String(text.clone())),
        Value::Integer(integer) => Some(Literal::Integer(*integer)),
        Value::Boolean(boolean) => Some(Literal::Boolean(*boolean)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{Current, Declaration, Declared, Policy, derive};
    use crate::error::DeclarationError;
    use crate::format::{Document, Format, Step};
    use crate::path::Path;
    use crate::schema::{Identity, Schema};
    use crate::value::{Datetime, Value};

    fn path(text: &str) -> Path {
        Path::parse(text).unwrap()
    }

    fn text(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn map<const N: usize>(members: [(&str, Value); N]) -> Value {
        Value::Map(
            members
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn schema(entries: &[(&str, Identity)]) -> Schema {
        entries
            .iter()
            .map(|(text, identity)| (path(text), identity.clone()))
            .collect()
    }

    fn units(
        schema: &Schema,
        declaration: &Declaration,
        document: &Document,
        format: Format,
    ) -> BTreeMap<String, (Current, Option<Declared>)> {
        let units = derive(schema, declaration, document, format);
        let count = units.len();
        let by_key: BTreeMap<_, _> = units
            .into_iter()
            .map(|unit| (unit.key.as_str().to_owned(), (unit.current, unit.declared)))
            .collect();
        assert_eq!(by_key.len(), count, "every unit key is derived once");
        by_key
    }

    fn document(value: Value) -> Document {
        Document {
            value,
            commented: BTreeSet::new(),
            duplicated: BTreeSet::new(),
        }
    }

    fn declare<const N: usize>(entries: [(&str, Value, Policy); N]) -> Declaration {
        entries
            .into_iter()
            .map(|(text, value, policy)| (path(text), value, policy))
            .collect()
    }

    fn enforce(value: Value) -> Declared {
        Declared {
            value,
            policy: Policy::Enforce,
        }
    }

    #[test]
    fn derive_keeps_atomic_paths_and_entries_whole() {
        let document = document(map([
            ("tool", map([("owner", text("you"))])),
            (
                "mcpServers",
                map([("a", map([("env", map([("X", text("0"))]))]))]),
            ),
        ]));
        let tool_owner = declare([("tool.owner", text("me"), Policy::Enforce)]);

        assert_eq!(
            units(&Schema::default(), &tool_owner, &document, Format::Toml),
            BTreeMap::from([(
                "tool.owner".to_owned(),
                (Current::Present(text("you")), Some(enforce(text("me")))),
            )]),
        );
        assert_eq!(
            units(
                &schema(&[("tool", Identity::Atomic)]),
                &tool_owner,
                &document,
                Format::Toml,
            ),
            BTreeMap::from([(
                "tool".to_owned(),
                (Current::Present(map([("owner", text("you"))])), None),
            )]),
        );
        assert_eq!(
            units(
                &schema(&[
                    ("mcpServers", Identity::Members),
                    ("mcpServers[].env", Identity::Atomic),
                ]),
                &declare([("mcpServers.a.env.X", text("1"), Policy::Enforce)]),
                &document,
                Format::Toml,
            ),
            BTreeMap::from([(
                "mcpServers.a.env".to_owned(),
                (Current::Present(map([("X", text("0"))])), None),
            )]),
        );
    }

    #[test]
    fn derive_addresses_set_elements_by_typed_canonical_json() {
        let declaration = declare([(
            "plugins",
            Value::List(vec![text("a"), Value::Float(1.0)]),
            Policy::Enforce,
        )]);
        let toml = document(map([(
            "plugins",
            Value::List(vec![
                text("a"),
                text("a"),
                Value::Integer(1),
                Value::Float(1.0),
                map([("x", Value::Integer(1))]),
                Value::List(vec![text("n")]),
                Value::Float(f64::NAN),
            ]),
        )]));
        assert_eq!(
            units(
                &schema(&[("plugins", Identity::Set)]),
                &declaration,
                &toml,
                Format::Toml,
            ),
            BTreeMap::from([
                (
                    r#"plugins[="a"]"#.to_owned(),
                    (Current::Duplicated, Some(enforce(text("a")))),
                ),
                (
                    "plugins[=1]".to_owned(),
                    (Current::Present(Value::Integer(1)), None),
                ),
                (
                    r#"plugins[={"$float":"1"}]"#.to_owned(),
                    (
                        Current::Present(Value::Float(1.0)),
                        Some(enforce(Value::Float(1.0))),
                    ),
                ),
            ]),
        );

        let json = document(map([(
            "n",
            Value::List(vec![Value::Integer(1), Value::Float(1.0)]),
        )]));
        assert_eq!(
            units(
                &schema(&[("n", Identity::Set)]),
                &Declaration::default(),
                &json,
                Format::Json,
            ),
            BTreeMap::from([("n[=1]".to_owned(), (Current::Duplicated, None))]),
        );
    }

    #[test]
    fn derive_addresses_refined_keyed_elements_by_their_fields() {
        let declaration = declare([(
            "repos",
            Value::List(vec![map([
                ("id", text("ruff")),
                ("rev", text("v1")),
                ("args", Value::List(vec![text("--fix")])),
            ])]),
            Policy::Enforce,
        )]);
        let document = document(map([(
            "repos",
            Value::List(vec![
                map([
                    ("id", text("ruff")),
                    ("rev", text("v0")),
                    ("args", Value::List(vec![text("--fix"), text("--x")])),
                ]),
                map([
                    ("id", text("mypy")),
                    ("args", Value::List(vec![text("-v")])),
                ]),
            ]),
        )]));
        assert_eq!(
            units(
                &schema(&[
                    ("repos", Identity::Keyed("id".to_owned())),
                    ("repos[].args", Identity::Set),
                ]),
                &declaration,
                &document,
                Format::Toml,
            ),
            BTreeMap::from([
                (
                    r#"repos[id="ruff"].rev"#.to_owned(),
                    (Current::Present(text("v0")), Some(enforce(text("v1")))),
                ),
                (
                    r#"repos[id="ruff"].args[="--fix"]"#.to_owned(),
                    (
                        Current::Present(text("--fix")),
                        Some(enforce(text("--fix")))
                    ),
                ),
                (
                    r#"repos[id="ruff"].args[="--x"]"#.to_owned(),
                    (Current::Present(text("--x")), None),
                ),
                (
                    r#"repos[id="mypy"].args[="-v"]"#.to_owned(),
                    (Current::Present(text("-v")), None),
                ),
            ]),
        );
    }

    #[test]
    fn derive_addresses_keyed_elements_by_literal_identity() {
        let declaration = declare([(
            "hooks",
            Value::List(vec![map([("name", text("new")), ("run", text("n"))])]),
            Policy::CreateOnly,
        )]);
        let document = document(map([(
            "hooks",
            Value::List(vec![
                map([("name", text("a")), ("run", text("x"))]),
                map([("name", text("a")), ("run", text("y"))]),
                map([("run", text("z"))]),
                map([("name", Value::Float(1.5))]),
                map([("name", map([("x", Value::Integer(1))]))]),
                text("str"),
                map([("name", Value::Integer(2)), ("run", text("w"))]),
                map([("name", Value::Boolean(true))]),
            ]),
        )]));
        assert_eq!(
            units(
                &schema(&[("hooks", Identity::Keyed("name".to_owned()))]),
                &declaration,
                &document,
                Format::Toml,
            ),
            BTreeMap::from([
                (r#"hooks[name="a"]"#.to_owned(), (Current::Duplicated, None)),
                (
                    "hooks[name=2]".to_owned(),
                    (
                        Current::Present(map([("name", Value::Integer(2)), ("run", text("w")),])),
                        None,
                    ),
                ),
                (
                    "hooks[name=true]".to_owned(),
                    (
                        Current::Present(map([("name", Value::Boolean(true))])),
                        None
                    ),
                ),
                (
                    r#"hooks[name="new"]"#.to_owned(),
                    (
                        Current::Absent,
                        Some(Declared {
                            value: map([("name", text("new")), ("run", text("n"))]),
                            policy: Policy::CreateOnly,
                        }),
                    ),
                ),
            ]),
        );
    }

    #[test]
    fn derive_addresses_members_by_name() {
        let declaration = declare([(
            "mcpServers",
            map([("a", map([("cmd", text("x"))]))]),
            Policy::Enforce,
        )]);
        let toml = document(map([(
            "mcpServers",
            map([
                ("a", map([("cmd", text("y"))])),
                ("b", map([("cmd", text("z"))])),
            ]),
        )]));
        assert_eq!(
            units(
                &schema(&[("mcpServers", Identity::Members)]),
                &declaration,
                &toml,
                Format::Toml,
            ),
            BTreeMap::from([
                (
                    "mcpServers.a".to_owned(),
                    (
                        Current::Present(map([("cmd", text("y"))])),
                        Some(enforce(map([("cmd", text("x"))]))),
                    ),
                ),
                (
                    "mcpServers.b".to_owned(),
                    (Current::Present(map([("cmd", text("z"))])), None),
                ),
            ]),
        );

        let json = Document {
            duplicated: BTreeSet::from([vec![
                Step::Key("env".to_owned()),
                Step::Key("X".to_owned()),
            ]]),
            ..document(map([("env", map([("X", text("1")), ("Y", text("2"))]))]))
        };
        assert_eq!(
            units(
                &schema(&[("env", Identity::Members)]),
                &Declaration::default(),
                &json,
                Format::Json,
            ),
            BTreeMap::from([
                ("env.X".to_owned(), (Current::Duplicated, None)),
                ("env.Y".to_owned(), (Current::Present(text("2")), None)),
            ]),
        );
    }

    #[test]
    fn derive_marks_units_in_a_container_of_the_wrong_kind() {
        let declaration = declare([("extras", Value::List(vec![text("e")]), Policy::Enforce)]);
        let document = document(map([("extras", map([("k", Value::Integer(1))]))]));
        assert_eq!(
            units(
                &schema(&[("extras", Identity::Set)]),
                &declaration,
                &document,
                Format::Toml,
            ),
            BTreeMap::from([(
                r#"extras[="e"]"#.to_owned(),
                (Current::InWrongContainer, Some(enforce(text("e")))),
            )]),
        );
    }

    fn containers() -> Schema {
        schema(&[
            ("plugins", Identity::Set),
            ("repos", Identity::Keyed("id".to_owned())),
            ("repos[].args", Identity::Set),
            ("mcpServers", Identity::Members),
        ])
    }

    fn validate_against(
        schema: &Schema,
        format: Format,
        declared: Vec<(&str, Value)>,
    ) -> Result<(), DeclarationError> {
        declared
            .into_iter()
            .map(|(text, value)| (path(text), value, Policy::Enforce))
            .collect::<Declaration>()
            .validate(schema, format)
    }

    fn validate(format: Format, declared: Vec<(&str, Value)>) -> Result<(), DeclarationError> {
        validate_against(&containers(), format, declared)
    }

    fn datetime() -> Value {
        Value::Datetime(Datetime::parse("1979-05-27T07:32:00Z").unwrap())
    }

    fn ruff() -> Value {
        map([("id", text("ruff"))])
    }

    #[test]
    fn validate_accepts_declarations_the_schema_and_format_take() {
        assert_eq!(
            validate(
                Format::Toml,
                vec![
                    (
                        "repos",
                        Value::List(vec![map([
                            ("id", text("ruff")),
                            ("rev", text("v1")),
                            ("args", Value::List(vec![text("--fix")])),
                        ])]),
                    ),
                    (
                        "plugins",
                        Value::List(vec![text("a"), Value::Integer(1), Value::Float(1.0)]),
                    ),
                    ("mcpServers.a", map([("cmd", text("x"))])),
                    ("when", datetime()),
                    ("tool.owner", text("me")),
                ],
            ),
            Ok(()),
        );
        assert_eq!(validate(Format::Json, vec![("x", Value::Null)]), Ok(()));
    }

    #[test]
    fn validate_rejects_overlapping_paths() {
        for (format, declared, overlapping) in [
            (
                Format::Toml,
                vec![
                    ("tool", map([("owner", text("me"))])),
                    ("tool.owner", text("me")),
                ],
                ["tool", "tool.owner"],
            ),
            (
                Format::Json,
                vec![
                    ("mcpServers", map([("a", map([]))])),
                    ("mcpServers.a", map([])),
                ],
                ["mcpServers", "mcpServers.a"],
            ),
        ] {
            let error = validate(format, declared).unwrap_err();
            let DeclarationError::Overlap { path, other } = &error else {
                panic!("{overlapping:?} must overlap, got {error:?}");
            };
            assert_eq!(
                BTreeSet::from([path.to_string(), other.to_string()]),
                BTreeSet::from(overlapping.map(str::to_owned)),
            );
        }
    }

    #[test]
    fn validate_rejects_duplicate_identities() {
        for (format, declared, container) in [
            (
                Format::Toml,
                ("plugins", Value::List(vec![text("a"), text("a")])),
                "plugins",
            ),
            (
                Format::Json,
                (
                    "plugins",
                    Value::List(vec![Value::Integer(1), Value::Float(1.0)]),
                ),
                "plugins",
            ),
            (
                Format::Toml,
                ("repos", Value::List(vec![ruff(), ruff()])),
                "repos",
            ),
        ] {
            let at = path(container);
            let error = validate(format, vec![declared]).unwrap_err();
            assert!(
                matches!(&error, DeclarationError::DuplicateIdentity { path: duplicated, .. } if duplicated == &at),
                "{container} in {format:?} must hold a duplicate identity, got {error:?}",
            );
        }
    }

    #[test]
    fn validate_rejects_values_that_do_not_fit_their_identity() {
        for (declared, identity) in [
            (("plugins", text("a")), Identity::Set),
            (
                (
                    "plugins",
                    Value::List(vec![map([("x", Value::Integer(1))])]),
                ),
                Identity::Set,
            ),
            (
                ("repos", Value::List(vec![text("ruff")])),
                Identity::Keyed("id".to_owned()),
            ),
            (
                ("mcpServers", Value::List(vec![text("a")])),
                Identity::Members,
            ),
        ] {
            let at = path(declared.0);
            assert_eq!(
                validate(Format::Toml, vec![declared]),
                Err(DeclarationError::KindMismatch { path: at, identity }),
            );
        }

        for (entries, declared) in [
            (vec![("tool", Identity::Atomic)], "tool.owner"),
            (
                vec![
                    ("mcpServers", Identity::Members),
                    ("mcpServers[].env", Identity::Atomic),
                ],
                "mcpServers.a.env.X",
            ),
        ] {
            assert_eq!(
                validate_against(
                    &schema(&entries),
                    Format::Toml,
                    vec![(declared, text("me"))],
                ),
                Err(DeclarationError::KindMismatch {
                    path: path(declared),
                    identity: Identity::Atomic,
                }),
            );
        }
    }

    #[test]
    fn validate_rejects_keyed_elements_without_their_key_field() {
        assert_eq!(
            validate(
                Format::Toml,
                vec![("repos", Value::List(vec![map([("rev", text("v1"))])]))],
            ),
            Err(DeclarationError::MissingKeyField {
                path: path("repos"),
                field: "id".to_owned(),
            }),
        );
    }

    #[test]
    fn validate_rejects_selectors() {
        assert_eq!(
            validate(Format::Toml, vec![(r#"repos[id="ruff"].rev"#, text("v1"))]),
            Err(DeclarationError::Selector {
                path: path(r#"repos[id="ruff"].rev"#),
            }),
        );
    }

    #[test]
    fn validate_rejects_values_the_format_cannot_encode() {
        for (format, value) in [
            (Format::Toml, Value::Null),
            (Format::Toml, map([("a", Value::Null)])),
            (Format::Json, datetime()),
            (Format::Yaml, Value::List(vec![datetime()])),
        ] {
            assert_eq!(
                validate(format, vec![("x", value.clone())]),
                Err(DeclarationError::Unencodable { path: path("x") }),
                "{value:?} in {format:?}",
            );
        }
    }

    #[test]
    fn validate_rejects_unprojectable_values() {
        for (format, value) in [
            (Format::Toml, Value::Float(f64::NAN)),
            (Format::Toml, Value::List(vec![Value::Float(f64::INFINITY)])),
            (Format::Yaml, Value::Integer(9_007_199_254_740_993)),
            (
                Format::Json,
                map([("n", Value::BigInteger("100000000000000000000".to_owned()))]),
            ),
        ] {
            assert_eq!(
                validate(format, vec![("x", value.clone())]),
                Err(DeclarationError::Unprojectable { path: path("x") }),
                "{value:?} in {format:?}",
            );
        }
    }
}
