//! The built-in library (ADR 0027 §4): what a `lib.<name>` module reference
//! resolves to. The composites' text is `signal-lib`'s; they are read by the
//! same loader rules as a project's composites (`docs/project-format.md` §3,
//! §4), with the library's own: a library composite uses `std.*` and `lib.*`
//! modules only, takes no `gpu` pin or `pattern`, and declares its category.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use signal_core::LIBRARY_PREFIX;
use signal_graph::CompositeDef;

use crate::SourceFile;
use crate::model::CompositeFile;

/// The pseudo-folder library composites are read from, as their diagnostics
/// name them (`signal-lib:lib/<name>.toml`): the file in the `signal-lib`
/// crate, never a path in the project folder.
pub const LIBRARY_FOLDER: &str = "signal-lib:lib";

/// The path a diagnostic names for the library composite `name`.
pub(crate) fn library_path(name: &str) -> String {
    format!("{LIBRARY_FOLDER}/{name}.toml")
}

/// The library composite's name of a [`library_path`], if `path` is one.
pub(crate) fn library_stem(path: &str) -> Option<&str> {
    path.strip_prefix(LIBRARY_FOLDER)?
        .strip_prefix('/')?
        .strip_suffix(".toml")
}

/// Whether a diagnostic's `file` names a library composite's file (it is
/// not relative to the project folder).
pub fn is_library_file(file: &str) -> bool {
    file.starts_with("signal-lib:")
}

/// The library composites a project can reference, by name without the
/// `lib.` prefix: [`Library::builtin`], or any set of files (tests).
#[derive(Clone, Debug, Default)]
pub struct Library {
    files: BTreeMap<String, String>,
}

impl Library {
    /// The built-in library's version: the app's MAJOR.MINOR. Archives
    /// and sessions record it.
    pub const VERSION: &str = signal_lib::VERSION;

    /// Whether the built-in composite `name` (without the prefix) is a
    /// validation reference (`signal_lib::VALIDATION`, M2.10-4): a std type
    /// rebuilt from core primitives to be compared with it, which the
    /// library demo leaves out.
    pub fn is_validation(name: &str) -> bool {
        signal_lib::is_validation(name)
    }

    /// The native type the built-in composite `name` (without the prefix)
    /// is a validation reference for (`signal_lib::reference_for`), shown
    /// as `reference_for` by `signal modules`.
    pub fn reference_for(name: &str) -> Option<&'static str> {
        signal_lib::reference_for(name)
    }

    /// The library shipped with Signal (`signal-lib`).
    pub fn builtin() -> &'static Library {
        static BUILTIN: OnceLock<Library> = OnceLock::new();
        BUILTIN.get_or_init(|| {
            Library::new(
                signal_lib::composites()
                    .iter()
                    .map(|f| (f.name.to_owned(), f.text.to_owned())),
            )
        })
    }

    /// A library of these `(name, text)` files.
    pub fn new(files: impl IntoIterator<Item = (String, String)>) -> Library {
        Library {
            files: files.into_iter().collect(),
        }
    }

    /// The text of the library composite `name` (without the prefix).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files.get(name).map(String::as_str)
    }

    /// Every name, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// The library files `defs` reference, transitively, in name order, as
    /// [`library_path`]s. A reference to no library composite is left to the
    /// graph (an unknown module); a file that does not parse is still
    /// returned, so its own errors are reported.
    pub(crate) fn closure<'d>(
        &self,
        defs: impl Iterator<Item = &'d CompositeDef>,
    ) -> Vec<SourceFile> {
        let mut pending: Vec<String> = defs
            .flat_map(|d| d.instances.iter())
            .filter_map(|i| i.module.strip_prefix(LIBRARY_PREFIX))
            .map(str::to_owned)
            .collect();
        let mut found = BTreeMap::new();
        while let Some(name) = pending.pop() {
            if found.contains_key(&name) {
                continue;
            }
            let Some(text) = self.get(&name) else {
                continue;
            };
            if let Ok(file) = toml::from_str::<CompositeFile>(text) {
                let instances = file.instances.map(|i| i.into_inner()).unwrap_or_default();
                pending.extend(instances.into_values().filter_map(|entry| {
                    let module = entry.into_inner().module.into_inner();
                    module.strip_prefix(LIBRARY_PREFIX).map(str::to_owned)
                }));
            }
            let file = SourceFile {
                path: library_path(&name),
                text: text.to_owned(),
            };
            found.insert(name, file);
        }
        found.into_values().collect()
    }
}

/// A library composite a project references (ADR 0027 §4).
#[derive(Clone, Debug)]
pub struct LibraryComposite {
    /// Its module reference, `lib.<name>`.
    pub name: String,
    /// The FNV-1a 64 of its text, 16 lowercase hex digits.
    pub hash: String,
    /// Its file as read (`signal-lib:lib/<name>.toml`).
    pub source: SourceFile,
    /// Its definition, named `lib.<name>`.
    pub def: CompositeDef,
}
