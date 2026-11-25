use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use toml_edit::{
    Array, ArrayOfTables, Decor, DocumentMut, InlineTable, Item, Key, RawString, Table, TableLike,
};

use super::{Document, Edit, Step};
use crate::error::FormatError;
use crate::value::{Datetime, Value};

type Toml = toml_edit::Value;

pub(crate) fn parse(bytes: &[u8]) -> Result<Document, FormatError> {
    let mut reader = Reader::default();
    let (value, _) = reader.item(document(bytes)?.as_item());
    Ok(Document {
        value,
        commented: reader.commented,
        // TOML rejects duplicate keys while parsing.
        duplicated: BTreeSet::new(),
    })
}

pub(crate) fn edit<'a>(
    bytes: &[u8],
    edits: impl IntoIterator<Item = &'a Edit>,
) -> Result<Vec<u8>, FormatError> {
    let mut document = document(bytes)?;
    for edit in edits {
        apply(&mut document, edit).ok_or(FormatError::UnfitEdit)?;
    }
    Ok(document.to_string().into_bytes())
}

pub(crate) fn encodable(value: &Value) -> bool {
    encode(value).is_some()
}

fn document(bytes: &[u8]) -> Result<DocumentMut, FormatError> {
    std::str::from_utf8(bytes)
        .map_err(syntax)?
        .parse()
        .map_err(syntax)
}

fn syntax(error: impl Display) -> FormatError {
    FormatError::Syntax {
        message: error.to_string(),
    }
}

#[derive(Default)]
struct Reader {
    at: Vec<Step>,
    commented: BTreeSet<Vec<Step>>,
}

impl Reader {
    fn item(&mut self, item: &Item) -> (Value, bool) {
        match item {
            Item::Value(value) => self.value(value),
            Item::Table(table) => {
                let (members, inside) = self.entries(table);
                self.mark(members, inside)
            }
            Item::ArrayOfTables(tables) => {
                let mut inside = false;
                let mut elements = Vec::new();
                for (index, table) in tables.iter().enumerate() {
                    let (element, commented) = self.within(Step::Index(index), |reader| {
                        let (members, inside) = reader.entries(table);
                        reader.mark(members, inside)
                    });
                    inside |= commented || index > 0 && commented_decor(table.decor());
                    elements.push(element);
                }
                self.mark(Value::List(elements), inside)
            }
            Item::None => unreachable!("table iteration skips removed items"),
        }
    }

    fn value(&mut self, value: &Toml) -> (Value, bool) {
        match value {
            Toml::String(text) => (Value::String(text.value().clone()), false),
            Toml::Integer(integer) => (Value::Integer(*integer.value()), false),
            Toml::Float(float) => (Value::Float(*float.value()), false),
            Toml::Boolean(boolean) => (Value::Boolean(*boolean.value()), false),
            Toml::Datetime(datetime) => {
                #[expect(
                    clippy::expect_used,
                    reason = "toml_edit parses only datetimes of TOML's four kinds"
                )]
                let datetime =
                    Datetime::parse(&datetime.value().to_string()).expect("a TOML datetime");
                (Value::Datetime(datetime), false)
            }
            Toml::Array(array) => {
                let mut inside = commented_raw(array.trailing());
                let mut elements = Vec::new();
                for (index, element) in array.iter().enumerate() {
                    let (element_value, commented) =
                        self.within(Step::Index(index), |reader| reader.value(element));
                    inside |= commented || commented_decor(element.decor());
                    elements.push(element_value);
                }
                self.mark(Value::List(elements), inside)
            }
            Toml::InlineTable(table) => {
                let (members, inside) = self.entries(table);
                self.mark(members, inside || commented_raw(table.trailing()))
            }
        }
    }

    fn entries(&mut self, table: &dyn TableLike) -> (Value, bool) {
        let mut inside = false;
        let mut members = BTreeMap::new();
        for (key, item) in table.iter() {
            inside |= table.key(key).is_some_and(|key| {
                commented_decor(key.leaf_decor()) || commented_decor(key.dotted_decor())
            });
            inside |= match item {
                Item::Value(value) => commented_decor(value.decor()),
                Item::Table(table) => commented_decor(table.decor()),
                Item::ArrayOfTables(tables) => tables
                    .get(0)
                    .is_some_and(|table| commented_decor(table.decor())),
                Item::None => false,
            };
            let (member, commented) =
                self.within(Step::Key(key.to_owned()), |reader| reader.item(item));
            inside |= commented;
            members.insert(key.to_owned(), member);
        }
        (Value::Map(members), inside)
    }

    fn within<T>(&mut self, step: Step, read: impl FnOnce(&mut Self) -> T) -> T {
        self.at.push(step);
        let read = read(self);
        self.at.pop();
        read
    }

    fn mark(&mut self, value: Value, inside: bool) -> (Value, bool) {
        if inside {
            self.commented.insert(self.at.clone());
        }
        (value, inside)
    }
}

