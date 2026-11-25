use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Map(BTreeMap<String, Value>),
    List(Vec<Value>),
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Datetime(Datetime),
    Null,
}

/// A TOML datetime kept as its TOML text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datetime {
    pub kind: DatetimeKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatetimeKind {
    Offset,
    LocalDatetime,
    LocalDate,
    LocalTime,
}
