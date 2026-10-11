//! Hierarchical instance and port addresses.
//!
//! Grammar (canonical text, nothing else is accepted):
//!
//! ```text
//! instance = ( "/" name )+          e.g. /drums/kick/amp_env
//! port     = instance "." name      e.g. /drums/kick/amp_env.out
//! name     = [a-z] [a-z0-9_]*
//! ```
//!
//! An address is stored as its canonical text, so parsing and display round-trip
//! exactly. `Ord` is the byte-wise order of that text, which ADR 0016 uses for
//! schedule tie-breaks (§1) and for ordering same-frame events by source port (§3).
//! It equals segment-wise order (compare the segment lists lexicographically,
//! each segment byte-wise, a proper prefix first) because `/` (0x2F) sorts below
//! every character a segment may contain (`0-9` 0x30–0x39, `_` 0x5F, `a-z`
//! 0x61–0x7A). For the same reason, and because `.` (0x2E) sorts below `/`, the
//! order of port addresses equals ordering by instance address, then port name.

use std::fmt;
use std::str::FromStr;

/// The address of an instance in the flattened graph, e.g. `/drums/kick/amp_env`.
///
/// One or more segments, each `[a-z][a-z0-9_]*`, each preceded by `/`. See the
/// module documentation for why the byte-wise `Ord` equals segment-wise order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceAddress(String);

impl InstanceAddress {
    /// The canonical text, e.g. `/drums/kick`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The segments in order, without separators.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        // Canonical text always starts with '/', so the first split item is empty.
        self.0.split('/').skip(1)
    }

    /// The address of a child instance: `self` + `/` + `segment`.
    pub fn child(&self, segment: &str) -> Result<InstanceAddress, AddressError> {
        let offset = self.0.len() + 1;
        check_name(segment, offset)?;
        Ok(InstanceAddress(format!("{}/{segment}", self.0)))
    }
}

impl FromStr for InstanceAddress {
    type Err = AddressError;

    fn from_str(text: &str) -> Result<Self, AddressError> {
        check_instance(text)?;
        Ok(InstanceAddress(text.to_owned()))
    }
}

impl fmt::Display for InstanceAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The address of a port of an instance: `<instance>.<port>`, e.g.
/// `/drums/kick/amp_env.out`. Parameters are input ports and use the same form.
///
/// The port name follows the segment grammar `[a-z][a-z0-9_]*`. `Ord` is the
/// byte-wise order of the canonical text; see the module documentation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortAddress {
    // `text` comes first so the derived order is the order of the text alone
    // (`dot` is a function of `text`).
    text: String,
    dot: usize,
}

impl PortAddress {
    /// The address of port `port` of `instance`.
    pub fn new(instance: &InstanceAddress, port: &str) -> Result<PortAddress, AddressError> {
        let dot = instance.0.len();
        check_name(port, dot + 1)?;
        Ok(PortAddress {
            text: format!("{}.{port}", instance.0),
            dot,
        })
    }

    /// The canonical text, e.g. `/drums/kick.out`.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The instance part.
    pub fn instance(&self) -> InstanceAddress {
        let (instance, _) = self.split();
        InstanceAddress(instance.to_owned())
    }

    /// The port name (the part after `.`).
    pub fn port(&self) -> &str {
        self.split().1
    }

    fn split(&self) -> (&str, &str) {
        match self.text.split_at_checked(self.dot) {
            // `rest` starts with the '.'.
            Some((instance, rest)) => (instance, rest.get(1..).unwrap_or("")),
            None => ("", ""),
        }
    }
}

impl FromStr for PortAddress {
    type Err = AddressError;

    fn from_str(text: &str) -> Result<Self, AddressError> {
        let Some((instance, port)) = text.split_once('.') else {
            check_instance(text)?;
            return Err(AddressError::MissingPort);
        };
        check_instance(instance)?;
        check_name(port, instance.len() + 1)?;
        Ok(PortAddress {
            text: text.to_owned(),
            dot: instance.len(),
        })
    }
}

impl fmt::Display for PortAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Why a text is not a canonical address. Positions are byte offsets into the
/// text that was parsed (for [`InstanceAddress::child`] and [`PortAddress::new`],
/// into the address being built).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressError {
    /// An instance address must start with `/`.
    MissingLeadingSlash,
    /// A segment or port name is empty (`//`, a trailing `/` or `.`, or a bare `/`).
    EmptyName {
        /// Byte offset where the name should start.
        position: usize,
    },
    /// A segment or port name must start with `a`–`z`.
    InvalidStart {
        /// Byte offset of the offending character.
        position: usize,
    },
    /// A segment or port name may contain only `a`–`z`, `0`–`9` and `_`.
    InvalidCharacter {
        /// Byte offset of the offending byte.
        position: usize,
    },
    /// A port address needs `.<port>` after the instance.
    MissingPort,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::MissingLeadingSlash => f.write_str("address must start with '/'"),
            AddressError::EmptyName { position } => write!(f, "empty name at byte {position}"),
            AddressError::InvalidStart { position } => {
                write!(f, "name must start with a-z (byte {position})")
            }
            AddressError::InvalidCharacter { position } => {
                write!(
                    f,
                    "invalid character at byte {position}; names use a-z, 0-9, _"
                )
            }
            AddressError::MissingPort => f.write_str("port address needs '.<port>'"),
        }
    }
}