fn commented_decor(decor: &Decor) -> bool {
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .any(commented_raw)
}

fn commented_raw(raw: &RawString) -> bool {
    raw.as_str().is_some_and(|text| text.contains('#'))
}

fn apply(document: &mut DocumentMut, edit: &Edit) -> Option<()> {
    match edit {
        Edit::Replace { at, value } => replace(document.as_item_mut(), at, value),
        Edit::Create { at, value } => create(document.as_item_mut(), at, value),
        Edit::Remove { at } => remove(document, at),
    }
}

fn replace(document: &mut Item, at: &[Step], value: &Value) -> Option<()> {
    let item = locate(document, at)?;
    let leading = match (&mut *item, value) {
        (Item::Table(table), Value::Map(members)) => {
            table.clear();
            return fill(table, members);
        }
        (Item::Value(old), _) => {
            let decor = old.decor().clone();
            *old = encode(value)?;
            *old.decor_mut() = decor;
            return Some(());
        }
        (Item::ArrayOfTables(old), Value::List(elements))
            if !elements.is_empty()
                && elements
                    .iter()
                    .all(|element| matches!(element, Value::Map(_))) =>
        {
            *old = tables(old.get(0), elements)?;
            return Some(());
        }
        (Item::Table(table), _) => comments(table.decor()),
        (Item::ArrayOfTables(old), _) => old
            .get(0)
            .map(|first| comments(first.decor()))
            .unwrap_or_default(),
        (Item::None, _) => return None,
    };
    // The value changes representation, so its key takes the default spacing.
    let Some((Step::Key(key), parents)) = at.split_last() else {
        return None;
    };
    let mut replaced = header(value)?;
    let line = match &mut replaced {
        Item::Table(table) => {
            prepend(table.decor_mut(), &leading);
            String::new()
        }
        _ => leading,
    };
    *item = replaced;
    let mut key = locate(document, parents)?
        .as_table_like_mut()?
        .key_mut(key)?;
    let decor = key.leaf_decor_mut();
    decor.clear();
    prepend(decor, &line);
    Some(())
}

/// An array of tables holding `elements`, the first under the header decor of `first`.
fn tables(first: Option<&Table>, elements: &[Value]) -> Option<ArrayOfTables> {
    let mut tables = ArrayOfTables::new();
    for element in elements {
        let Value::Map(members) = element else {
            return None;
        };
        let mut table = Table::new();
        fill(&mut table, members)?;
        tables.push(table);
    }
    if let (Some(first), Some(table)) = (first, tables.get_mut(0)) {
        *table.decor_mut() = first.decor().clone();
        table.set_position(first.position());
    }
    Some(tables)
}

fn create(document: &mut Item, at: &[Step], value: &Value) -> Option<()> {
    let (last, parents) = at.split_last()?;
    let mut parent = document;
    for (depth, step) in parents.iter().enumerate() {
        parent = container(parent, step, vacant(&at[depth + 1..], value))?;
    }
    match (parent, last) {
        (Item::Table(table), Step::Key(key)) => append(table, key, header(value)?),
        (Item::ArrayOfTables(tables), Step::Index(_)) => {
            let Value::Map(members) = value else {
                return None;
            };
            let mut table = Table::new();
            fill(&mut table, members)?;
            tables.push(table);
        }
        (Item::Value(Toml::InlineTable(table)), Step::Key(key)) => {
            append_inline(table, key, encode(value)?);
        }
        (Item::Value(Toml::Array(array)), Step::Index(_)) => push(array, encode(value)?),
        _ => return None,
    }
    Some(())
}

