//! Keyboard shortcuts (ADR 0034): the built-in keymap
//! (`ui/lib/keys.default.js`, the single source the pages, the native
//! menu and this check all read) and the person's overrides, `keys.toml`
//! beside the Setup file (a machine's preference, never a project's).
//!
//! Reading never fails: an unknown command, a fixed one, a chord that does
//! not parse or is of the wrong kind is reported with its place in the file
//! (`keys.toml:4:1: …`) and left out; a file that is not TOML (or of another
//! version) is reported and the built-in keys are used (`fallback`). Two
//! commands of one scope on one chord, a scope's chord hiding its parent's
//! where the built-in keymap does not, and a wheel chord holding the wheel
//! modifier are reported as conflicts, located at the override that made
//! them. The rules are `ui/lib/keys.js`'s; both read
//! `ui/lib/fixtures/keys-cases.json` in their tests.
//!
//! Writing ([`write_override`], [`reset`]) edits the file with `toml_edit`,
//! so its comments and the other entries stay as written, and replaces it
//! atomically. An override equal to the built-in keys is removed: the file
//! holds only what differs.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use serde::Deserialize;
use serde_json::{Value, json};

/// The built-in keymap as the UI library serves it: an ES module whose
/// default export is one JSON object ([`defaults_json`]).
pub const DEFAULTS_MODULE: &str = include_str!("../../../ui/lib/keys.default.js");

/// The JSON object of `ui/lib/keys.default.js`: from the first `{` after
/// the line starting `export default` to the last `}`.
pub fn defaults_json(module: &str) -> Option<&str> {
    // At a line's start: the header comment names `export default` too.
    let export = module.find("\nexport default")? + 1;
    let start = export + module.get(export..)?.find('{')?;
    let end = module.rfind('}')?;
    module.get(start..=end)
}

/// The person's overrides: this file beside the Setup file.
pub const FILE_NAME: &str = "keys.toml";

/// The overrides file's format version.
pub const VERSION: i64 = 1;

/// The command whose chords' modifiers take the wheel for a control that
/// does not own it (and so never reach a control's wheel chord).
pub const WHEEL_INSIDE: &str = "view.wheel_inside";

/// A scope: where a command's chords are looked up.
#[derive(Clone, Debug, Deserialize)]
pub struct Scope {
    /// Its id (`view`, `piano_roll`, …).
    pub id: String,
    /// Its name in the Settings page.
    pub label: String,
    /// The scope asked after it, if any.
    #[serde(default)]
    pub parent: Option<String>,
    /// Conventions (text fields, menus): shown, not rebindable.
    #[serde(default)]
    pub fixed: bool,
}

/// A command of the built-in keymap.
#[derive(Clone, Debug, Deserialize)]
pub struct Command {
    /// Its stable id (`view.page_down`).
    pub id: String,
    /// Its scope.
    pub scope: String,
    /// Its name.
    pub label: String,
    /// Its built-in chords.
    pub keys: Vec<String>,
    /// The system's or a convention: not rebindable.
    #[serde(default)]
    pub fixed: bool,
    /// It takes wheel chords (`Wheel`, `Shift+Wheel`, …), not keys.
    #[serde(default)]
    pub wheel: bool,
    /// Shift added to its chord makes it finer.
    #[serde(default)]
    pub fine: bool,
    /// A native menu item: its first key chord is the item's.
    #[serde(default)]
    pub menu: bool,
}

/// The built-in keymap.
#[derive(Debug, Deserialize)]
pub struct Defaults {
    /// The scopes, in order.
    pub scopes: Vec<Scope>,
    /// The commands, in order.
    pub commands: Vec<Command>,
}

impl Defaults {
    /// Command `id`.
    pub fn command(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }

    fn scope(&self, id: &str) -> Option<&Scope> {
        self.scopes.iter().find(|s| s.id == id)
    }

    /// Whether command `id` is fixed (its own flag, or its scope's).
    pub fn fixed(&self, id: &str) -> bool {
        self.command(id)
            .is_some_and(|c| c.fixed || self.scope(&c.scope).is_some_and(|s| s.fixed))
    }
}

