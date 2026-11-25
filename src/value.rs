use std::collections::BTreeMap;

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
    /// A floating-point number.
    Float(f64),
    /// A boolean.
    Boolean(bool),
    /// A TOML datetime.
    Datetime(Datetime),
    /// A JSON or YAML null.
    Null,
}

/// A TOML datetime kept as its TOML text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datetime {
    /// Which of TOML's four datetime kinds the text is.
    pub kind: DatetimeKind,
    /// The datetime's TOML text.
    pub text: String,
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
