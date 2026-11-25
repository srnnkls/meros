use std::fmt::{self, Write as _};

use crate::error::PathError;
use crate::schema::Identity;

/// A validated taxis path: keys and `[field=literal]` selectors; `[]` only in schema paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub(crate) segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    Key(String),
    Select {
        field: String,
        literal: Literal,
    },
    /// `[]`: any element or member.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Literal {
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
        let mut parser = Parser { input, at: 0 };
        let mut segments = Vec::new();
        loop {
            segments.push(Segment::Key(parser.key()?));
            if parser.eat('[') {
                segments.push(parser.selector()?);
            }
            match parser.peek() {
                None => return Ok(Self { segments }),
                Some('.') => parser.at += 1,
                Some(_) => return Err(parser.syntax(parser.at)),
            }
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.input[self.at..].chars().next()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, expected: char) -> bool {
        let found = self.peek() == Some(expected);
        if found {
            self.at += expected.len_utf8();
        }
        found
    }

    fn expect(&mut self, expected: char) -> Result<(), PathError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(self.syntax(self.at))
        }
    }

    fn syntax(&self, at: usize) -> PathError {
        PathError::Syntax {
            path: self.input.to_owned(),
            at,
        }
    }

    fn word(&mut self, continues: impl Fn(char) -> bool) -> &'a str {
        let start = self.at;
        while self.peek().is_some_and(&continues) {
            self.at += 1;
        }
        &self.input[start..self.at]
    }

    /// Reads one quoted run, or else one run of `bare` characters.
    fn run(&mut self, bare: impl Fn(char) -> bool) -> Option<&'a str> {
        match self.peek() {
            Some(quote @ ('"' | '\'')) => self.quoted(quote),
            _ => Some(self.word(bare)),
        }
    }

    fn key(&mut self) -> Result<String, PathError> {
        let start = self.at;
        self.run(is_bare_key)
            .and_then(|run| run.parse::<toml_edit::Key>().ok())
            .map(|key| key.get().to_owned())
            .ok_or_else(|| self.syntax(start))
    }

    /// Parses a selector after its opening `[`.
    fn selector(&mut self) -> Result<Segment, PathError> {
        let open = self.at - 1;
        if self.eat(']') {
            return Ok(Segment::Any);
        }
        let field_start = self.at;
        let field = self.key()?;
        if self.peek() == Some(']')
            && matches!(
                self.input[field_start..self.at].parse(),
                Ok(toml_edit::Value::Integer(_))
            )
        {
            return Err(PathError::IndexSelector {
                path: self.input.to_owned(),
                at: open,
            });
        }
        self.expect('=')?;
        let literal = self.literal()?;
        self.expect(']')?;
        Ok(Segment::Select { field, literal })
    }

    fn literal(&mut self) -> Result<Literal, PathError> {
        let start = self.at;
        match self
            .run(|c| is_bare_key(c) || c == '+')
            .and_then(|run| run.parse().ok())
        {
            Some(toml_edit::Value::String(text)) => Ok(Literal::String(text.into_value())),
            Some(toml_edit::Value::Integer(number)) => Ok(Literal::Integer(number.into_value())),
            Some(toml_edit::Value::Boolean(boolean)) => Ok(Literal::Boolean(boolean.into_value())),
            _ => Err(self.syntax(start)),
        }
    }

    /// Reads a basic (`"`) or literal (`'`) string, single- or multi-line,
    /// up to its unescaped closing delimiter.
    fn quoted(&mut self, quote: char) -> Option<&'a str> {
        let start = self.at;
        let triple = if quote == '"' { r#"""""# } else { "'''" };
        let delimiter = if self.input[start..].starts_with(triple) {
            triple
        } else {
            &triple[..1]
        };
        self.at += delimiter.len();
        loop {
            if self.input[self.at..].starts_with(delimiter) {
                self.at += delimiter.len();
                if delimiter == triple {
                    // Up to two quotes after a multi-line delimiter belong to the content.
                    while self.eat(quote) {}
                }
                return Some(&self.input[start..self.at]);
            }
            if self.next()? == '\\' && quote == '"' {
                self.next()?;
            }
        }
    }
}

fn is_bare_key(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Key(key) => {
                    if index > 0 {
                        f.write_char('.')?;
                    }
                    write_key(f, key)?;
                }
                Segment::Select { field, literal } => {
                    f.write_char('[')?;
                    write_key(f, field)?;
                    let literal = match literal {
                        Literal::String(text) => toml_edit::Value::from(text.as_str()),
                        Literal::Integer(number) => toml_edit::Value::from(*number),
                        Literal::Boolean(boolean) => toml_edit::Value::from(*boolean),
                    };
                    write!(f, "={literal}]")?;
                }
                Segment::Any => f.write_str("[]")?,
            }
        }
        Ok(())
    }
}