impl std::error::Error for AddressError {}

fn check_instance(text: &str) -> Result<(), AddressError> {
    let Some(rest) = text.strip_prefix('/') else {
        return Err(AddressError::MissingLeadingSlash);
    };
    let mut offset = 1;
    for segment in rest.split('/') {
        check_name(segment, offset)?;
        offset += segment.len() + 1;
    }
    Ok(())
}

/// Checks `[a-z][a-z0-9_]*`; `offset` is the name's position for error reports.
fn check_name(name: &str, offset: usize) -> Result<(), AddressError> {
    let mut bytes = name.bytes();
    match bytes.next() {
        None => return Err(AddressError::EmptyName { position: offset }),
        Some(b'a'..=b'z') => {}
        Some(_) => return Err(AddressError::InvalidStart { position: offset }),
    }
    match bytes.position(|b| !matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_')) {
        Some(i) => Err(AddressError::InvalidCharacter {
            position: offset + 1 + i,
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(s: &str) -> InstanceAddress {
        s.parse().unwrap()
    }

    #[test]
    fn parses_canonical_instances() {
        let a = inst("/drums/kick/amp_env");
        assert_eq!(a.to_string(), "/drums/kick/amp_env");
        assert_eq!(
            a.segments().collect::<Vec<_>>(),
            ["drums", "kick", "amp_env"]
        );
        assert_eq!(inst("/a").child("b2_c").unwrap(), inst("/a/b2_c"));
    }

    #[test]
    fn rejects_non_canonical_instances() {
        let err = |s: &str| s.parse::<InstanceAddress>().unwrap_err();
        assert_eq!(err(""), AddressError::MissingLeadingSlash);
        assert_eq!(err("drums"), AddressError::MissingLeadingSlash);
        assert_eq!(err("/"), AddressError::EmptyName { position: 1 });
        assert_eq!(err("/a//b"), AddressError::EmptyName { position: 3 });
        assert_eq!(err("/a/"), AddressError::EmptyName { position: 3 });
        assert_eq!(err("/a/1b"), AddressError::InvalidStart { position: 3 });
        assert_eq!(err("/a/_b"), AddressError::InvalidStart { position: 3 });
        assert_eq!(err("/a/bC"), AddressError::InvalidCharacter { position: 4 });
        assert_eq!(err("/a.b"), AddressError::InvalidCharacter { position: 2 });
        assert_eq!(err("/é"), AddressError::InvalidStart { position: 1 });
        assert_eq!(
            inst("/a").child("B"),
            Err(AddressError::InvalidStart { position: 3 })
        );
    }

    #[test]
    fn parses_ports() {
        let p: PortAddress = "/drums/kick/amp_env.out".parse().unwrap();
        assert_eq!(p.instance(), inst("/drums/kick/amp_env"));
        assert_eq!(p.port(), "out");
        assert_eq!(p.to_string(), "/drums/kick/amp_env.out");
        assert_eq!(
            PortAddress::new(&inst("/drums/kick/amp_env"), "out").unwrap(),
            p
        );
    }

    #[test]
    fn rejects_non_canonical_ports() {
        let err = |s: &str| s.parse::<PortAddress>().unwrap_err();
        assert_eq!(err("/a"), AddressError::MissingPort);
        assert_eq!(err("a.out"), AddressError::MissingLeadingSlash);
        assert_eq!(err("/a."), AddressError::EmptyName { position: 3 });
        assert_eq!(
            err("/a.out.x"),
            AddressError::InvalidCharacter { position: 6 }
        );
        assert_eq!(err("/a/.out"), AddressError::EmptyName { position: 3 });
        assert_eq!(
            PortAddress::new(&inst("/a"), "9"),
            Err(AddressError::InvalidStart { position: 3 })
        );
    }

    #[test]
    fn adr_0016_example_order() {
        let a: PortAddress = "/drums/kick/env.out".parse().unwrap();
        let b: PortAddress = "/lfo.out".parse().unwrap();
        assert!(a < b);
        assert!(inst("/a") < inst("/a/b"));
        assert!(inst("/a/b") < inst("/ab"));
    }
}