/// The built-in keymap, read once. Its text is checked by the tests; an
/// unreadable one (never shipped) is an empty keymap.
pub fn defaults() -> &'static Defaults {
    static DEFAULTS: OnceLock<Defaults> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        defaults_json(DEFAULTS_MODULE)
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or(Defaults {
                scopes: Vec::new(),
                commands: Vec::new(),
            })
    })
}

/// Where the overrides live: `keys.toml` beside the Setup file.
pub fn keys_path(setup_path: &Path) -> PathBuf {
    setup_path.with_file_name(FILE_NAME)
}

/// The modifiers, in the order a chord is written (⌃⌥⇧⌘).
pub const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Cmd"];

fn modifier(token: &str) -> Option<&'static str> {
    Some(match token.to_lowercase().as_str() {
        "ctrl" | "control" | "⌃" => "Ctrl",
        "alt" | "option" | "opt" | "⌥" => "Alt",
        "shift" | "⇧" => "Shift",
        "cmd" | "command" | "meta" | "super" | "⌘" => "Cmd",
        _ => return None,
    })
}

fn named(token: &str) -> Option<String> {
    let lower = token.to_lowercase();
    let name = match lower.as_str() {
        "space" => "Space",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "arrowup" | "up" => "ArrowUp",
        "arrowdown" | "down" => "ArrowDown",
        "arrowleft" | "left" => "ArrowLeft",
        "arrowright" | "right" => "ArrowRight",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "home" => "Home",
        "end" => "End",
        "contextmenu" | "menu" => "ContextMenu",
        "wheel" => "Wheel",
        "plus" => "+",
        "minus" => "-",
        "comma" => ",",
        "period" => ".",
        _ => {
            let n = lower.strip_prefix('f')?.parse::<u8>().ok()?;
            return (1..=20).contains(&n).then(|| format!("F{n}"));
        }
    };
    Some(name.to_owned())
}

fn is_letter(c: char) -> bool {
    c.is_alphabetic() && c.to_uppercase().ne(c.to_lowercase())
}

/// The canonical text of chord `text` (`cmd+shift+n` → `Shift+Cmd+N`), or
/// why it is not one; `ui/lib/keys.js`'s `parseChord`.
pub fn parse_chord(text: &str) -> Result<String, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err("an empty chord".to_owned());
    }
    let mut tokens: Vec<&str> = if raw == "+" {
        vec!["+"]
    } else if let Some(rest) = raw.strip_suffix("++") {
        rest.split('+').chain(std::iter::once("+")).collect()
    } else {
        raw.split('+').collect()
    };
    let key_token = tokens.pop().unwrap_or_default().trim();
    let mut present: Vec<&'static str> = Vec::new();
    for token in tokens {
        let token = token.trim();
        let Some(name) = modifier(token) else {
            return Err(if token.is_empty() {
                format!("'{raw}' has an empty part")
            } else {
                format!("'{token}' is not a modifier (Ctrl, Alt, Shift, Cmd)")
            });
        };
        if present.contains(&name) {
            return Err(format!("{name} twice"));
        }
        present.push(name);
    }
    if key_token.is_empty() {
        return Err(format!("'{raw}' has no key"));
    }
    let key = match named(key_token) {
        Some(key) => key,
        None => {
            let mut chars = key_token.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(format!(
                    "'{key_token}' is not a key (a letter, a digit, a character, or a name such as PageDown)"
                ));
            };
            if modifier(key_token).is_some() {
                return Err(format!("'{raw}' has no key"));
            }
            if c.is_whitespace() {
                return Err("write Space for the space bar".to_owned());
            }
            if is_letter(c) {
                c.to_uppercase().collect()
            } else {
                c.to_string()
            }
        }
    };
    let single = key.chars().count() == 1;
    if present.contains(&"Shift") && single && !key.chars().next().is_some_and(is_letter) {
        return Err(format!(
            "Shift with '{key}': write the character Shift types, without Shift"
        ));
    }
    let mut parts: Vec<String> = MODIFIERS
        .iter()
        .filter(|m| present.contains(m))
        .map(|m| (*m).to_owned())
        .collect();
    parts.push(key);
    Ok(parts.join("+"))
}

/// A chord's modifiers and key.
fn split(chord: &str) -> (Vec<&str>, &str) {
    if chord == "+" {
        return (Vec::new(), "+");
    }
    if let Some(rest) = chord.strip_suffix("++") {
        return (rest.split('+').collect(), "+");
    }
    let mut parts: Vec<&str> = chord.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    (parts, key)
}

