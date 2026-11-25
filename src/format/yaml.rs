use std::collections::BTreeSet;
use std::fmt::{Display, Write};
use std::ops::Range;

use saphyr_parser::{Event, Parser, ScalarStyle, Span, StrInput, Tag};

use super::{Document, Edit, Step};
use crate::error::{FormatError, YamlFeature};
use crate::value::Value;

/// The document as a value; comments and duplicate keys make it unsupported, so it
/// marks neither.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called by `read`, whose body is `todo!()`")
)]
pub(crate) fn parse(bytes: &[u8]) -> Result<Document, FormatError> {
    Ok(Document {
        value: read(bytes)?.into_value(),
        commented: BTreeSet::new(),
        duplicated: BTreeSet::new(),
    })
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called by `plan`, whose body is `todo!()`")
)]
pub(crate) fn edit<'a>(
    bytes: &[u8],
    edits: impl IntoIterator<Item = &'a Edit>,
) -> Result<Vec<u8>, FormatError> {
    let mut root = read(bytes)?;
    for edit in edits {
        apply(&mut root, edit).ok_or(FormatError::UnfitEdit)?;
    }
    let mut text = String::new();
    block(&mut text, &root, 0);
    Ok(text.into_bytes())
}

pub(crate) fn encodable(value: &Value) -> bool {
    encode(value).is_some()
}

/// A YAML node in document order, each scalar with the YAML text it is written as.
enum Node {
    Map(Vec<(Key, Node)>),
    List(Vec<Node>),
    Scalar { value: Value, text: String },
}

impl Node {
    fn into_value(self) -> Value {
        match self {
            Self::Map(members) => Value::Map(
                members
                    .into_iter()
                    .map(|(key, member)| (key.text, member.into_value()))
                    .collect(),
            ),
            Self::List(elements) => {
                Value::List(elements.into_iter().map(Self::into_value).collect())
            }
            Self::Scalar { value, .. } => value,
        }
    }

    fn is_block(&self) -> bool {
        match self {
            Self::Map(members) => !members.is_empty(),
            Self::List(elements) => !elements.is_empty(),
            Self::Scalar { .. } => false,
        }
    }
}

/// A mapping key: a plain key is written back as read, any other quoted where needed.
struct Key {
    text: String,
    plain: bool,
}

impl Key {
    fn new(text: String) -> Self {
        Self { text, plain: false }
    }
}

/// The single document of `bytes`; an empty stream is an empty map.
fn read(bytes: &[u8]) -> Result<Node, FormatError> {
    let text = std::str::from_utf8(bytes).map_err(syntax)?;
    let mut reader = Reader {
        text,
        events: Parser::new_from_str(text),
        scalars: Vec::new(),
    };
    let root = reader.stream()?;
    if reader.commented() {
        return Err(FormatError::UnsupportedYaml(YamlFeature::Comment));
    }
    Ok(root)
}

fn syntax(error: impl Display) -> FormatError {
    FormatError::Syntax {
        message: error.to_string(),
    }
}

struct Reader<'input> {
    text: &'input str,
    events: Parser<'input, StrInput<'input>>,
    /// Character ranges of scalar content, where a `#` starts no comment.
    scalars: Vec<Range<usize>>,
}

impl<'input> Reader<'input> {
    fn next(&mut self) -> Result<(Event<'input>, Span), FormatError> {
        self.events
            .next_event()
            .ok_or_else(|| syntax("unexpected end of the stream"))?
            .map_err(syntax)
    }

    fn stream(&mut self) -> Result<Node, FormatError> {
        self.next()?;
        let root = match self.next()?.0 {
            Event::StreamEnd => return Ok(Node::Map(Vec::new())),
            Event::DocumentStart(_) => {
                let (event, span) = self.next()?;
                self.node(event, span)?
            }
            event => return Err(syntax(format_args!("unexpected {event:?}"))),
        };
        self.next()?;
        match self.next()?.0 {
            Event::StreamEnd => Ok(root),
            _ => Err(FormatError::UnsupportedYaml(YamlFeature::MultipleDocuments)),
        }
    }

