use crate::error::PathError;

/// A validated taxis path: keys and `[field=literal]` selectors; `[]` only in schema paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub(crate) segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Select {
        field: String,
        literal: Literal,
    },
    /// `[]`: any element or member.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    String(String),
    Integer(i64),
    Boolean(bool),
}

impl Path {
    /// Parses taxis's path grammar, rejecting index selectors.
    ///
    /// # Errors
    ///
    /// Fails when `input` is not a valid taxis path.
    pub fn parse(input: &str) -> Result<Self, PathError> {
        let _ = input;
        todo!()
    }
}