/// The container a create makes for a missing parent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Vacant {
    Table,
    Tables,
    Array,
}

impl Vacant {
    fn item(self) -> Item {
        match self {
            Self::Table => Item::Table(Table::new()),
            Self::Tables => Item::ArrayOfTables(ArrayOfTables::new()),
            Self::Array => Item::Value(Toml::Array(Array::new())),
        }
    }

    fn value(self) -> Toml {
        match self {
            Self::Table => Toml::InlineTable(InlineTable::new()),
            Self::Tables | Self::Array => Toml::Array(Array::new()),
        }
    }
}

/// The container for a missing parent of the steps `below` it, leading to `value`.
fn vacant(below: &[Step], value: &Value) -> Vacant {
    match below {
        [Step::Key(_), ..] => Vacant::Table,
        [Step::Index(_), Step::Key(_), ..] => Vacant::Tables,
        [Step::Index(_)] if matches!(value, Value::Map(_)) => Vacant::Tables,
        _ => Vacant::Array,
    }
}

fn container<'a>(item: &'a mut Item, step: &Step, vacant: Vacant) -> Option<&'a mut Item> {
    let missing = match step {
        Step::Key(key) => item.get(key.as_str()).is_none(),
        Step::Index(index) => item.get(*index).is_none(),
    };
    if missing {
        match (&mut *item, step) {
            (Item::Table(table), Step::Key(key)) => append(table, key, vacant.item()),
            (Item::Value(Toml::InlineTable(table)), Step::Key(key)) => {
                append_inline(table, key, vacant.value());
            }
            (Item::ArrayOfTables(tables), Step::Index(_)) if vacant == Vacant::Table => {
                tables.push(Table::new());
            }
            (Item::Value(Toml::Array(array)), Step::Index(_)) => push(array, vacant.value()),
            _ => return None,
        }
    }
    child(item, step)
}

fn locate<'a>(mut item: &'a mut Item, at: &[Step]) -> Option<&'a mut Item> {
    for step in at {
        item = child(item, step)?;
    }
    Some(item)
}

fn child<'a>(item: &'a mut Item, step: &Step) -> Option<&'a mut Item> {
    match step {
        Step::Key(key) => item.as_table_like_mut()?.get_mut(key),
        Step::Index(index) => item.get_mut(*index),
    }
}

fn append(table: &mut Table, key: &str, item: Item) {
    let indentation = table
        .iter()
        .filter(|(_, sibling)| sibling.is_value())
        .last()
        .and_then(|(sibling, _)| table.key(sibling)?.leaf_decor().prefix()?.as_str())
        .map(|prefix| prefix.rsplit('\n').next().unwrap_or_default().to_owned());
    let indented = item.is_value();
    table.insert(key, item);
    if let (true, Some(indentation), Some(mut key)) = (indented, indentation, table.key_mut(key)) {
        key.leaf_decor_mut().set_prefix(indentation);
    }
}

fn append_inline(table: &mut InlineTable, key: &str, mut value: Toml) {
    let mut formatted = Key::new(key);
    if let Some(last) = table.iter().last().map(|(last, _)| last.to_owned())
        && let Some((last_key, Item::Value(last_value))) = table.get_key_value_mut(&last)
    {
        formatted = formatted.with_leaf_decor(last_key.leaf_decor().clone());
        *value.decor_mut() = last_value.decor().clone();
        last_value.decor_mut().set_suffix("");
    }
    table.insert_formatted(&formatted, value);
}