    fn node(&mut self, event: Event<'input>, span: Span) -> Result<Node, FormatError> {
        match event {
            Event::Scalar(text, style, anchor, tag) => {
                bare(anchor, tag.as_deref())?;
                self.cover(span, style);
                let value = match style {
                    ScalarStyle::Plain => resolve(&text),
                    _ => Value::String(text.into_owned()),
                };
                encode(&value).ok_or_else(|| syntax(format_args!("unwritable scalar {value:?}")))
            }
            Event::SequenceStart(anchor, tag) => {
                bare(anchor, tag.as_deref())?;
                let mut elements = Vec::new();
                loop {
                    match self.next()? {
                        (Event::SequenceEnd, _) => return Ok(Node::List(elements)),
                        (event, span) => elements.push(self.node(event, span)?),
                    }
                }
            }
            Event::MappingStart(anchor, tag) => {
                bare(anchor, tag.as_deref())?;
                let mut members = Vec::new();
                let mut texts = BTreeSet::new();
                let mut values = BTreeSet::new();
                loop {
                    let key = match self.next()? {
                        (Event::MappingEnd, _) => return Ok(Node::Map(members)),
                        (Event::Scalar(key, style, anchor, tag), span) => {
                            bare(anchor, tag.as_deref())?;
                            self.cover(span, style);
                            Key {
                                text: key.into_owned(),
                                plain: style == ScalarStyle::Plain,
                            }
                        }
                        (Event::Alias(_), _) => {
                            return Err(FormatError::UnsupportedYaml(YamlFeature::Alias));
                        }
                        _ => return Err(syntax("a mapping key that is no scalar")),
                    };
                    let value = if key.plain {
                        resolve(&key.text)
                    } else {
                        Value::String(key.text.clone())
                    };
                    let value = render(&value)
                        .ok_or_else(|| syntax(format_args!("unwritable key {value:?}")))?;
                    if !texts.insert(key.text.clone()) || !values.insert(value) {
                        return Err(FormatError::UnsupportedYaml(YamlFeature::DuplicateKey));
                    }
                    let (event, span) = self.next()?;
                    members.push((key, self.node(event, span)?));
                }
            }
            Event::Alias(_) => Err(FormatError::UnsupportedYaml(YamlFeature::Alias)),
            event => Err(syntax(format_args!("unexpected {event:?}"))),
        }
    }

    /// Records a scalar's content; a block scalar's header line may end in a comment.
    fn cover(&mut self, span: Span, style: ScalarStyle) {
        let mut start = span.start.index();
        if matches!(style, ScalarStyle::Literal | ScalarStyle::Folded) {
            start += self
                .text
                .chars()
                .skip(start)
                .position(|char| char == '\n')
                .unwrap_or_default();
        }
        self.scalars.push(start..span.end.index());
    }

    /// Whether a `#` lies outside every scalar, where only a comment can hold it once
    /// anchors, aliases and tags are refused.
    fn commented(&self) -> bool {
        self.text.chars().enumerate().any(|(index, char)| {
            char == '#' && !self.scalars.iter().any(|scalar| scalar.contains(&index))
        })
    }
}

/// Refuses a node carrying an anchor or a tag.
fn bare(anchor: usize, tag: Option<&Tag>) -> Result<(), FormatError> {
    if anchor != 0 {
        return Err(FormatError::UnsupportedYaml(YamlFeature::Anchor));
    }
    if tag.is_some() {
        return Err(FormatError::UnsupportedYaml(YamlFeature::Tag));
    }
    Ok(())
}

/// A plain scalar's value under the YAML 1.2 core schema.
fn resolve(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => Value::Boolean(true),
        "false" | "False" | "FALSE" => Value::Boolean(false),
        ".nan" | ".NaN" | ".NAN" => Value::Float(f64::NAN),
        _ => integer(text)
            .or_else(|| float(text).map(Value::Float))
            .unwrap_or_else(|| Value::String(text.to_owned())),
    }
}

fn integer(text: &str) -> Option<Value> {
    let (digits, radix) = if let Some(digits) = text.strip_prefix("0o") {
        (digits, 8)
    } else if let Some(digits) = text.strip_prefix("0x") {
        (digits, 16)
    } else {
        (text.strip_prefix(['-', '+']).unwrap_or(text), 10)
    };
    if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
        return None;
    }
    let integer = if radix == 10 {
        text.parse()
    } else {
        i64::from_str_radix(digits, radix)
    };
    Some(integer.map_or_else(|_| Value::BigInteger(text.to_owned()), Value::Integer))
}