/// Why a menu item's chord is refused.
pub const MENU_CHORD: &str = "a menu command needs Cmd, Ctrl or Alt (or a function key)";

/// Whether `chord` may be a menu item's: with Cmd, Ctrl or Alt, or a
/// function key. A bare key (or Shift and a key) would be taken from every
/// text field by the native menu.
pub fn menu_chord(chord: &str) -> bool {
    let (mods, key) = split(chord);
    mods.iter().any(|m| matches!(*m, "Cmd" | "Ctrl" | "Alt"))
        || key
            .strip_prefix('F')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Whether `chord` is a wheel chord.
pub fn is_wheel(chord: &str) -> bool {
    split(chord).1 == "Wheel"
}

/// How the native menu takes a chord: `muda`'s accelerator text
/// (`Shift+Cmd+S` as it is; it parses the same names), or `None` for a
/// wheel chord.
pub fn menu_accelerator(chord: &str) -> Option<String> {
    (!is_wheel(chord)).then(|| chord.to_owned())
}

/// A conflict between chords (`ui/lib/keys.js`'s `buildKeymap`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// `same` (two commands of one scope: `with` keeps it), `shadow` (the
    /// command's scope hides `with` in its parent `with_scope`) or `wheel`
    /// (a wheel chord holding the wheel modifier).
    pub kind: &'static str,
    /// The chord.
    pub chord: String,
    /// The scope where it is.
    pub scope: String,
    /// The command that does not get it (`same`), or hides another.
    pub command: String,
    /// The other command.
    pub with: String,
    /// The other command's scope.
    pub with_scope: String,
}

impl Conflict {
    /// Whether `other` is this conflict: a control's chord hiding its
    /// parents' is the same one whichever parent it hides (ADR 0039: a
    /// focused button's Space hides the transport's, or the view's when an
    /// override puts Space back there), so a built-in shadow stays unreported.
    fn same_as(&self, other: &Conflict) -> bool {
        self.kind == other.kind
            && self.chord == other.chord
            && self.scope == other.scope
            && self.command == other.command
            && (self.kind == "shadow"
                || (self.with == other.with && self.with_scope == other.with_scope))
    }
}

/// The person's keys: the overrides as read, what is wrong with the file,
/// and whether the built-in keys are used for want of a readable file.
#[derive(Clone, Debug, PartialEq)]
pub struct Keys {
    /// The file.
    pub path: PathBuf,
    /// The overrides used, by command id: canonical chords.
    pub overrides: BTreeMap<String, Vec<String>>,
    /// What the file says that is not used, each located.
    pub problems: Vec<String>,
    /// The file does not read: the built-in keys are used.
    pub fallback: bool,
    /// Where each override is (`line:column`), for the conflicts' messages.
    places: BTreeMap<String, String>,
}

/// `line:column` (from 1) of byte `offset` in `text`.
fn place(text: &str, offset: usize) -> String {
    let before = text.get(..offset.min(text.len())).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    format!("{line}:{column}")
}

impl Keys {
    /// No file: the built-in keys.
    pub fn built_in(path: &Path) -> Keys {
        Keys {
            path: path.to_owned(),
            overrides: BTreeMap::new(),
            problems: Vec::new(),
            fallback: false,
            places: BTreeMap::new(),
        }
    }