fn push(array: &mut Array, mut value: Toml) {
    let line = array
        .iter()
        .last()
        .and_then(|last| last.decor().prefix()?.as_str())
        .and_then(|prefix| prefix.rfind('\n').map(|start| prefix[start..].to_owned()));
    match line {
        Some(line) => {
            value.decor_mut().set_prefix(line);
            array.push_formatted(value);
        }
        None => array.push(value),
    }
}

fn header(value: &Value) -> Option<Item> {
    Some(match value {
        Value::Map(members) => {
            let mut table = Table::new();
            fill(&mut table, members)?;
            Item::Table(table)
        }
        _ => Item::Value(encode(value)?),
    })
}

fn fill(table: &mut Table, members: &BTreeMap<String, Value>) -> Option<()> {
    for (key, member) in members {
        table.insert(key, header(member)?);
    }
    Some(())
}

fn encode(value: &Value) -> Option<Toml> {
    Some(match value {
        Value::Map(members) => Toml::InlineTable(
            members
                .iter()
                .map(|(key, member)| Some((key, encode(member)?)))
                .collect::<Option<_>>()?,
        ),
        Value::List(elements) => Toml::Array(elements.iter().map(encode).collect::<Option<_>>()?),
        Value::String(text) => Toml::from(text.as_str()),
        Value::Integer(integer) => Toml::from(*integer),
        Value::Float(float) => Toml::from(*float),
        Value::Boolean(boolean) => Toml::from(*boolean),
        Value::Datetime(datetime) => {
            Toml::from(datetime.as_str().parse::<toml_edit::Datetime>().ok()?)
        }
        Value::BigInteger(_) | Value::Null => return None,
    })
}

/// Leading comments of a removed unit that no next key in its table takes, placed above
/// the first header after `after`.
#[derive(Default)]
struct Carried {
    comments: String,
    after: Option<isize>,
}

fn remove(document: &mut DocumentMut, at: &[Step]) -> Option<()> {
    let (last, parents) = at.split_last()?;
    let carried = match (locate(document.as_item_mut(), parents)?, last) {
        (Item::Table(table), Step::Key(key)) => remove_entry(table, key)?,
        (Item::Value(Toml::InlineTable(table)), Step::Key(key)) => {
            remove_inline(table, key)?;
            Carried::default()
        }
        (Item::Value(Toml::Array(array)), Step::Index(index)) => {
            remove_element(array, *index)?;
            Carried::default()
        }
        (Item::ArrayOfTables(tables), Step::Index(index)) => {
            let carried = remove_table(tables, *index)?;
            if tables.is_empty() {
                keep_empty(document, parents)?;
            }
            carried
        }
        _ => return None,
    };
    if !carried.comments.is_empty() {
        place(document, &carried);
    }
    Some(())
}

fn remove_entry(table: &mut Table, key: &str) -> Option<Carried> {
    let next = next_line(table, key);
    let (removed_key, removed) = table.remove_entry(key)?;
    if table.is_empty() {
        table.set_implicit(false);
        table.set_dotted(false);
    }
    Some(match removed {
        Item::Table(removed) => Carried {
            comments: comments(removed.decor()),
            after: removed.position(),
        },
        Item::ArrayOfTables(removed) => Carried {
            comments: removed
                .get(0)
                .map(|first| comments(first.decor()))
                .unwrap_or_default(),
            after: removed.get(0).and_then(Table::position),
        },
        Item::Value(_) | Item::None => {
            let comments = comments(removed_key.leaf_decor());
            if let Some(next) = next {
                prepend_line(table, &next, &comments)?;
                return Some(Carried::default());
            }
            Carried {
                comments,
                after: table.position(),
            }
        }
    })
}

/// The keys, through dotted tables, of the first key-value line after `key` in `table`.
fn next_line(table: &Table, key: &str) -> Option<Vec<String>> {
    let (next, mut item) = table
        .iter()
        .skip_while(|(sibling, _)| *sibling != key)
        .skip(1)
        .find(|(_, sibling)| {
            sibling.is_value() || sibling.as_table().is_some_and(Table::is_dotted)
        })?;
    let mut keys = vec![next.to_owned()];
    while let Item::Table(dotted) = item {
        let (first, inner) = dotted.iter().next()?;
        keys.push(first.to_owned());
        item = inner;
    }
    Some(keys)
}

