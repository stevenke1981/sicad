//! Validated identifier newtypes (spec §1.2, §1.3).
//!
//! * [`Id`] — `^[a-z][a-z0-9_]{0,31}$`
//! * [`PortRef`] — `id.port` (constraint `a`/`b`)
//! * [`FieldRef`] — `id:field` (binding targets, expressions, `set_field` targets)
//!
//! All three validate on construction ([`FromStr`], [`TryFrom<String>`]) and on deserialize,
//! serialize as plain JSON strings, and order/hash by their string form (so `BTreeMap` keys sort
//! exactly like JSON object keys sorted by a byte-wise string comparison).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

/// Maximum length of an [`Id`] in bytes (all valid ids are ASCII).
pub const ID_MAX_LEN: usize = 32;
/// JSON-Schema pattern of an [`Id`].
pub const ID_PATTERN: &str = "^[a-z][a-z0-9_]{0,31}$";
/// JSON-Schema pattern of a [`PortRef`].
pub const PORT_REF_PATTERN: &str = "^[a-z][a-z0-9_]{0,31}\\.[a-z][a-z0-9_]{0,31}$";
/// JSON-Schema pattern of a [`FieldRef`].
pub const FIELD_REF_PATTERN: &str = "^[a-z][a-z0-9_]{0,31}:[a-z][a-z0-9_]{0,31}$";

/// Error returned when a string is not a valid [`Id`], [`PortRef`] or [`FieldRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError {
    /// Which syntax was expected (`"id"`, `"id.port"` or `"id:field"`).
    pub expected: &'static str,
    /// The rejected input.
    pub input: String,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pattern = match self.expected {
            "id.port" => PORT_REF_PATTERN,
            "id:field" => FIELD_REF_PATTERN,
            _ => ID_PATTERN,
        };
        write!(
            f,
            "invalid {} {:?}: must match {}",
            self.expected, self.input, pattern
        )
    }
}

impl std::error::Error for IdError {}

/// Returns `true` when `s` matches `^[a-z][a-z0-9_]{0,31}$`.
pub fn is_valid_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > ID_MAX_LEN {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// Splits `s` at the single separator `sep` and validates both halves as ids.
fn split_ref(s: &str, sep: char) -> Option<usize> {
    let idx = s.find(sep)?;
    let (left, right) = (&s[..idx], &s[idx + 1..]);
    (is_valid_id(left) && is_valid_id(right)).then_some(idx)
}

// ---------------------------------------------------------------------------------------------

/// A validated identifier: `^[a-z][a-z0-9_]{0,31}$` (spec §1.2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(String);

impl Id {
    /// Validates and wraps `s`.
    pub fn new(s: impl Into<String>) -> Result<Self, IdError> {
        let s = s.into();
        if is_valid_id(&s) {
            Ok(Self(s))
        } else {
            Err(IdError {
                expected: "id",
                input: s,
            })
        }
    }

    /// The identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Builds the port reference `self.port`.
    pub fn port(&self, port: &Id) -> PortRef {
        PortRef::from_parts(self, port)
    }

    /// Builds the field reference `self:field`.
    pub fn field(&self, field: &Id) -> FieldRef {
        FieldRef::from_parts(self, field)
    }
}

impl TryFrom<String> for Id {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for Id {
    type Error = IdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl FromStr for Id {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl From<Id> for String {
    fn from(id: Id) -> String {
        id.0
    }
}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for Id {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for Id {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for Id {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:?})", self.0)
    }
}

impl JsonSchema for Id {
    fn schema_name() -> Cow<'static, str> {
        "Id".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": ID_PATTERN,
            "minLength": 1,
            "maxLength": ID_MAX_LEN,
        })
    }
}

// ---------------------------------------------------------------------------------------------

