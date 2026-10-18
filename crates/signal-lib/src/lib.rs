//! The built-in library (ADR 0027 §4): composites shipped with Signal,
//! referenced from a project as `lib.<name>`.
//!
//! Each is an ordinary composite file (ADR 0010: no logic), embedded here from
//! `lib/<name>.toml` and read by the project loader under the same rules as a
//! project's `composites/<name>.toml` (`docs/project-format.md` §3, §4). This
//! crate only holds the text: what a `lib.` reference resolves to, and how it
//! is checked, is `signal-project`'s. Library composites are read-only; `signal
//! vendor` copies one into a project, where it is the project's own text.
//!
//! **Versioning** (`CHANGELOG.md`): the library is the app's MAJOR.MINOR
//! ([`VERSION`]). A change of a composite's output is a new name, or a MINOR
//! change with a changelog entry; names are never reused; a removed composite
//! warns for one MINOR first. `tests/pinned.rs` pins every composite's render
//! hash.

use signal_core::fnv1a64;

/// The module-reference prefix of library composites (`lib.<name>`).
pub const PREFIX: &str = signal_core::LIBRARY_PREFIX;

/// The library's version: the app's MAJOR.MINOR.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);

/// One library composite: its name (without the `lib.` prefix) and its file's
/// text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LibraryFile {
    /// E.g. `tape_echo`, referenced as `lib.tape_echo`.
    pub name: &'static str,
    /// The text of `lib/<name>.toml`.
    pub text: &'static str,
}

impl LibraryFile {
    /// The content hash: FNV-1a 64 of the text, 16 lowercase hex digits, as
    /// a render manifest's `project_file_hashes`.
    pub fn hash(&self) -> String {
        format!("{:016x}", fnv1a64(self.text.as_bytes()))
    }
}

macro_rules! library {
    ($($name:literal),* $(,)?) => {
        [$(LibraryFile { name: $name, text: include_str!(concat!("../lib/", $name, ".toml")) }),*]
    };
}

/// Every library composite, in name order.
static COMPOSITES: [LibraryFile; 20] = library![
    "adsr_core",
    "analog_clap",
    "analog_cymbal",
    "analog_hat",
    "analog_kick",
    "analog_rim",
    "analog_snare",
    "analog_tom",
    "ar_env",
    "crossfade",
    "delay_core",
    "ladder_voice",
    "lag_core",
    "lfo_shapes",
    "onepole_core",
    "osc_core",
    "stereo_width",
    "svf_core",
    "tanh_shaper_core",
    "tape_echo",
];

/// One validation pair (ADR 0027 §4, M2.10-4): a library composite rebuilt
/// from core primitives and the native `std.*` type it is rendered against,
/// with the outputs they share (same names on both). `tests/validation.rs`
/// compares each pair; `docs/benchmarks.md` records their costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationPair {
    /// The library composite, without the `lib.` prefix.
    pub composite: &'static str,
    /// The native reference type.
    pub native: &'static str,
    /// The outputs compared.
    pub outputs: &'static [&'static str],
}

/// The validation references: library composites that exist to be compared
/// with a native type, not to be used in place of it. The library demo
/// leaves them out (`projects/library-demo` uses every other composite).
pub static VALIDATION: [ValidationPair; 7] = [
    ValidationPair {
        composite: "adsr_core",
        native: "std.adsr",
        outputs: &["out"],
    },
    ValidationPair {
        composite: "delay_core",
        native: "std.delay",
        outputs: &["out"],
    },
    ValidationPair {
        composite: "lag_core",
        native: "std.lag",
        outputs: &["out"],
    },
    ValidationPair {
        composite: "onepole_core",
        native: "std.onepole",
        outputs: &["lp", "hp"],
    },
    ValidationPair {
        composite: "osc_core",
        native: "std.osc",
        outputs: &["out"],
    },
    ValidationPair {
        composite: "svf_core",
        native: "std.svf",
        outputs: &["lp", "bp", "hp"],
    },
    ValidationPair {
        composite: "tanh_shaper_core",
        native: "std.tanh_shaper",
        outputs: &["out"],
    },
];

/// Whether `name` (without the `lib.` prefix) is a validation reference.
pub fn is_validation(name: &str) -> bool {
    reference_for(name).is_some()
}

/// The native type the library composite `name` (without the `lib.` prefix)
/// is a validation reference for (`"std.svf"` for `svf_core`), or `None`
/// for a composite meant as a building block. `signal modules` and the
/// palette show it as `reference_for` (`docs/module-contract.md`).
pub fn reference_for(name: &str) -> Option<&'static str> {
    VALIDATION
        .iter()
        .find(|pair| pair.composite == name)
        .map(|pair| pair.native)
}

/// Every library composite, in name order.
pub fn composites() -> &'static [LibraryFile] {
    &COMPOSITES
}

/// The library composite `name` (without the `lib.` prefix), if there is one.
pub fn get(name: &str) -> Option<&'static LibraryFile> {
    COMPOSITES.iter().find(|file| file.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is in name order, and every name is the file's.
    #[test]
    fn composites_are_in_name_order() {
        let names: Vec<&str> = composites().iter().map(|f| f.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
        assert_eq!(get("tape_echo").map(|f| f.name), Some("tape_echo"));
        assert!(get("lib.tape_echo").is_none());
        assert_eq!(get("tape_echo").unwrap().hash().len(), 16);
    }

    /// Every validation pair names a library composite, once.
    #[test]
    fn validation_pairs_are_library_composites() {
        for (i, pair) in VALIDATION.iter().enumerate() {
            assert!(get(pair.composite).is_some(), "{}", pair.composite);
            assert!(pair.native.starts_with("std."), "{}", pair.native);
            assert!(
                !VALIDATION[..i]
                    .iter()
                    .any(|p| p.composite == pair.composite)
            );
        }
        assert!(is_validation("svf_core"));
        assert!(!is_validation("tape_echo"));
        assert_eq!(reference_for("svf_core"), Some("std.svf"));
        assert_eq!(reference_for("tape_echo"), None);
    }

    /// The version is MAJOR.MINOR of the workspace's version.
    #[test]
    fn the_version_is_major_minor() {
        let full = env!("CARGO_PKG_VERSION");
        assert!(
            full.starts_with(&format!("{VERSION}.")),
            "{full} vs {VERSION}"
        );
        assert_eq!(VERSION.matches('.').count(), 1);
    }
}