fn float(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return Some(if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, None), |(mantissa, exponent)| {
            (mantissa, Some(exponent))
        });
    let mantissa = match mantissa.split_once('.') {
        Some((whole, fraction)) => {
            digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty())
        }
        None => !mantissa.is_empty() && digits(mantissa),
    };
    let exponent = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    if mantissa && exponent {
        text.parse().ok()
    } else {
        None
    }
}

fn encode(value: &Value) -> Option<Node> {
    Some(match value {
        Value::Map(members) => Node::Map(
            members
                .iter()
                .map(|(key, member)| Some((Key::new(key.clone()), encode(member)?)))
                .collect::<Option<_>>()?,
        ),
        Value::List(elements) => Node::List(elements.iter().map(encode).collect::<Option<_>>()?),
        scalar => Node::Scalar {
            text: render(scalar)?,
            value: scalar.clone(),
        },
    })
}

/// A scalar `value` as YAML text that reads back as that value.
fn render(value: &Value) -> Option<String> {
    Some(match value {
        Value::String(text) => string(text),
        Value::Integer(integer) => integer.to_string(),
        Value::BigInteger(text) => text.clone(),
        Value::Float(float) if float.is_nan() => ".nan".to_owned(),
        Value::Float(float) if float.is_infinite() => if float.is_sign_negative() {
            "-.inf"
        } else {
            ".inf"
        }
        .to_owned(),
        Value::Float(float) => format!("{float:?}"),
        Value::Boolean(boolean) => boolean.to_string(),
        Value::Null => "null".to_owned(),
        Value::Map(_) | Value::List(_) | Value::Datetime(_) => return None,
    })
}

/// `text` plain when every YAML 1.1 or 1.2 reader takes it as that string, else double-quoted.
fn string(text: &str) -> String {
    let plain = text.starts_with(|char: char| char.is_ascii_alphabetic())
        && text
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '_' | '-' | '.' | '/'))
        && matches!(resolve(text), Value::String(_))
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "y" | "n" | "yes" | "no" | "on" | "off"
        );
    if plain {
        return text.to_owned();
    }
    let mut quoted = String::from('"');
    for char in text.chars() {
        match char {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            _ if char.is_control() || matches!(char, '\u{feff}' | '\u{fffe}' | '\u{ffff}') => {
                let _ = write!(quoted, "\\U{:08X}", u32::from(char));
            }
            _ => quoted.push(char),
        }
    }
    quoted.push('"');
    quoted
}

/// Writes `node` in block style at `indent`, each line ending in a newline.
fn block(out: &mut String, node: &Node, indent: usize) {
    let pad = " ".repeat(indent);
    match node {
        Node::Map(members) if node.is_block() => {
            for (key, member) in members {
                out.push_str(&pad);
                if key.plain {
                    out.push_str(&key.text);
                } else {
                    out.push_str(&string(&key.text));
                }
                out.push(':');
                if member.is_block() {
                    out.push('\n');
                    block(out, member, indent + 2);
                } else {
                    out.push(' ');
                    out.push_str(inline(member));
                    out.push('\n');
                }
            }
        }
        Node::List(elements) if node.is_block() => {
            for element in elements {
                out.push_str(&pad);
                out.push_str("- ");
                if element.is_block() {
                    let mut nested = String::new();
                    block(&mut nested, element, indent + 2);
                    out.push_str(&nested[indent + 2..]);
                } else {
                    out.push_str(inline(element));
                    out.push('\n');
                }
            }
        }
        _ => {
            out.push_str(&pad);
            out.push_str(inline(node));
            out.push('\n');
        }
    }
}

/// A scalar's text, or an empty container in flow style.
fn inline(node: &Node) -> &str {
    match node {
        Node::Map(_) => "{}",
        Node::List(_) => "[]",
        Node::Scalar { text, .. } => text,
    }
}

fn apply(root: &mut Node, edit: &Edit) -> Option<()> {
    match edit {
        Edit::Replace { at, value } => *locate(root, at)? = encode(value)?,
        Edit::Create { at, value } => create(root, at, encode(value)?)?,
        Edit::Remove { at } => remove(root, at)?,
    }
    Some(())
}

