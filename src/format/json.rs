use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{
    CstContainerNode, CstInputValue, CstLeafNode, CstNode, CstObject, CstRootNode,
};
use serde_json::{Number, Value as Json};

use super::{Document, Edit, Step};
use crate::error::FormatError;
use crate::value::Value;

pub(crate) fn parse(bytes: &[u8]) -> Result<Document, FormatError> {
    let mut reader = Reader::default();
    let value = match document(utf8(bytes)?)?.value() {
        Some(node) => reader.node(&node)?.0,
        None => Value::Map(BTreeMap::new()),
    };
    Ok(Document {
        value,
        commented: reader.commented,
        duplicated: reader.duplicated,
    })
}

pub(crate) fn edit<'a>(
    bytes: &[u8],
    edits: impl IntoIterator<Item = &'a Edit>,
) -> Result<Vec<u8>, FormatError> {
    let mut text = utf8(bytes)?.to_owned();
    for edit in edits {
        text = apply(&document(&text)?, edit).ok_or(FormatError::UnfitEdit)?;
    }
    Ok(text.into_bytes())
}

pub(crate) fn encodable(value: &Value) -> bool {
    encode(value).is_some()
}

fn utf8(bytes: &[u8]) -> Result<&str, FormatError> {
    std::str::from_utf8(bytes).map_err(syntax)
}

/// JSON with JSONC's comments and trailing commas, and none of the parser's other extensions.
fn document(text: &str) -> Result<CstRootNode, FormatError> {
    let options = ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    CstRootNode::parse(text, &options).map_err(syntax)
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
    duplicated: BTreeSet<Vec<Step>>,
}

