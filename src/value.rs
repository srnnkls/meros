use std::collections::BTreeMap;

use serde_json::{Map, Number, Value as Json};
use sha2::{Digest, Sha256};

use crate::format::Format;

/// A parsed value of any supported format.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A table, object or mapping.
    Map(BTreeMap<String, Value>),
    /// An array or sequence.
    List(Vec<Value>),
    /// A string.
    String(String),
    /// An integer.
    Integer(i64),
    /// An integer outside `i64`, kept as its document text; it has no canonical projection.
    BigInteger(String),
    /// A floating-point number.
    Float(f64),
    /// A boolean.
    Boolean(bool),
    /// A TOML datetime.
    Datetime(Datetime),
    /// A JSON or YAML null.
    Null,
}

/// A TOML datetime kept as its TOML text, with the kind that text has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datetime {
    kind: DatetimeKind,
    text: String,
}

impl Datetime {
    /// Parses TOML datetime text, deriving its kind; `None` when the text is no TOML datetime.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let parsed: toml_edit::Datetime = text.parse().ok()?;
        let kind = match (parsed.date, parsed.time, parsed.offset) {
            (Some(_), Some(_), Some(_)) => DatetimeKind::Offset,
            (Some(_), Some(_), None) => DatetimeKind::LocalDatetime,
            (Some(_), None, None) => DatetimeKind::LocalDate,
            (None, Some(_), None) => DatetimeKind::LocalTime,
            _ => return None,
        };
        Some(Self {
            kind,
            text: parsed.to_string(),
        })
    }

    /// Which of TOML's four datetime kinds the text is.
    #[must_use]
    pub fn kind(&self) -> DatetimeKind {
        self.kind
    }

    /// The datetime's TOML text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// TOML's datetime kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatetimeKind {
    /// A date and time with a UTC offset.
    Offset,
    /// A date and time without an offset.
    LocalDatetime,
    /// A date alone.
    LocalDate,
    /// A time alone.
    LocalTime,
}

impl DatetimeKind {
    fn name(self) -> &'static str {
        match self {
            Self::Offset => "offset",
            Self::LocalDatetime => "local-datetime",
            Self::LocalDate => "local-date",
            Self::LocalTime => "local-time",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unprojectable;

const MAX_SAFE_INTEGER: u64 = 1 << 53;

fn canonical_json(value: &Value, format: Format) -> Result<String, Unprojectable> {
    let projection = project(value, format)?;
    #[expect(
        clippy::expect_used,
        reason = "a projection holds only strings, finite numbers and JSON containers"
    )]
    Ok(serde_json_canonicalizer::to_string(&projection).expect("projection serializes"))
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read by unit derivation, whose body is `todo!()`")
)]
pub(crate) fn canonical_hash(value: &Value, format: Format) -> Result<[u8; 32], Unprojectable> {
    Ok(Sha256::digest(canonical_json(value, format)?).into())
}

fn project(value: &Value, format: Format) -> Result<Json, Unprojectable> {
    Ok(match value {
        Value::Map(members) => Json::Object(
            members
                .iter()
                .map(|(key, member)| Ok((escape(key), project(member, format)?)))
                .collect::<Result<Map<_, _>, _>>()?,
        ),
        Value::List(elements) => Json::Array(
            elements
                .iter()
                .map(|element| project(element, format))
                .collect::<Result<_, _>>()?,
        ),
        Value::String(text) => Json::String(text.clone()),
        Value::Integer(integer) if integer.unsigned_abs() <= MAX_SAFE_INTEGER => {
            Json::from(*integer)
        }
        Value::Float(float) if float.is_finite() => match format {
            Format::Json => Number::from_f64(*float)
                .map(Json::Number)
                .ok_or(Unprojectable)?,
            Format::Toml | Format::Yaml => tagged("$float", float.to_string()),
        },
        Value::Boolean(boolean) => Json::Bool(*boolean),
        Value::Datetime(datetime) => tagged(
            "$datetime",
            format!("{}:{}", datetime.kind.name(), datetime.text),
        ),
        Value::Null => Json::Null,
        Value::Integer(_) | Value::BigInteger(_) | Value::Float(_) => return Err(Unprojectable),
    })
}

