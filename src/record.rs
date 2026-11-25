use std::collections::BTreeMap;

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};

use crate::unit::UnitKey;

/// What the tool last wrote in one file; the caller stores it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Record {
    /// The hash scheme the entries use.
    pub scheme: Scheme,
    /// The owned units, sorted by key.
    pub entries: BTreeMap<UnitKey, Entry>,
}

impl<'de> Deserialize<'de> for Record {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            #[serde(default = "Scheme::missing")]
            scheme: Scheme,
            entries: Option<serde_json::Value>,
        }

        let Stored { scheme, entries } = Stored::deserialize(deserializer)?;
        let entries = match scheme {
            Scheme::Meros1 => {
                let entries = entries.ok_or_else(|| D::Error::missing_field("entries"))?;
                serde_json::from_value(entries).map_err(D::Error::custom)?
            }
            Scheme::Unknown => BTreeMap::new(),
        };
        Ok(Self { scheme, entries })
    }
}

/// The hash scheme of a record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scheme {
    /// `meros-1`: SHA-256 over RFC 8785 JSON of the typed projection.
    #[default]
    #[serde(rename = "meros-1")]
    Meros1,
    /// A missing or unrecognized scheme: the record counts as no record.
    #[serde(other)]
    Unknown,
}

impl Scheme {
    fn missing() -> Self {
        Self::Unknown
    }
}

/// What the tool last wrote at one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The canonical hash of the written value.
    #[serde(with = "hex")]
    pub hash: [u8; 32],
    /// Marks a `CreateOnly` write.
    pub seeded: bool,
}

mod hex {
    use std::fmt::Write;

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        hash: &[u8; 32],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let text = hash
            .iter()
            .fold(String::with_capacity(64), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            });
        serializer.serialize_str(&text)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        let invalid = || D::Error::custom("hash is not 64 hex digits");
        if text.len() != 64 {
            return Err(invalid());
        }
        let digit = |byte: u8| {
            char::from(byte)
                .to_digit(16)
                .and_then(|digit| u8::try_from(digit).ok())
        };
        let mut hash = [0; 32];
        let (pairs, _) = text.as_bytes().as_chunks::<2>();
        for (byte, &[high, low]) in hash.iter_mut().zip(pairs) {
            let (high, low) = digit(high).zip(digit(low)).ok_or_else(invalid)?;
            *byte = high << 4 | low;
        }
        Ok(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::{Record, Scheme};

    const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const HASH_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    #[test]
    fn record_serializes_scheme_and_sorted_hex_entries_and_reads_unknown_schemes() {
        let unsorted = format!(
            r#"{{"scheme":"meros-1","entries":{{"b":{{"hash":"{HASH_B}","seeded":true}},"a":{{"hash":"{HASH_A}","seeded":false}}}}}}"#
        );
        let record: Record = serde_json::from_str(&unsorted).unwrap();
        assert_eq!(record.scheme, Scheme::Meros1);
        let first = record.entries.values().next().unwrap();
        assert_eq!(
            first.hash[..8],
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]
        );
        assert!(!first.seeded);

        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            format!(
                r#"{{"scheme":"meros-1","entries":{{"a":{{"hash":"{HASH_A}","seeded":false}},"b":{{"hash":"{HASH_B}","seeded":true}}}}}}"#
            ),
        );

        for unknown in [r#"{"scheme":"meros-9","entries":{}}"#, r#"{"entries":{}}"#] {
            let record: Record = serde_json::from_str(unknown).unwrap();
            assert_eq!(record.scheme, Scheme::Unknown, "{unknown}");
        }

        let long_hash = format!(
            r#"{{"scheme":"meros-2","entries":{{"a":{{"hash":"{HASH_A}{HASH_B}","seeded":false}}}}}}"#
        );
        for unknown in [
            r#"{"scheme":"meros-2"}"#,
            r#"{"scheme":"meros-2","entries":{"a":{"hash":"00","seeded":false}}}"#,
            r#"{"scheme":"meros-2","entries":{"a":{"digest":"00"}}}"#,
            &long_hash,
        ] {
            let record: Record = serde_json::from_str(unknown).unwrap();
            assert_eq!(record.scheme, Scheme::Unknown, "{unknown}");
            assert!(record.entries.is_empty(), "{unknown}");
        }
    }
}