impl Reader {
    fn node(&mut self, node: &CstNode) -> Result<(Value, bool), FormatError> {
        let value = match node {
            CstNode::Container(CstContainerNode::Object(object)) => {
                let mut inside = commented(&node.children());
                let mut members = BTreeMap::new();
                for property in object.properties() {
                    inside |= commented(&property.children());
                    let key = property
                        .name()
                        .ok_or_else(|| syntax("a property without a name"))?
                        .decoded_value()
                        .map_err(syntax)?;
                    let value = property
                        .value()
                        .ok_or_else(|| syntax("a property without a value"))?;
                    let repeated = members.contains_key(&key);
                    let (member, commented) = self.within(Step::Key(key.clone()), |reader| {
                        if repeated {
                            reader.duplicated.insert(reader.at.clone());
                        }
                        reader.node(&value)
                    })?;
                    inside |= commented;
                    members.entry(key).or_insert(member);
                }
                return Ok(self.mark(Value::Map(members), inside));
            }
            CstNode::Container(CstContainerNode::Array(array)) => {
                let mut inside = commented(&node.children());
                let mut elements = Vec::new();
                for (index, element) in array.elements().iter().enumerate() {
                    let (element, commented) =
                        self.within(Step::Index(index), |reader| reader.node(element))?;
                    inside |= commented;
                    elements.push(element);
                }
                return Ok(self.mark(Value::List(elements), inside));
            }
            CstNode::Leaf(CstLeafNode::StringLit(text)) => {
                Value::String(text.decoded_value().map_err(syntax)?)
            }
            CstNode::Leaf(CstLeafNode::NumberLit(number)) => number_value(&number.to_string())?,
            CstNode::Leaf(CstLeafNode::BooleanLit(boolean)) => Value::Boolean(boolean.value()),
            CstNode::Leaf(CstLeafNode::NullKeyword(_)) => Value::Null,
            _ => return Err(syntax(format_args!("unexpected `{node}`"))),
        };
        Ok((value, false))
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

fn commented(children: &[CstNode]) -> bool {
    children.iter().any(CstNode::is_comment)
}

/// A JSON number: an integer when written without fraction or exponent, else a float.
fn number_value(text: &str) -> Result<Value, FormatError> {
    if text.contains(['.', 'e', 'E']) {
        return text.parse().map(Value::Float).map_err(syntax);
    }
    Ok(text
        .parse()
        .map_or_else(|_| Value::BigInteger(text.to_owned()), Value::Integer))
}

/// The text of `root` after `edit`.
fn apply(root: &CstRootNode, edit: &Edit) -> Option<String> {
    match edit {
        Edit::Replace { at, value } => replace(root, at, encode(value)?)?,
        Edit::Create { at, value } => return create(root, at, encode(value)?),
        Edit::Remove { at } => return remove(root, at),
    }
    Some(root.to_string())
}

fn replace(root: &CstRootNode, at: &[Step], value: CstInputValue) -> Option<()> {
    if at.is_empty() {
        root.set_value(value);
        return Some(());
    }
    match locate(root, at)? {
        CstNode::Container(CstContainerNode::Object(node)) => node.replace_with(value),
        CstNode::Container(CstContainerNode::Array(node)) => node.replace_with(value),
        CstNode::Leaf(CstLeafNode::StringLit(node)) => node.replace_with(value),
        CstNode::Leaf(CstLeafNode::NumberLit(node)) => node.replace_with(value),
        CstNode::Leaf(CstLeafNode::BooleanLit(node)) => node.replace_with(value),
        CstNode::Leaf(CstLeafNode::NullKeyword(node)) => node.replace_with(value),
        _ => None,
    }
    .map(drop)
}

/// Appends `value` at the end of its parent, creating missing parents around it.
fn create(root: &CstRootNode, at: &[Step], value: CstInputValue) -> Option<String> {
    let Some(mut parent) = root.value() else {
        root.set_value(nest(at, value));
        return Some(root.to_string());
    };
    let (last, parents) = at.split_last()?;
    for (depth, step) in parents.iter().enumerate() {
        match child(&parent, step) {
            Some(next) => parent = next,
            None => return append(root, &parent, step, nest(&at[depth + 1..], value)),
        }
    }
    if matches!(last, Step::Key(_)) && child(&parent, last).is_some() {
        return None;
    }
    append(root, &parent, last, value)
}

/// `value` inside the containers that the steps `below` it lead through.
fn nest(below: &[Step], value: CstInputValue) -> CstInputValue {
    below.iter().rev().fold(value, |inner, step| match step {
        Step::Key(key) => CstInputValue::Object(vec![(key.clone(), inner)]),
        Step::Index(_) => CstInputValue::Array(vec![inner]),
    })
}

fn append(
    root: &CstRootNode,
    parent: &CstNode,
    step: &Step,
    value: CstInputValue,
) -> Option<String> {
    match step {
        Step::Key(key) => {
            let object = parent.as_object()?;
            if let Some(text) = append_on_line(root, &object, key, &value) {
                return Some(text);
            }
            drop(object.append(key, value));
        }
        Step::Index(_) => drop(parent.as_array()?.append(value)),
    }
    Some(root.to_string())
}

/// The text of `root` with a member appended on the line of `object`, in the spacing of
/// its last members; `None` when `object` is empty or spans several lines, which
/// `CstObject::append` handles.
fn append_on_line(
    root: &CstRootNode,
    object: &CstObject,
    key: &str,
    value: &CstInputValue,
) -> Option<String> {
    if object.children().iter().any(CstNode::is_newline) {
        return None;
    }
    let members = object.properties();
    let (last, earlier) = members.split_last()?;
    let colon: String = last
        .children()
        .iter()
        .skip(1)
        .take_while(|part| part.is_trivia() || part.is_token())
        .map(ToString::to_string)
        .collect();
    let space = earlier
        .last()
        .and_then(|previous| previous.trailing_comma()?.next_sibling()?.as_whitespace())
        .map_or_else(|| " ".to_owned(), |space| space.value());
    let member = format!("{}{colon}{}", Json::from(key), inline(value));
    let (position, inserted) = if let Some(comma) = last.trailing_comma() {
        (offset(&comma.into()) + 1, format!("{space}{member},"))
    } else {
        let last = CstNode::from(last.clone());
        (
            offset(&last) + last.to_string().len(),
            format!(",{space}{member}"),
        )
    };
    let mut text = root.to_string();
    text.insert_str(position, &inserted);
    Some(text)
}

/// `value` as JSON on one line.
fn inline(value: &CstInputValue) -> String {
    match value {
        CstInputValue::Null => "null".to_owned(),
        CstInputValue::Bool(boolean) => boolean.to_string(),
        CstInputValue::Number(number) => number.clone(),
        CstInputValue::String(text) => Json::from(text.as_str()).to_string(),
        CstInputValue::Array(elements) => format!(
            "[{}]",
            elements.iter().map(inline).collect::<Vec<_>>().join(", ")
        ),
        CstInputValue::Object(members) if members.is_empty() => "{}".to_owned(),
        CstInputValue::Object(members) => format!(
            "{{ {} }}",
            members
                .iter()
                .map(|(key, member)| format!("{}: {}", Json::from(key.as_str()), inline(member)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The byte position of `node` in the text of its root.
fn offset(node: &CstNode) -> usize {
    let before = |node: &CstNode| -> usize {
        node.previous_siblings()
            .map(|sibling| sibling.to_string().len())
            .sum()
    };
    before(node)
        + node
            .ancestors()
            .map(|ancestor| before(&CstNode::Container(ancestor)))
            .sum::<usize>()
}

fn remove(root: &CstRootNode, at: &[Step]) -> Option<String> {
    let (last, parents) = at.split_last()?;
    let parent = locate(root, parents)?;
    let node = match last {
        Step::Key(key) => parent.as_object()?.get(key)?.into(),
        Step::Index(_) => child(&parent, last)?,
    };
    if let Some(text) = remove_before_comment(root, &node) {
        return Some(text);
    }
    node.remove();
    Some(root.to_string())
}

/// The text of `root` without `node`, its comma and the spaces after it, when a comment
/// ends the line after that comma; `CstNode::remove` would delete that comment.
fn remove_before_comment(root: &CstRootNode, node: &CstNode) -> Option<String> {
    let comma = node.trailing_comma()?;
    let line: Vec<CstNode> = comma
        .next_siblings()
        .take_while(|part| !part.is_newline())
        .collect();
    if !line.iter().all(CstNode::is_trivia) || !line.iter().any(CstNode::is_comment) {
        return None;
    }
    let spaces: usize = line
        .iter()
        .take_while(|part| part.is_whitespace())
        .map(|part| part.to_string().len())
        .sum();
    let end = offset(&comma.into()) + 1 + spaces;
    let mut text = root.to_string();
    text.replace_range(offset(node)..end, "");
    Some(text)
}

fn locate(root: &CstRootNode, at: &[Step]) -> Option<CstNode> {
    at.iter()
        .try_fold(root.value()?, |node, step| child(&node, step))
}

fn child(node: &CstNode, step: &Step) -> Option<CstNode> {
    match step {
        Step::Key(key) => node.as_object()?.get(key)?.value(),
        Step::Index(index) => node.as_array()?.elements().get(*index).cloned(),
    }
}

fn encode(value: &Value) -> Option<CstInputValue> {
    Some(match value {
        Value::Map(members) => CstInputValue::Object(
            members
                .iter()
                .map(|(key, member)| Some((key.clone(), encode(member)?)))
                .collect::<Option<_>>()?,
        ),
        Value::List(elements) => {
            CstInputValue::Array(elements.iter().map(encode).collect::<Option<_>>()?)
        }
        Value::String(text) => CstInputValue::String(text.clone()),
        Value::Integer(integer) => CstInputValue::Number(integer.to_string()),
        Value::BigInteger(text) => CstInputValue::Number(text.clone()),
        Value::Float(float) => CstInputValue::Number(Number::from_f64(*float)?.to_string()),
        Value::Boolean(boolean) => CstInputValue::Bool(*boolean),
        Value::Null => CstInputValue::Null,
        Value::Datetime(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{edit, encodable, parse};
    use crate::error::FormatError;
    use crate::format::{self, Edit, Format, Step};
    use crate::value::{Datetime, Value};

    fn at(keys: &[&str]) -> Vec<Step> {
        keys.iter()
            .map(|key| Step::Key((*key).to_owned()))
            .collect()
    }

    fn element(keys: &[&str], index: usize) -> Vec<Step> {
        let mut steps = at(keys);
        steps.push(Step::Index(index));
        steps
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

    fn edited(source: &str, edits: &[Edit]) -> String {
        String::from_utf8(format::edit(Format::Json, source.as_bytes(), edits).unwrap()).unwrap()
    }

    #[test]
    fn parse_reads_jsonc_values_comments_inside_values_and_duplicate_keys() {
        let empty = parse(b"").unwrap();
        assert_eq!(empty.value, map([]));
        assert!(empty.commented.is_empty());
        assert!(empty.duplicated.is_empty());

        let document = parse(
            br#"// leading
{
  "title": "q\"\u00e9", // trailing
  "numbers": [1, -3, 1.5, 9223372036854775807, 9223372036854775808],
  "args": [
    "a", // pinned
    "b",
  ],
  "server": {
    /* about on */
    "on": true,
  },
  "plain": { "off": false, "none": null },
}
"#,
        )
        .unwrap();
        assert_eq!(
            document.value,
            map([
                ("title", text("q\"\u{e9}")),
                (
                    "numbers",
                    Value::List(vec![
                        Value::Integer(1),
                        Value::Integer(-3),
                        Value::Float(1.5),
                        Value::Integer(i64::MAX),
                        Value::BigInteger("9223372036854775808".to_owned()),
                    ]),
                ),
                ("args", Value::List(vec![text("a"), text("b")])),
                ("server", map([("on", Value::Boolean(true))])),
                (
                    "plain",
                    map([("off", Value::Boolean(false)), ("none", Value::Null)]),
                ),
            ]),
        );
        assert!(document.commented.contains(&at(&["args"])));
        assert!(document.commented.contains(&at(&["server"])));
        for quiet in [at(&["title"]), at(&["numbers"]), at(&["plain"])] {
            assert!(
                !document.commented.contains(&quiet),
                "{quiet:?} holds no comment inside",
            );
        }
        assert!(document.duplicated.is_empty());

        let repeated = parse(br#"{"a": 1, "a": 2, "o": {"k": 1, "k": 1, "u": 0}}"#).unwrap();
        assert_eq!(
            repeated.duplicated,
            BTreeSet::from([at(&["a"]), at(&["o", "k"])]),
        );

        assert!(matches!(
            parse(br#"{"a": }"#),
            Err(FormatError::Syntax { .. })
        ));

        let datetime = Value::Datetime(Datetime::parse("1979-05-27").unwrap());
        assert!(encodable(&map([
            ("none", Value::Null),
            ("list", Value::List(vec![Value::Null, text("x")])),
        ])));
        assert!(!encodable(&datetime));
        assert!(!encodable(&map([(
            "deep",
            Value::List(vec![map([("when", datetime)])]),
        )])));
    }

    #[test]
    fn edit_replaces_and_removes_units_in_place_keeping_order_comments_and_trailing_commas() {
        let source = r#"{
  // about owner
  "owner": "me", // mine
  // about gone
  "gone": 1,
  "list": [
    "a", // first
    "b",
    "c",
  ],
  "keep": { "only": "x" },
  // about next
  "next": true,
}
"#;
        let output = edited(
            source,
            &[
                Edit::Replace {
                    at: at(&["owner"]),
                    value: text("you"),
                },
                Edit::Remove { at: at(&["gone"]) },
                Edit::Remove {
                    at: element(&["list"], 1),
                },
                Edit::Replace {
                    at: element(&["list"], 2),
                    value: text("C"),
                },
                Edit::Remove { at: at(&["next"]) },
            ],
        );
        assert_eq!(
            output,
            r#"{
  // about owner
  "owner": "you", // mine
  // about gone
  "list": [
    "a", // first
    "C",
  ],
  "keep": { "only": "x" },
  // about next
}
"#,
        );

        assert_eq!(
            edited(
                "{\"a\": 1, // keep\n \"b\": 2}",
                &[Edit::Remove { at: at(&["a"]) }],
            ),
            "{// keep\n \"b\": 2}",
        );

        let emptied = edited(
            r#"{"keep": {"only": "x"}, "other": 1}"#,
            &[Edit::Remove {
                at: at(&["keep", "only"]),
            }],
        );
        assert_eq!(
            parse(emptied.as_bytes()).unwrap().value,
            map([("keep", map([])), ("other", Value::Integer(1))]),
        );
    }

    const NESTED: &str = r#"{
    "a": {
        "x": 1
    },
    "list": [1, 2]
}
"#;

    #[test]
    fn edit_creates_units_at_the_end_of_their_parent_in_sibling_style() {
        assert_eq!(
            edited(
                NESTED,
                &[
                    Edit::Create {
                        at: at(&["a", "z"]),
                        value: Value::Integer(3),
                    },
                    Edit::Create {
                        at: element(&["list"], 2),
                        value: Value::Integer(3),
                    },
                    Edit::Create {
                        at: at(&["b", "c"]),
                        value: Value::Boolean(true),
                    },
                ],
            ),
            r#"{
    "a": {
        "x": 1,
        "z": 3
    },
    "list": [1, 2, 3],
    "b": {
        "c": true
    }
}
"#,
        );

        assert_eq!(
            edited(
                r#"{
  "tail": [
    "p", // pinned
  ],
  // last
  "end": 1,
}
"#,
                &[
                    Edit::Create {
                        at: element(&["tail"], 1),
                        value: text("q"),
                    },
                    Edit::Create {
                        at: at(&["new"]),
                        value: text("v"),
                    },
                ],
            ),
            r#"{
  "tail": [
    "p", // pinned
    "q",
  ],
  // last
  "end": 1,
  "new": "v",
}
"#,
        );

        assert_eq!(
            edited(
                r#"{"keep": { "only": "x" }, "z": 1}"#,
                &[Edit::Create {
                    at: at(&["keep", "w"]),
                    value: Value::Integer(1),
                }],
            ),
            r#"{"keep": { "only": "x", "w": 1 }, "z": 1}"#,
        );
        assert_eq!(
            edited(
                r#"{"a": 1}"#,
                &[Edit::Create {
                    at: at(&["b"]),
                    value: Value::Integer(1),
                }],
            ),
            r#"{"a": 1, "b": 1}"#,
        );
    }

    #[test]
    fn edit_appends_list_creates_at_the_original_length_only() {
        assert_eq!(
            edited(
                NESTED,
                &[
                    Edit::Create {
                        at: element(&["list"], 2),
                        value: Value::Integer(3),
                    },
                    Edit::Create {
                        at: element(&["list"], 2),
                        value: Value::Integer(4),
                    },
                ],
            ),
            NESTED.replace("[1, 2]", "[1, 2, 3, 4]"),
        );
        for (source, unfit) in [
            ("[1]", vec![Step::Index(0)]),
            (NESTED, element(&["list"], 5)),
            (NESTED, element(&["missing"], 1)),
        ] {
            assert_eq!(
                format::edit(
                    Format::Json,
                    source.as_bytes(),
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
    fn edit_writes_floats_that_parse_back_as_the_same_floats() {
        for float in [1e16, 1e300, 1.0, 0.5] {
            let written = edited(
                "{}",
                &[Edit::Create {
                    at: at(&["f"]),
                    value: Value::Float(float),
                }],
            );
            assert_eq!(
                parse(written.as_bytes()).unwrap().value,
                map([("f", Value::Float(float))]),
            );
        }
        assert!(!encodable(&Value::Float(f64::NAN)));
    }

    #[test]
    fn edit_writes_a_new_file_in_default_style() {
        assert_eq!(
            edited(
                "",
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
            ),
            r#"{
  "top": 1,
  "tool": {
    "owner": "you"
  }
}
"#,
        );
        let args = edited(
            "",
            &[Edit::Create {
                at: element(&["args"], 0),
                value: text("--fix"),
            }],
        );
        assert!(args.ends_with("}\n") && !args.ends_with("\n\n"), "{args:?}");
        assert_eq!(
            parse(args.as_bytes()).unwrap().value,
            map([("args", Value::List(vec![text("--fix")]))]),
        );
    }

    #[test]
    fn edit_refuses_unfit_edits() {
        for unfit in [
            Edit::Create {
                at: at(&["a", "x", "deep"]),
                value: Value::Integer(1),
            },
            Edit::Replace {
                at: element(&["list"], 5),
                value: Value::Integer(1),
            },
            Edit::Remove {
                at: at(&["a", "missing"]),
            },
        ] {
            assert_eq!(
                edit(NESTED.as_bytes(), &[unfit]),
                Err(FormatError::UnfitEdit),
            );
        }
    }
}