/// Appends `value` at the end of its parent, creating missing parents around it.
fn create(root: &mut Node, at: &[Step], value: Node) -> Option<()> {
    let (last, parents) = at.split_last()?;
    let mut parent = root;
    for (depth, step) in parents.iter().enumerate() {
        if child(parent, step).is_none() {
            return append(parent, step, nest(&at[depth + 1..], value));
        }
        parent = child(parent, step)?;
    }
    if matches!(last, Step::Key(_)) && child(parent, last).is_some() {
        return None;
    }
    append(parent, last, value)
}

/// `value` inside the containers that the steps `below` it lead through.
fn nest(below: &[Step], value: Node) -> Node {
    below.iter().rev().fold(value, |inner, step| match step {
        Step::Key(key) => Node::Map(vec![(Key::new(key.clone()), inner)]),
        Step::Index(_) => Node::List(vec![inner]),
    })
}

fn append(parent: &mut Node, step: &Step, value: Node) -> Option<()> {
    match (parent, step) {
        (Node::Map(members), Step::Key(key)) => members.push((Key::new(key.clone()), value)),
        (Node::List(elements), Step::Index(_)) => elements.push(value),
        _ => return None,
    }
    Some(())
}

fn remove(root: &mut Node, at: &[Step]) -> Option<()> {
    let (last, parents) = at.split_last()?;
    match (locate(root, parents)?, last) {
        (Node::Map(members), Step::Key(key)) => {
            let position = members.iter().position(|(name, _)| name.text == *key)?;
            members.remove(position);
        }
        (Node::List(elements), Step::Index(index)) if *index < elements.len() => {
            elements.remove(*index);
        }
        _ => return None,
    }
    Some(())
}

fn locate<'a>(root: &'a mut Node, at: &[Step]) -> Option<&'a mut Node> {
    at.iter().try_fold(root, |node, step| child(node, step))
}