    /// Reads `path`: a missing file is the built-in keys.
    pub fn load(path: &Path) -> Keys {
        match fs::read_to_string(path) {
            Ok(text) => Keys::parse(&text, path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Keys::built_in(path),
            Err(error) => {
                let mut keys = Keys::built_in(path);
                keys.problems.push(format!(
                    "{}: {error}; using the built-in keys",
                    path.display()
                ));
                keys.fallback = true;
                keys
            }
        }
    }

    /// [`load`](Keys::load) of the text of `path`.
    pub fn parse(text: &str, path: &Path) -> Keys {
        let file = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut keys = Keys::built_in(path);
        let document = match toml_edit::Document::parse(text) {
            Ok(document) => document,
            Err(error) => {
                let at = error
                    .span()
                    .map_or_else(|| "1:1".to_owned(), |s| place(text, s.start));
                let reason = error.message().trim().to_owned();
                keys.problems.push(format!(
                    "{file}:{at}: not valid TOML ({reason}); using the built-in keys"
                ));
                keys.fallback = true;
                return keys;
            }
        };
        let at = |span: Option<std::ops::Range<usize>>| {
            span.map_or_else(|| "1:1".to_owned(), |s| place(text, s.start))
        };
        let root = document.as_table();
        if let Some(version) = root.get("version")
            && version.as_integer() != Some(VERSION)
        {
            keys.problems.push(format!(
                "{file}:{}: version {} is not {VERSION}; using the built-in keys",
                at(version.span()),
                version.to_string().trim()
            ));
            keys.fallback = true;
            return keys;
        }
        let defaults = defaults();
        let mut entries: Vec<(String, &toml_edit::Item, String)> = Vec::new();
        for (name, item) in root.iter() {
            let where_ = at(root
                .key(name)
                .and_then(toml_edit::Key::span)
                .or_else(|| item.span()));
            if name == "version" {
                continue;
            }
            if name.contains('.') {
                entries.push((name.to_owned(), item, where_));
                continue;
            }
            match item.as_table_like() {
                Some(table) => {
                    for (key, value) in table.iter() {
                        let place = at(table
                            .key(key)
                            .and_then(toml_edit::Key::span)
                            .or_else(|| value.span()));
                        entries.push((format!("{name}.{key}"), value, place));
                    }
                }
                None => keys.problems.push(format!(
                    "{file}:{where_}: '{name}' is not a group of commands ([{name}] with name = [\"chord\", …]); ignored"
                )),
            }
        }
        for (id, item, where_) in entries {
            let mut bad = |reason: String| keys.problems.push(format!("{file}:{where_}: {reason}"));
            let Some(command) = defaults.command(&id) else {
                bad(format!("unknown command '{id}'; ignored"));
                continue;
            };
            if defaults.fixed(&id) {
                let why = if defaults.scope(&command.scope).is_some_and(|s| s.fixed) {
                    "a convention"
                } else {
                    "the system's"
                };
                bad(format!("{id} is fixed ({why}); ignored"));
                continue;
            }
            let Some(array) = item.as_array() else {
                bad(format!(
                    "{id}: must be a list of chords ([\"Cmd+S\"], or [] for none); ignored"
                ));
                continue;
            };
            let mut chords: Vec<String> = Vec::new();
            for value in array.iter() {
                let Some(text) = value.as_str() else {
                    bad(format!(
                        "{id}: {} is not a chord; ignored",
                        value.to_string().trim()
                    ));
                    continue;
                };
                match parse_chord(text) {
                    Err(reason) => bad(format!("{id}: '{text}': {reason}; ignored")),
                    Ok(chord) if is_wheel(&chord) != command.wheel => bad(format!(
                        "{id}: '{text}': {}; ignored",
                        if command.wheel {
                            "takes a wheel chord (Wheel, Shift+Wheel, …)"
                        } else {
                            "takes a key, not the wheel"
                        }
                    )),
                    Ok(chord) if id == WHEEL_INSIDE && split(&chord).0.is_empty() => bad(format!(
                        "{id}: '{text}': needs a modifier (Alt+Wheel); ignored"
                    )),
                    Ok(chord) if command.menu && !menu_chord(&chord) => {
                        bad(format!("{id}: '{text}': {MENU_CHORD}; ignored"))
                    }
                    Ok(chord) => {
                        if !chords.contains(&chord) {
                            chords.push(chord);
                        }
                    }
                }
            }
            keys.places.insert(id.clone(), where_);
            keys.overrides.insert(id, chords);
        }
        keys
    }

    /// What the file says that is not used ([`problems`](Keys::problems)),
    /// then each conflict, located at the override that made it.
    pub fn report(&self) -> Vec<String> {
        let file = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut report = self.problems.clone();
        for conflict in self.conflicts() {
            let at = self
                .places
                .get(&conflict.command)
                .or_else(|| self.places.get(&conflict.with))
                .map_or("1:1", String::as_str);
            report.push(format!("{file}:{at}: {}", describe(&conflict)));
        }
        report
    }

    /// The chords of command `id` in force: its override, else its built-in ones.
    pub fn keys_of(&self, id: &str) -> Vec<String> {
        if let Some(keys) = self.overrides.get(id) {
            return keys.clone();
        }
        defaults()
            .command(id)
            .map(|c| c.keys.iter().filter_map(|k| parse_chord(k).ok()).collect())
            .unwrap_or_default()
    }

    /// The conflicts the overrides make (none without overrides).
    pub fn conflicts(&self) -> Vec<Conflict> {
        if self.overrides.is_empty() {
            return Vec::new();
        }
        let base = bind(&|id: &str| {
            defaults()
                .command(id)
                .map(|c| c.keys.iter().filter_map(|k| parse_chord(k).ok()).collect())
                .unwrap_or_default()
        });
        let now = bind(&|id: &str| self.keys_of(id));
        now.into_iter()
            .filter(|c| !base.iter().any(|b| b.same_as(c)))
            .collect()
    }

    /// The menu items' chords: each menu command and its first key chord.
    pub fn menu_chords(&self) -> Vec<(String, Option<String>)> {
        defaults()
            .commands
            .iter()
            .filter(|c| c.menu)
            .map(|c| {
                let first = self.keys_of(&c.id).into_iter().find(|k| !is_wheel(k));
                (c.id.clone(), first.and_then(|k| menu_accelerator(&k)))
            })
            .collect()
    }

    /// The `keys` message (docs/ui-protocol.md "Keyboard shortcuts").
    pub fn message(&self) -> Value {
        let conflicts: Vec<Value> = self
            .conflicts()
            .iter()
            .map(|c| {
                json!({
                    "kind": c.kind, "chord": c.chord, "scope": c.scope, "command": c.command,
                    "with": c.with, "withScope": c.with_scope, "message": describe(c),
                })
            })
            .collect();
        json!({
            "type": "keys",
            "path": self.path.display().to_string(),
            "overrides": self.overrides,
            "problems": self.report(),
            "conflicts": conflicts,
            "fallback": self.fallback,
        })
    }
}

/// A conflict as a sentence.
pub fn describe(c: &Conflict) -> String {
    let defaults = defaults();
    let label = |id: &str| {
        defaults
            .command(id)
            .map_or(id.to_owned(), |c| c.label.clone())
    };
    let scope = |id: &str| {
        defaults
            .scope(id)
            .map_or(id.to_owned(), |s| s.label.clone())
    };
    match c.kind {
        "same" => format!(
            "{} is both {} ▸ {} and {}: {} keeps it",
            c.chord,
            scope(&c.scope),
            label(&c.with),
            label(&c.command),
            label(&c.with)
        ),
        "shadow" => format!(
            "{} in {} ▸ {} hides {} ▸ {} there",
            c.chord,
            scope(&c.scope),
            label(&c.command),
            scope(&c.with_scope),
            label(&c.with)
        ),
        _ => format!(
            "{} ({} ▸ {}) holds the wheel modifier, which only takes the wheel",
            c.chord,
            scope(&c.scope),
            label(&c.command)
        ),
    }
}

/// Every scope's chords and the conflicts, with `keys_of` each command's
/// chords; in the order `ui/lib/keys.js` finds them.
fn bind(keys_of: &dyn Fn(&str) -> Vec<String>) -> Vec<Conflict> {
    let defaults = defaults();
    let mut tables: Vec<(String, Vec<(String, String)>)> = defaults
        .scopes
        .iter()
        .map(|s| (s.id.clone(), Vec::new()))
        .collect();
    let mut conflicts = Vec::new();
    let mut ordered: Vec<&Command> = defaults.commands.iter().collect();
    ordered.sort_by_key(|c| !defaults.fixed(&c.id));
    for command in ordered {
        let Some((_, table)) = tables.iter_mut().find(|(s, _)| *s == command.scope) else {
            continue;
        };
        for chord in keys_of(&command.id) {
            match table.iter().find(|(k, _)| *k == chord) {
                Some((_, holder)) if *holder != command.id => conflicts.push(Conflict {
                    kind: "same",
                    chord,
                    scope: command.scope.clone(),
                    command: command.id.clone(),
                    with: holder.clone(),
                    with_scope: command.scope.clone(),
                }),
                Some(_) => {}
                None => table.push((chord, command.id.clone())),
            }
        }
    }
    let parent_of = |id: &str| defaults.scope(id).and_then(|s| s.parent.clone());
    for (scope, table) in &tables {
        for (chord, id) in table {
            let mut seen: Vec<String> = Vec::new();
            let mut at = parent_of(scope);
            while let Some(parent) = at {
                if seen.contains(&parent) {
                    break;
                }
                let hidden = tables
                    .iter()
                    .find(|(s, _)| *s == parent)
                    .and_then(|(_, t)| t.iter().find(|(k, _)| k == chord))
                    .map(|(_, c)| c.clone());
                if let Some(hidden) = hidden {
                    if hidden != WHEEL_INSIDE {
                        conflicts.push(Conflict {
                            kind: "shadow",
                            chord: chord.clone(),
                            scope: scope.clone(),
                            command: id.clone(),
                            with: hidden,
                            with_scope: parent.clone(),
                        });
                    }
                    break;
                }
                at = parent_of(&parent);
                seen.push(parent);
            }
        }
    }
    let owners: Vec<Vec<String>> = keys_of(WHEEL_INSIDE)
        .iter()
        .map(|k| {
            split(k)
                .0
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|mods| !mods.is_empty())
        .collect();
    for command in &defaults.commands {
        if !command.wheel || command.id == WHEEL_INSIDE {
            continue;
        }
        for chord in keys_of(&command.id) {
            let (mods, _) = split(&chord);
            if owners
                .iter()
                .any(|owner| owner.iter().all(|o| mods.contains(&o.as_str())))
            {
                conflicts.push(Conflict {
                    kind: "wheel",
                    chord,
                    scope: command.scope.clone(),
                    command: command.id.clone(),
                    with: WHEEL_INSIDE.to_owned(),
                    with_scope: "view".to_owned(),
                });
            }
        }
    }
    conflicts
}

/// The file's stamp (modified time and length), for the watch; `None` without one.
pub fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

const HEADER: &str = "# Signal's keyboard shortcuts: overrides of the built-in keymap (ADR 0034,\n# ui/lib/keys.default.js). One group per command id's first part:\n#   [view]\n#   page_down = [\"PageDown\", \"F20\"]   # [] unbinds it\n# Signal.app ▸ Settings… edits this file; an edit here applies at once.\nversion = 1\n";

/// Checks `chords` for command `id` as the file would take them: the
/// canonical chords, or why not.
fn checked(id: &str, chords: &[String]) -> Result<Vec<String>, String> {
    let defaults = defaults();
    let Some(command) = defaults.command(id) else {
        return Err(format!("unknown command '{id}'"));
    };
    if defaults.fixed(id) {
        return Err(format!("{id} is fixed: it cannot be changed"));
    }
    let mut out: Vec<String> = Vec::new();
    for text in chords {
        let chord = parse_chord(text).map_err(|reason| format!("'{text}': {reason}"))?;
        if is_wheel(&chord) != command.wheel {
            return Err(format!(
                "'{text}': {id} {}",
                if command.wheel {
                    "takes a wheel chord (Wheel, Shift+Wheel, …)"
                } else {
                    "takes a key, not the wheel"
                }
            ));
        }
        if id == WHEEL_INSIDE && split(&chord).0.is_empty() {
            return Err(format!("'{text}': needs a modifier (Alt+Wheel)"));
        }
        if command.menu && !menu_chord(&chord) {
            return Err(format!("'{text}': {MENU_CHORD}"));
        }
        if !out.contains(&chord) {
            out.push(chord);
        }
    }
    Ok(out)
}

/// The file's text with command `id` given `chords` (`None`: back to the
/// built-in ones; chords equal to them also remove the override), its
/// comments and other entries kept.
pub fn with_override(text: &str, id: &str, chords: Option<&[String]>) -> Result<String, String> {
    let chords = match chords {
        Some(chords) => Some(checked(id, chords)?),
        None => {
            if defaults().command(id).is_none() {
                return Err(format!("unknown command '{id}'"));
            }
            None
        }
    };
    let built_in: Vec<String> = defaults()
        .command(id)
        .map(|c| c.keys.iter().filter_map(|k| parse_chord(k).ok()).collect())
        .unwrap_or_default();
    let chords =
        chords.filter(|c| !(c.len() == built_in.len() && c.iter().all(|k| built_in.contains(k))));
    let source = if text.trim().is_empty() { HEADER } else { text };
    let mut document = editable(source)?;
    let (group, name) = id
        .split_once('.')
        .ok_or_else(|| format!("unknown command '{id}'"))?;
    // A quoted top-level `"view.page_down" = […]` is edited where it is.
    let quoted = document.as_table().contains_key(id);
    if quoted {
        match &chords {
            Some(chords) => {
                document.insert(id, toml_edit::value(array(chords)));
            }
            None => {
                document.as_table_mut().remove(id);
            }
        }
    }
    let table_like = document
        .as_table_mut()
        .get_mut(group)
        .and_then(toml_edit::Item::as_table_like_mut);
    match (table_like, &chords) {
        // The group's entry goes too, so the quoted one is the only one.
        (Some(table), _) if quoted => {
            table.remove(name);
        }
        (Some(table), Some(chords)) => {
            table.insert(name, toml_edit::value(array(chords)));
        }
        (Some(table), None) => {
            table.remove(name);
        }
        (None, Some(chords)) if !quoted => {
            let mut table = toml_edit::Table::new();
            table.insert(name, toml_edit::value(array(chords)));
            document.insert(group, toml_edit::Item::Table(table));
        }
        (None, _) => {}
    }
    // A group left empty (and without its own comments) goes.
    let empty = document
        .get(group)
        .and_then(toml_edit::Item::as_table)
        .is_some_and(|t| {
            t.is_empty()
                && t.decor()
                    .prefix()
                    .and_then(|p| p.as_str())
                    .is_none_or(|p| p.trim().is_empty())
        });
    if empty {
        document.as_table_mut().remove(group);
    }
    if !document.as_table().contains_key("version") {
        document
            .as_table_mut()
            .insert("version", toml_edit::value(VERSION));
    }
    Ok(document.to_string())
}

/// The file's text to edit, refused when the server would not read it (not
/// TOML, or another `version`): what it says is never overwritten unread.
fn editable(text: &str) -> Result<toml_edit::DocumentMut, String> {
    let document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            format!(
                "{FILE_NAME} does not read ({}): fix or remove it first",
                error.message().trim()
            )
        })?;
    if let Some(version) = document.get("version")
        && version.as_integer() != Some(VERSION)
    {
        return Err(format!(
            "{FILE_NAME} is version {}, not {VERSION}: fix or remove it first",
            version.to_string().trim()
        ));
    }
    Ok(document)
}