fn escape(key: &str) -> String {
    if key.starts_with('$') {
        format!("${key}")
    } else {
        key.to_owned()
    }
}

fn tagged(tag: &str, text: String) -> Json {
    Json::Object(Map::from_iter([(tag.to_owned(), Json::String(text))]))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Datetime, Value, canonical_hash, canonical_json};
    use crate::format::Format;

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

    #[test]
    fn canonical_json_projects_typed_values_with_escaped_keys() {
        let toml = map([
            ("time", datetime("07:32:00")),
            (
                "n",
                Value::List(vec![
                    Value::Integer(1),
                    Value::Float(1.0),
                    Value::Boolean(true),
                    Value::String("x".to_owned()),
                ]),
            ),
            ("local", datetime("1979-05-27T07:32:00")),
            ("date", datetime("1979-05-27")),
            ("at", datetime("1979-05-27T07:32:00Z")),
            ("$schema", Value::String("s".to_owned())),
        ]);
        assert_eq!(
            canonical_json(&toml, Format::Toml).unwrap(),
            concat!(
                r#"{"$$schema":"s","#,
                r#""at":{"$datetime":"offset:1979-05-27T07:32:00Z"},"#,
                r#""date":{"$datetime":"local-date:1979-05-27"},"#,
                r#""local":{"$datetime":"local-datetime:1979-05-27T07:32:00"},"#,
                r#""n":[1,{"$float":"1"},true,"x"],"#,
                r#""time":{"$datetime":"local-time:07:32:00"}}"#,
            ),
        );

        let json = map([
            ("x", Value::Float(0.5)),
            ("n", Value::Float(100.0)),
            ("$float", Value::Null),
        ]);
        assert_eq!(
            canonical_json(&json, Format::Json).unwrap(),
            r#"{"$$float":null,"n":100,"x":0.5}"#,
        );
    }

    #[test]
    fn canonical_hash_compares_json_numbers_by_value_and_refuses_unprojectable_values() {
        let hash = |value: &Value, format| canonical_hash(value, format).unwrap();
        assert_eq!(
            hash(&Value::Float(1.0), Format::Json),
            hash(&Value::Integer(1), Format::Json),
        );
        assert_ne!(
            hash(&Value::Float(1.0), Format::Toml),
            hash(&Value::Integer(1), Format::Toml),
        );
        assert_ne!(
            hash(&Value::Float(1.0), Format::Yaml),
            hash(&Value::Integer(1), Format::Yaml),
        );
        assert_ne!(
            hash(&datetime("1979-05-27"), Format::Toml),
            hash(&datetime("1979-05-27T00:00:00Z"), Format::Toml),
        );
        assert_ne!(
            hash(&Value::String("a".to_owned()), Format::Json),
            hash(&Value::String("b".to_owned()), Format::Json),
        );

        let nested = |leaf| map([("a", Value::List(vec![Value::Boolean(true), leaf]))]);
        for format in [Format::Json, Format::Toml, Format::Yaml] {
            assert!(canonical_hash(&nested(Value::Integer(9_007_199_254_740_992)), format).is_ok());
            assert!(
                canonical_hash(&nested(Value::Integer(-9_007_199_254_740_992)), format).is_ok()
            );
            for leaf in [
                Value::Float(f64::NAN),
                Value::Float(f64::INFINITY),
                Value::Float(f64::NEG_INFINITY),
                Value::Integer(9_007_199_254_740_993),
                Value::Integer(-9_007_199_254_740_993),
            ] {
                assert!(
                    canonical_hash(&nested(leaf.clone()), format).is_err(),
                    "{leaf:?} in {format:?} must be unprojectable",
                );
            }
        }
    }
}