macro_rules! two_part_ref {
    (
        $(#[$meta:meta])*
        $name:ident, sep = $sep:literal, expected = $expected:literal,
        pattern = $pattern:expr, schema_name = $schema_name:literal,
        second = $second:ident
    ) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name {
            raw: String,
            sep: usize,
        }

        impl $name {
            /// Validates and wraps `s`.
            pub fn new(s: impl Into<String>) -> Result<Self, IdError> {
                let raw = s.into();
                match split_ref(&raw, $sep) {
                    Some(sep) => Ok(Self { raw, sep }),
                    None => Err(IdError { expected: $expected, input: raw }),
                }
            }

            /// Joins two already-valid ids.
            pub fn from_parts(id: &Id, second: &Id) -> Self {
                let raw = format!("{}{}{}", id, $sep, second);
                Self { sep: id.as_str().len(), raw }
            }

            /// The component/constraint id part.
            pub fn id_str(&self) -> &str {
                &self.raw[..self.sep]
            }

            /// The id part as an owned [`Id`].
            pub fn id(&self) -> Id {
                Id(self.id_str().to_owned())
            }

            /// The second part as text.
            pub fn $second(&self) -> &str {
                &self.raw[self.sep + 1..]
            }

            /// The whole reference text.
            pub fn as_str(&self) -> &str {
                &self.raw
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::new(s)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;
            fn try_from(s: &str) -> Result<Self, Self::Error> {
                Self::new(s)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl From<$name> for String {
            fn from(r: $name) -> String {
                r.raw
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.raw
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.raw
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.raw)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.raw)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                $schema_name.into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({ "type": "string", "pattern": $pattern })
            }
        }
    };
}

two_part_ref!(
    /// A port reference `id.port` (spec §1.3), used by constraint `a`/`b`.
    PortRef,
    sep = '.',
    expected = "id.port",
    pattern = PORT_REF_PATTERN,
    schema_name = "PortRef",
    second = port
);

two_part_ref!(
    /// A field reference `id:field` (spec §1.3), used by binding targets and `set_field`.
    FieldRef,
    sep = ':',
    expected = "id:field",
    pattern = FIELD_REF_PATTERN,
    schema_name = "FieldRef",
    second = field
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_accepts_and_rejects() {
        for ok in [
            "a",
            "v_plate",
            "k_d1",
            "a0_",
            "abcdefghijklmnopqrstuvwxyz012345",
        ] {
            assert!(Id::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "A",
            "1a",
            "_a",
            "a-b",
            "a.b",
            "a:b",
            "aB",
            "é",
            " a",
            "abcdefghijklmnopqrstuvwxyz0123456",
        ] {
            assert!(Id::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn port_ref_parts() {
        let p: PortRef = "v_plate.top".parse().unwrap();
        assert_eq!(p.id_str(), "v_plate");
        assert_eq!(p.id(), Id::new("v_plate").unwrap());
        assert_eq!(p.port(), "top");
        assert_eq!(p.to_string(), "v_plate.top");
        for bad in [
            "v_plate",
            "v_plate.",
            ".top",
            "v_plate:top",
            "a.b.c",
            "A.b",
            "a.B",
        ] {
            assert!(PortRef::new(bad).is_err(), "{bad}");
        }
        let built = Id::new("e_h1").unwrap().port(&Id::new("axis").unwrap());
        assert_eq!(built, PortRef::new("e_h1.axis").unwrap());
    }

    #[test]
    fn field_ref_parts() {
        let f: FieldRef = "k_d1:u".parse().unwrap();
        assert_eq!(f.id_str(), "k_d1");
        assert_eq!(f.field(), "u");
        for bad in ["k_d1.u", "k_d1:", ":u", "a:b:c", "a:B"] {
            assert!(FieldRef::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn serde_validates() {
        let id: Id = serde_json::from_str("\"v_body\"").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"v_body\"");
        assert!(serde_json::from_str::<Id>("\"V\"").is_err());
        assert!(serde_json::from_str::<PortRef>("\"a:b\"").is_err());
        assert!(serde_json::from_str::<FieldRef>("\"a.b\"").is_err());
        let p: PortRef = serde_json::from_str("\"a.b\"").unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"a.b\"");
    }

    #[test]
    fn ordering_is_by_string() {
        // ':' (0x3A) sorts after '0' (0x30): string order, not (id, field) tuple order.
        let a = FieldRef::new("a:z").unwrap();
        let b = FieldRef::new("a0:b").unwrap();
        assert!(a > b);
        assert_eq!(a.as_str().cmp(b.as_str()), a.cmp(&b));
    }

    #[test]
    fn schema_has_pattern() {
        let s = schemars::schema_for!(PortRef);
        assert_eq!(
            s.get("pattern").and_then(|v| v.as_str()),
            Some(PORT_REF_PATTERN)
        );
    }
}