fn child<'a>(node: &'a mut Node, step: &Step) -> Option<&'a mut Node> {
    match (node, step) {
        (Node::Map(members), Step::Key(key)) => members
            .iter_mut()
            .find(|(name, _)| name.text == *key)
            .map(|(_, member)| member),
        (Node::List(elements), Step::Index(index)) => elements.get_mut(*index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::error::{FormatError, YamlFeature};
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

    fn parsed(source: &str) -> Value {
        format::parse(Format::Yaml, source.as_bytes())
            .unwrap()
            .value
    }

    fn edited(source: &str, edits: &[Edit]) -> String {
        String::from_utf8(format::edit(Format::Yaml, source.as_bytes(), edits).unwrap()).unwrap()
    }

    #[test]
    fn parse_refuses_comments_anchors_aliases_tags_duplicate_keys_and_multiple_documents() {
        for (source, feature) in [
            ("# leading\na: 1\n", YamlFeature::Comment),
            ("a: 1 # trailing\n", YamlFeature::Comment),
            ("a:\n  - 1\n  # between\n  - 2\n", YamlFeature::Comment),
            ("a: {b: 1, # inside\n  c: 2}\n", YamlFeature::Comment),
            ("a: 1\n# last\n", YamlFeature::Comment),
            ("a: &x 1\n", YamlFeature::Anchor),
            ("a: &x\n  b: 1\n", YamlFeature::Anchor),
            ("a: !!str 1\n", YamlFeature::Tag),
            ("a: !local x\n", YamlFeature::Tag),
            ("a: !!map\n  b: 1\n", YamlFeature::Tag),
            ("a: 1\na: 2\n", YamlFeature::DuplicateKey),
            ("a: 1\n\"a\": 1\n", YamlFeature::DuplicateKey),
            ("o:\n  - {k: 1, k: 2}\n", YamlFeature::DuplicateKey),
            ("a: 1\n---\nb: 2\n", YamlFeature::MultipleDocuments),
            (
                "---\na: 1\n...\n---\na: 1\n",
                YamlFeature::MultipleDocuments,
            ),
        ] {
            assert_eq!(
                format::parse(Format::Yaml, source.as_bytes()).err(),
                Some(FormatError::UnsupportedYaml(feature)),
                "{source:?}",
            );
        }
        let aliased = format::parse(Format::Yaml, b"a: &x 1\nb: *x\n").err();
        assert!(
            matches!(
                aliased,
                Some(FormatError::UnsupportedYaml(
                    YamlFeature::Anchor | YamlFeature::Alias
                ))
            ),
            "{aliased:?}",
        );
        assert!(matches!(
            format::parse(Format::Yaml, b"a: [1\n"),
            Err(FormatError::Syntax { .. })
        ));
        assert_eq!(
            format::edit(
                Format::Yaml,
                b"a: 1 # mine\n",
                &[Edit::Remove { at: at(&["a"]) }],
            ),
            Err(FormatError::UnsupportedYaml(YamlFeature::Comment)),
        );

        let lookalikes = format::parse(
            Format::Yaml,
            br"---
hashed: 'x # y'
glued: x#y
quoted: 'it''s &a *b !c'
literal: |
  # kept
  line
...
",
        )
        .unwrap();
        assert_eq!(
            lookalikes.value,
            map([
                ("hashed", text("x # y")),
                ("glued", text("x#y")),
                ("quoted", text("it's &a *b !c")),
                ("literal", text("# kept\nline\n")),
            ]),
        );
        assert!(lookalikes.commented.is_empty());
        assert!(lookalikes.duplicated.is_empty());
    }

    #[test]
    fn parse_resolves_core_schema_scalars_keeping_timestamps_as_strings() {
        let empty = format::parse(Format::Yaml, b"").unwrap();
        assert_eq!(empty.value, map([]));
        assert!(empty.commented.is_empty());
        assert!(empty.duplicated.is_empty());

        assert_eq!(
            parsed(
                r#"stamp: 2001-12-14t21:59:43.10-05:00
day: 2002-12-14
int: 42
negative: -17
octal: 0o14
hex: 0x1A
max: 9223372036854775807
big: 9223372036854775808
small: -9223372036854775809
float: 1.5
exponent: 1e3
yes: yes
off: False
on: TRUE
none: null
tilde: ~
blank:
quoted: "42"
single: '1.5'
escaped: "q\"\u00e9"
"q k": hello world
flow: [1, two, {k: v}]
block:
  - - nested
"#
            ),
            map([
                ("stamp", text("2001-12-14t21:59:43.10-05:00")),
                ("day", text("2002-12-14")),
                ("int", Value::Integer(42)),
                ("negative", Value::Integer(-17)),
                ("octal", Value::Integer(12)),
                ("hex", Value::Integer(26)),
                ("max", Value::Integer(i64::MAX)),
                ("big", Value::BigInteger("9223372036854775808".to_owned())),
                (
                    "small",
                    Value::BigInteger("-9223372036854775809".to_owned())
                ),
                ("float", Value::Float(1.5)),
                ("exponent", Value::Float(1000.0)),
                ("yes", text("yes")),
                ("off", Value::Boolean(false)),
                ("on", Value::Boolean(true)),
                ("none", Value::Null),
                ("tilde", Value::Null),
                ("blank", Value::Null),
                ("quoted", text("42")),
                ("single", text("1.5")),
                ("escaped", text("q\"\u{e9}")),
                ("q k", text("hello world")),
                (
                    "flow",
                    Value::List(vec![
                        Value::Integer(1),
                        text("two"),
                        map([("k", text("v"))])
                    ]),
                ),
                (
                    "block",
                    Value::List(vec![Value::List(vec![text("nested")])])
                ),
            ]),
        );

        let Value::Map(special) = parsed("nan: .NaN\ninf: .inf\nfloor: -.Inf\n") else {
            panic!("a mapping parses as a map");
        };
        assert!(matches!(special.get("nan"), Some(Value::Float(nan)) if nan.is_nan()));
        assert_eq!(special.get("inf"), Some(&Value::Float(f64::INFINITY)));
        assert_eq!(special.get("floor"), Some(&Value::Float(f64::NEG_INFINITY)));

        let datetime = Value::Datetime(Datetime::parse("1979-05-27").unwrap());
        assert!(format::encodable(
            Format::Yaml,
            &map([
                ("none", Value::Null),
                ("list", Value::List(vec![Value::Float(1.5), text("x")])),
            ]),
        ));
        assert!(!format::encodable(Format::Yaml, &datetime));
        assert!(!format::encodable(
            Format::Yaml,
            &map([("deep", Value::List(vec![map([("when", datetime)])]))]),
        ));
    }

    const SOURCE: &str = r#"alpha: meros
owner: me
gone: 1
listing:
  - a
  - b
  - c
nested:
  keep: "42"
  stamp: 2001-12-14
  ratio: 1.5
  big: 9223372036854775808
  odd: "null"
  spaced: ' lead'
  multi: "one\ntwo"
hollow:
  only: x
"#;

    #[test]
    fn edit_rebuilds_the_document_changing_only_the_edited_units() {
        let output = edited(
            SOURCE,
            &[
                Edit::Replace {
                    at: at(&["owner"]),
                    value: text("you"),
                },
                Edit::Remove { at: at(&["gone"]) },
                Edit::Remove {
                    at: at(&["hollow", "only"]),
                },
                Edit::Remove {
                    at: element(&["listing"], 1),
                },
                Edit::Replace {
                    at: element(&["listing"], 2),
                    value: text("true"),
                },
                Edit::Create {
                    at: element(&["listing"], 3),
                    value: Value::Float(2.0),
                },
                Edit::Create {
                    at: at(&["nested", "zed"]),
                    value: map([("deep", Value::Null), ("empty", Value::List(vec![]))]),
                },
                Edit::Create {
                    at: at(&["fresh", "inner"]),
                    value: text("# not a comment"),
                },
            ],
        );
        assert_eq!(
            parsed(&output),
            map([
                ("alpha", text("meros")),
                ("owner", text("you")),
                (
                    "listing",
                    Value::List(vec![text("a"), text("true"), Value::Float(2.0)])
                ),
                (
                    "nested",
                    map([
                        ("keep", text("42")),
                        ("stamp", text("2001-12-14")),
                        ("ratio", Value::Float(1.5)),
                        ("big", Value::BigInteger("9223372036854775808".to_owned())),
                        ("odd", text("null")),
                        ("spaced", text(" lead")),
                        ("multi", text("one\ntwo")),
                        (
                            "zed",
                            map([("deep", Value::Null), ("empty", Value::List(vec![]))])
                        ),
                    ]),
                ),
                ("hollow", map([])),
                ("fresh", map([("inner", text("# not a comment"))])),
            ]),
        );
        let position = |key: &str| output.find(key).unwrap();
        for (before, after) in [
            ("alpha", "owner"),
            ("owner", "listing"),
            ("listing", "nested"),
            ("nested", "hollow"),
            ("hollow", "fresh"),
            ("multi", "zed"),
        ] {
            assert!(
                position(before) < position(after),
                "{before} precedes {after} in {output}"
            );
        }

        let created = [Edit::Create {
            at: at(&["a", "b"]),
            value: Value::Integer(1),
        }];
        assert_eq!(
            parsed(&edited("", &created)),
            map([("a", map([("b", Value::Integer(1))]))])
        );
        for unfit in [
            Edit::Replace {
                at: at(&["missing", "x"]),
                value: Value::Integer(1),
            },
            Edit::Create {
                at: at(&["alpha", "x"]),
                value: Value::Integer(1),
            },
        ] {
            assert_eq!(
                format::edit(Format::Yaml, SOURCE.as_bytes(), &[unfit]),
                Err(FormatError::UnfitEdit),
            );
        }
    }

    #[test]
    fn edit_writes_plain_keys_back_plain_and_quotes_new_keys_as_needed() {
        assert_eq!(
            edited(
                "404: x\n0x1A: y\n~: z\ntrue: w\nresponses:\n  404: missing\ngone: 1\n",
                &[
                    Edit::Remove { at: at(&["gone"]) },
                    Edit::Create {
                        at: at(&["responses", "500"]),
                        value: text("failed"),
                    },
                ],
            ),
            "404: x\n0x1A: \"y\"\n~: z\ntrue: w\nresponses:\n  404: missing\n  \"500\": failed\n",
        );
    }

    #[test]
    fn parse_refuses_keys_equal_under_the_core_schema() {
        for source in [
            "0x1A: a\n26: b\n",
            "~: a\nnull: b\n",
            "true: a\nTrue: b\n",
            "m: {1e3: a, 1000.0: b}\n",
            "\"404\": a\n404: b\n",
        ] {
            assert_eq!(
                format::parse(Format::Yaml, source.as_bytes()).err(),
                Some(FormatError::UnsupportedYaml(YamlFeature::DuplicateKey)),
                "{source:?}",
            );
        }
    }
}