fn array(chords: &[String]) -> toml_edit::Array {
    chords.iter().map(String::as_str).collect()
}

/// The file's text without overrides (every command back to the built-in
/// keys), its comments outside the groups kept.
pub fn without_overrides(text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Ok(HEADER.to_owned());
    }
    let mut document = editable(text)?;
    let names: Vec<String> = document
        .as_table()
        .iter()
        .map(|(k, _)| k.to_owned())
        .filter(|k| k != "version")
        .collect();
    for name in names {
        document.as_table_mut().remove(&name);
    }
    if !document.as_table().contains_key("version") {
        document
            .as_table_mut()
            .insert("version", toml_edit::value(VERSION));
    }
    Ok(document.to_string())
}

/// Writes `text` to `path` atomically: a temporary file beside it (beside
/// the file a link names, for a link), synced, then renamed over it.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    // A linked file (a keys.toml kept elsewhere, in dotfiles) is written
    // where it is: the link stays a link.
    let resolved = fs::canonicalize(path).ok();
    let path = resolved.as_deref().unwrap_or(path);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

fn read_or_empty(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Gives command `id` `chords` in the file at `path` (`None`: back to the
/// built-in ones), atomically, keeping what else it says.
pub fn write_override(path: &Path, id: &str, chords: Option<&[String]>) -> Result<(), String> {
    let text = read_or_empty(path)?;
    let next = with_override(&text, id, chords)?;
    write_atomic(path, &next).map_err(|error| format!("{}: {error}", path.display()))
}

/// Every command back to the built-in keys in the file at `path`.
pub fn reset(path: &Path) -> Result<(), String> {
    let text = read_or_empty(path)?;
    let next = without_overrides(&text)?;
    write_atomic(path, &next).map_err(|error| format!("{}: {error}", path.display()))
}