fn prepend_line(mut table: &mut Table, keys: &[String], comments: &str) -> Option<()> {
    let (last, dotted) = keys.split_last()?;
    for key in dotted {
        table = table.get_mut(key)?.as_table_mut()?;
    }
    prepend(table.key_mut(last)?.leaf_decor_mut(), comments);
    Some(())
}

fn remove_inline(table: &mut InlineTable, key: &str) -> Option<()> {
    let keys: Vec<String> = table
        .iter()
        .map(|(sibling, _)| sibling.to_owned())
        .collect();
    let index = keys.iter().position(|sibling| sibling == key)?;
    let (removed_key, removed) = table.remove_entry(key)?;
    let comments = comments(removed_key.leaf_decor());
    if let Some(next) = keys.get(index + 1) {
        prepend(table.key_mut(next)?.leaf_decor_mut(), &comments);
    } else {
        if let Some(previous) = index
            .checked_sub(1)
            .and_then(|previous| table.get_mut(&keys[previous]))
        {
            let suffix = text(removed.decor().suffix()).to_owned();
            previous.decor_mut().set_suffix(suffix);
        }
        let trailing = format!("{comments}{}", text(Some(table.trailing())));
        table.set_trailing(trailing);
    }
    Some(())
}

fn remove_element(array: &mut Array, index: usize) -> Option<()> {
    if index >= array.len() {
        return None;
    }
    let removed = array.remove(index);
    let comments = comments(removed.decor());
    // An element prefix opens with its own line break.
    let comments = comments.strip_suffix('\n').unwrap_or(&comments);
    match array.get_mut(index) {
        Some(next) if index == 0 && !text(next.decor().prefix()).contains('\n') => {
            next.decor_mut()
                .set_prefix(text(removed.decor().prefix()).to_owned());
        }
        Some(next) => prepend(next.decor_mut(), comments),
        None => {
            let trailing = format!("{comments}{}", text(Some(array.trailing())));
            array.set_trailing(trailing);
        }
    }
    Some(())
}

fn remove_table(tables: &mut ArrayOfTables, index: usize) -> Option<Carried> {
    if index >= tables.len() {
        return None;
    }
    let removed = tables.remove(index);
    Some(Carried {
        comments: comments(removed.decor()),
        after: removed.position(),
    })
}

/// An emptied array of tables prints nothing, so it stays as an empty array.
fn keep_empty(document: &mut DocumentMut, at: &[Step]) -> Option<()> {
    let (Step::Key(key), parents) = at.split_last()? else {
        return None;
    };
    let parent = locate(document.as_item_mut(), parents)?.as_table_like_mut()?;
    parent.insert(key, Item::Value(Toml::Array(Array::new())));
    parent.key_mut(key)?.leaf_decor_mut().clear();
    Some(())
}

fn place(document: &mut DocumentMut, carried: &Carried) {
    let mut positions = Vec::new();
    visit(document.as_item_mut(), &mut |table| {
        if !table.is_dotted() && !table.is_implicit() {
            positions.extend(table.position());
        }
    });
    let mut placed = false;
    if let Some(next) = positions
        .into_iter()
        .filter(|position| Some(*position) > carried.after)
        .min()
    {
        visit(document.as_item_mut(), &mut |table| {
            if table.position() == Some(next) {
                prepend(table.decor_mut(), &carried.comments);
                placed = true;
            }
        });
    }
    if !placed {
        let trailing = format!("{}{}", carried.comments, text(Some(document.trailing())));
        document.set_trailing(trailing);
    }
}

fn visit(item: &mut Item, each: &mut impl FnMut(&mut Table)) {
    match item {
        Item::Table(table) => {
            each(table);
            for (_, child) in table.iter_mut() {
                visit(child, each);
            }
        }
        Item::ArrayOfTables(tables) => {
            for table in tables.iter_mut() {
                each(table);
                for (_, child) in table.iter_mut() {
                    visit(child, each);
                }
            }
        }
        Item::Value(_) | Item::None => {}
    }
}