fn write_key(f: &mut fmt::Formatter<'_>, key: &str) -> fmt::Result {
    f.write_str(&toml_edit::Key::new(key).display_repr())
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Atomic => f.write_str("atomic"),
            Self::Set => f.write_str("set"),
            Self::Keyed(field) => write!(f, "keyed({field})"),
            Self::Members => f.write_str("members"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Literal, Path, Segment};
    use crate::error::{PathError, SchemaError};
    use crate::schema::Identity;

    fn key(name: &str) -> Segment {
        Segment::Key(name.to_owned())
    }

    fn segments(input: &str) -> Vec<Segment> {
        Path::parse(input).unwrap().segments
    }

    #[test]
    fn parse_reads_keys_selectors_and_any_element() {
        assert_eq!(segments("lint.select"), [key("lint"), key("select")]);
        assert_eq!(
            segments(r#"lint."per-file-ignores"."tests/*""#),
            [key("lint"), key("per-file-ignores"), key("tests/*")]
        );
        assert_eq!(segments(r#""a.b"."a\"b""#), [key("a.b"), key("a\"b")]);
        assert_eq!(segments("'x.y'"), [key("x.y")]);
        assert_eq!(
            segments(r#"repos[id="ruff"].args"#),
            [
                key("repos"),
                Segment::Select {
                    field: "id".to_owned(),
                    literal: Literal::String("ruff".to_owned()),
                },
                key("args"),
            ]
        );
        assert_eq!(
            segments(r#"servers["port"=8080].hosts[enabled=true]"#),
            [
                key("servers"),
                Segment::Select {
                    field: "port".to_owned(),
                    literal: Literal::Integer(8080),
                },
                key("hosts"),
                Segment::Select {
                    field: "enabled".to_owned(),
                    literal: Literal::Boolean(true),
                },
            ]
        );
        assert_eq!(
            segments("repos[id='ruff']"),
            segments(r#"repos[id="ruff"]"#)
        );
        assert_eq!(
            segments("repos[].args"),
            [key("repos"), Segment::Any, key("args")]
        );
        for (input, field, value) in [
            ("repos[port=0x1F90]", "port", 8080),
            ("repos[id=0o20]", "id", 16),
            ("repos[id=0b10000]", "id", 16),
        ] {
            assert_eq!(
                segments(input),
                [
                    key("repos"),
                    Segment::Select {
                        field: field.to_owned(),
                        literal: Literal::Integer(value),
                    },
                ]
            );
        }
        assert_eq!(segments(r#"a."\x41""#), [key("a"), key("A")]);
        assert_eq!(segments(r#"a."\e""#), [key("a"), key("\u{1b}")]);
    }

    #[test]
    fn parse_rejects_index_selectors_and_malformed_paths() {
        assert_eq!(
            Path::parse("repos[0]"),
            Err(PathError::IndexSelector {
                path: "repos[0]".to_owned(),
                at: 5,
            })
        );
        assert_eq!(
            Path::parse("a..b"),
            Err(PathError::Syntax {
                path: "a..b".to_owned(),
                at: 2,
            })
        );
        assert_eq!(
            Path::parse("a."),
            Err(PathError::Syntax {
                path: "a.".to_owned(),
                at: 2,
            })
        );
        assert_eq!(
            Path::parse(""),
            Err(PathError::Syntax {
                path: String::new(),
                at: 0,
            })
        );
        for malformed in [
            "a b",
            "[]",
            "a[][]",
            "a[id=]",
            "a[id=ruff]",
            r#"a[id="ruff""#,
            r#"a[="ruff"]"#,
            r#""open.key"#,
        ] {
            assert!(
                matches!(Path::parse(malformed), Err(PathError::Syntax { .. })),
                "{malformed} must be a syntax error"
            );
        }
    }

    #[test]
    fn display_writes_canonical_taxis_text() {
        let display = |input: &str| Path::parse(input).unwrap().to_string();

        assert_eq!(
            display(r#""lint"."per-file-ignores"."tests/*""#),
            r#"lint.per-file-ignores."tests/*""#
        );
        assert_eq!(display(r#""a\"b".c"#), r#"'a"b'.c"#);
        assert_eq!(
            display(r"repos['id'='ruff'].args"),
            r#"repos[id="ruff"].args"#
        );
        assert_eq!(
            display("servers[port=8080].hosts[enabled=false]"),
            "servers[port=8080].hosts[enabled=false]"
        );
        assert_eq!(display("repos[].args"), "repos[].args");
        for input in [r#"repos[id="a\nb"]"#, r#"repos[id="a\"b'c"]"#] {
            let path = Path::parse(input).unwrap();
            assert_eq!(Path::parse(&path.to_string()), Ok(path), "{input}");
        }

        assert_eq!(Identity::Atomic.to_string(), "atomic");
        assert_eq!(Identity::Set.to_string(), "set");
        assert_eq!(Identity::Keyed("id".to_owned()).to_string(), "keyed(id)");
        assert_eq!(Identity::Members.to_string(), "members");

        let nested = SchemaError::NestedInAtomic {
            path: Path::parse(r#"tool."x y".z"#).unwrap(),
            atomic: Path::parse(r#"tool."x y""#).unwrap(),
        };
        assert_eq!(
            nested.to_string(),
            r#"schema entry tool."x y".z lies inside the atomic unit tool."x y""#
        );
    }
}