fn comments(decor: &Decor) -> String {
    let prefix = text(decor.prefix());
    match prefix.rfind('\n') {
        Some(end) if prefix.contains('#') => prefix[..=end].to_owned(),
        _ => String::new(),
    }
}

fn prepend(decor: &mut Decor, comments: &str) {
    if !comments.is_empty() {
        let prefix = format!("{comments}{}", text(decor.prefix()));
        decor.set_prefix(prefix);
    }
}

fn text(raw: Option<&RawString>) -> &str {
    raw.and_then(RawString::as_str).unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{edit, parse};
    use crate::error::FormatError;
    use crate::format::{self, Edit, Format, Step};
    use crate::value::{Datetime, Value};

    fn at(keys: &[&str]) -> Vec<Step> {
        keys.iter()
            .map(|key| Step::Key((*key).to_owned()))
            .collect()
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

    fn datetime(text: &str) -> Value {
        Value::Datetime(Datetime::parse(text).unwrap())
    }

    fn edited(source: &str, edits: &[Edit]) -> String {
        String::from_utf8(edit(source.as_bytes(), edits).unwrap()).unwrap()
    }

    #[test]
    fn parse_reads_values_datetimes_and_comments_inside_values() {
        let empty = parse(b"").unwrap();
        assert_eq!(empty.value, map([]));
        assert!(empty.commented.is_empty());
        assert!(empty.duplicated.is_empty());

        let document = parse(
            br#"# leading
title = "meros" # trailing

[tool]
args = [
  "a", # pinned
  "b",
]
plain = [1, 2.0]
inline = { on = true }
when = 1979-05-27T07:32:00Z
local = 1979-05-27T07:32:00
date = 1979-05-27
time = 07:32:00

[tool.quiet]
n = 1
"#,
        )
        .unwrap();
        assert_eq!(
            document.value,
            map([
                ("title", text("meros")),
                (
                    "tool",
                    map([
                        ("args", Value::List(vec![text("a"), text("b")])),
                        (
                            "plain",
                            Value::List(vec![Value::Integer(1), Value::Float(2.0)]),
                        ),
                        ("inline", map([("on", Value::Boolean(true))])),
                        ("when", datetime("1979-05-27T07:32:00Z")),
                        ("local", datetime("1979-05-27T07:32:00")),
                        ("date", datetime("1979-05-27")),
                        ("time", datetime("07:32:00")),
                        ("quiet", map([("n", Value::Integer(1))])),
                    ]),
                ),
            ]),
        );
        assert!(document.commented.contains(&at(&["tool", "args"])));
        for quiet in [
            at(&["title"]),
            at(&["tool", "plain"]),
            at(&["tool", "inline"]),
            at(&["tool", "quiet"]),
        ] {
            assert!(
                !document.commented.contains(&quiet),
                "{quiet:?} holds no comment inside",
            );
        }
        assert!(document.duplicated.is_empty());

        assert!(matches!(parse(b"a = "), Err(FormatError::Syntax { .. })));
    }

    #[test]
    fn edit_replaces_and_removes_units_keeping_every_comment_outside_them() {
        let source = r#"# top
[tool]
# about owner
owner = "me" # mine
# about gone
gone = 1
# about next
next = true

[keep]
# about only
only = "x"
"#;
        assert_eq!(
            edited(
                source,
                &[
                    Edit::Replace {
                        at: at(&["tool", "owner"]),
                        value: text("you"),
                    },
                    Edit::Remove {
                        at: at(&["tool", "gone"]),
                    },
                    Edit::Remove {
                        at: at(&["keep", "only"]),
                    },
                ],
            ),
            r#"# top
[tool]
# about owner
owner = "you" # mine
# about gone
# about next
next = true

[keep]
# about only
"#,
        );
    }

    #[test]
    fn edit_creates_units_in_sibling_style_and_new_files_in_default_style() {
        let source = r"[a]
  x = 1
  inline = { p = 1 }

[b]
y = 2
";
        assert_eq!(
            edited(
                source,
                &[
                    Edit::Create {
                        at: at(&["a", "z"]),
                        value: Value::Integer(3),
                    },
                    Edit::Create {
                        at: at(&["a", "inline", "q"]),
                        value: Value::Integer(2),
                    },
                    Edit::Create {
                        at: at(&["c", "w"]),
                        value: text("v"),
                    },
                ],
            ),
            r#"[a]
  x = 1
  inline = { p = 1, q = 2 }
  z = 3

[b]
y = 2

[c]
w = "v"
"#,
        );

        let created = edit(
            b"",
            &[
                Edit::Create {
                    at: at(&["top"]),
                    value: Value::Integer(1),
                },
                Edit::Create {
                    at: at(&["tool", "owner"]),
                    value: text("you"),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            parse(&created).unwrap().value,
            map([
                ("top", Value::Integer(1)),
                ("tool", map([("owner", text("you"))])),
            ]),
        );
    }

    #[test]
    fn edit_creates_list_elements_under_missing_parents() {
        let args = vec![
            Step::Key("tool".to_owned()),
            Step::Key("args".to_owned()),
            Step::Index(0),
        ];
        let repos = vec![Step::Key("repos".to_owned()), Step::Index(0)];
        assert_eq!(
            edited(
                "",
                &[
                    Edit::Create {
                        at: args,
                        value: text("a"),
                    },
                    Edit::Create {
                        at: repos,
                        value: map([("id", text("ruff"))]),
                    },
                ],
            ),
            r#"[tool]
args = ["a"]

[[repos]]
id = "ruff"
"#,
        );

        let nested = vec![
            Step::Key("repos".to_owned()),
            Step::Index(0),
            Step::Key("args".to_owned()),
            Step::Index(0),
        ];
        assert_eq!(
            parse(
                edited(
                    "",
                    &[Edit::Create {
                        at: nested,
                        value: text("--fix")
                    }]
                )
                .as_bytes()
            )
            .unwrap()
            .value,
            map([(
                "repos",
                Value::List(vec![map([("args", Value::List(vec![text("--fix")]))])]),
            )]),
        );
    }

    #[test]
    fn edit_refuses_list_creates_off_the_original_length() {
        let list = |index| vec![Step::Key("list".to_owned()), Step::Index(index)];
        let missing = vec![Step::Key("missing".to_owned()), Step::Index(1)];
        for unfit in [list(0), list(5), missing] {
            assert_eq!(
                format::edit(
                    Format::Toml,
                    b"list = [1]\n",
                    &[Edit::Create {
                        at: unfit,
                        value: Value::Integer(2),
                    }],
                ),
                Err(FormatError::UnfitEdit),
            );
        }
    }

    #[test]
    fn edit_replaces_an_array_of_tables_keeping_its_leading_comment() {
        let source = r#"[tool]
x = 1

# servers we own
[[tool.items]]
name = "a"
"#;
        assert_eq!(
            edited(
                source,
                &[Edit::Replace {
                    at: at(&["tool", "items"]),
                    value: Value::List(vec![map([("id", text("ruff"))])]),
                }],
            ),
            r#"[tool]
x = 1

# servers we own
[[tool.items]]
id = "ruff"
"#,
        );
        assert_eq!(
            edited(
                source,
                &[Edit::Replace {
                    at: at(&["tool", "items"]),
                    value: Value::List(vec![text("a")]),
                }],
            ),
            r#"[tool]
x = 1

# servers we own
items = ["a"]
"#,
        );
    }

    #[test]
    fn edit_moves_a_removed_comment_before_a_dotted_key() {
        assert_eq!(
            edited(
                "[tool]\n# about gone\ngone = 1\nsub.x = 2\n\n[other]\ny = 2\n",
                &[Edit::Remove {
                    at: at(&["tool", "gone"]),
                }],
            ),
            "[tool]\n# about gone\nsub.x = 2\n\n[other]\ny = 2\n",
        );
    }
}
