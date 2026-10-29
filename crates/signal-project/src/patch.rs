//! Patching from the schematic (ADR 0026 §6, M2.9-3, `docs/ui-protocol.md`
//! "patch"): the five ops written into a level's file (`project.toml` or
//! `composites/<name>.toml`) and positions into `ui/schematic.toml`, as text.
//! Control-thread code: it works on text, the caller reads and writes files.
//!
//! Every change is a list of edits, each anchored to an item edge (an entry,
//! a table header, the connections array's `[`, the file start) plus a byte
//! offset into the gap after it. An entry is an instance (by name) or a
//! connection (by index). `commit` rewrites the inside of instance entries
//! only, never a gap, so an anchor survives a commit between a patch and its
//! undo. Applying an edit returns its exact inverse, and a patch's inverse is
//! the reversed list of them: an undo restores the bytes the forward op
//! changed, whatever they were (a comma, a created `[wiring]`). The files are
//! located with `toml_edit`'s parsed spans and spliced as whole lines, so a
//! removed line takes its own trailing comment and never a neighbour's.
//!
//! An op that carries edits (an inverse the page sends back) is untrusted:
//! the edits must pass the gap checks, and the change they make must be
//! exactly what a forward op makes, in one direction or the other, compared
//! as whole texts byte for byte.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml_edit::{Document, DocumentMut, InlineTable, Item as TomlItem, Table, Value};

use crate::schematic::{read_positions, set_position};
use crate::slot::{ContentNames, Interface};
use crate::write::{inline, number, standard};

mod group;
mod inside;
mod slot;

pub use group::{part_key, part_path};
pub use inside::FileChange;
pub(crate) use slot::unloaded_text;
pub use slot::{Change, PortJson};

/// One patch op (`docs/ui-protocol.md` "patch"). `edits` and `layout` are
/// inverse-only: the server writes them into `inverse`, the page sends them
/// back unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    /// An instance, appended to `[instances]`, at `x`, `y` in the schematic.
    #[serde(rename = "instance.add")]
    InstanceAdd {
        /// Its name at the level.
        name: String,
        /// Its module: a registry type or a project composite.
        module: String,
        /// Parameter values.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        params: BTreeMap<String, f64>,
        /// Its schematic position.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        /// Its schematic position.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f64>,
        /// Inverse-only: the level file's edits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<Vec<Edit>>,
        /// Inverse-only: `ui/schematic.toml`'s edits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<Layout>,
    },
    /// An instance, with its connections at the level and its position.
    #[serde(rename = "instance.remove")]
    InstanceRemove {
        /// Its name at the level.
        name: String,
        /// Inverse-only: the level file's edits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<Vec<Edit>>,
        /// Inverse-only: `ui/schematic.toml`'s edits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<Layout>,
    },
    /// A connection, appended to `connections`.
    #[serde(rename = "connection.add")]
    ConnectionAdd {
        /// `instance.port`.
        from: String,
        /// `instance.port`.
        to: String,
        /// Its factor; 1 when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        /// Inverse-only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<Vec<Edit>>,
    },
    /// The first connection `from` → `to` in source order.
    #[serde(rename = "connection.remove")]
    ConnectionRemove {
        /// `instance.port`.
        from: String,
        /// `instance.port`.
        to: String,
        /// Inverse-only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<Vec<Edit>>,
    },
    /// The first connection `from` → `to`'s factor; 1 removes the key.
    #[serde(rename = "connection.scale")]
    ConnectionScale {
        /// `instance.port`.
        from: String,
        /// `instance.port`.
        to: String,
        /// The factor.
        scale: f64,
        /// Inverse-only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<Vec<Edit>>,
    },
    /// ADR 0037: a slot, appended to `[instances]` as `instance.add` appends
    /// an instance: `name = { slot = <slot>, load = "<load>" }`.
    #[serde(rename = "slot.add")]
    SlotAdd {
        /// Its name at the level.
        name: String,
        /// Its interface: a template's name (`"lib.voice"`, `"deck"`) or a
        /// table (`{"inputs": {"in": "audio"}}`); absent: `{}`.
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        slot: serde_json::Value,
        /// Its content, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        load: Option<String>,
        /// Its schematic position.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        /// Its schematic position.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f64>,
    },
    /// ADR 0037 §6–7: `load` written (a Load, or a Replace of the content
    /// loaded): `map` and `set` replace the slot's when given, else the
    /// entries the new content accepts are kept and the rest removed.
    #[serde(rename = "slot.load")]
    SlotLoad {
        /// The slot's name at the level.
        slot: String,
        /// The content: a module reference or a project path.
        load: String,
        /// The slot's new `map` (`{}` removes it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map: Option<BTreeMap<String, serde_json::Value>>,
        /// The slot's new `set` (`{}` removes it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<BTreeMap<String, f64>>,
    },
    /// ADR 0037 §6: `load` removed (and `seed`, a project's): the slot empty.
    #[serde(rename = "slot.unload")]
    SlotUnload {
        /// The slot's name at the level.
        slot: String,
    },
    /// ADR 0037 §4: the slot's `map` written (`{}` removes it).
    #[serde(rename = "slot.map")]
    SlotMap {
        /// The slot's name at the level.
        slot: String,
        /// Slot port or parameter → the content's name, or `false`.
        map: BTreeMap<String, serde_json::Value>,
    },
    /// ADR 0037 §2: the slot's interface edited.
    #[serde(rename = "slot.interface")]
    SlotInterface {
        /// The slot's name at the level.
        slot: String,
        /// Ports and parameters added.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<PortJson>,
        /// Removed (a template's: named in `remove`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
        /// Renamed, old → new, with the level's wires and exports on them.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        rename: BTreeMap<String, String>,
        /// Declared anew (shape, rate, kind, unit, range, group).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        retype: Vec<PortJson>,
        /// Remove the wires on removed ports too (else refused, `in-use`).
        #[serde(
            default,
            rename = "removeWires",
            skip_serializing_if = "std::ops::Not::not"
        )]
        remove_wires: bool,
        /// ADR 0046 §7: remove the controls of the slot's part on removed
        /// ports too (else refused, `in-use`). The host's: the part is a
        /// panel file.
        #[serde(
            default,
            rename = "removeControls",
            skip_serializing_if = "std::ops::Not::not"
        )]
        remove_controls: bool,
        /// "Detach from template": the resolved interface written inline.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        detach: bool,
        /// The interface set to this template alone (`slot = "<template>"`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
    },
    /// The inverse of a slot op: keys of the slot's entry set back to what
    /// they said, and the level's wires on its ports renamed back.
    #[serde(rename = "slot.restore")]
    SlotRestore {
        /// The slot's name at the level.
        slot: String,
        /// The keys, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes: Vec<Change>,
        /// Port → port, the wires and exports renamed.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        wires: BTreeMap<String, String>,
    },
    /// ADR 0037, editable insides: the slot's own inside made, a project
    /// composite `composites/<name>.toml` copied from its content (empty,
    /// a project or library composite, a primitive wrapped) with
    /// `inside_of` naming the slot and its exports named after the slot's
    /// ports; the slot then loads it (its `map` removed).
    #[serde(rename = "slot.inside")]
    SlotInside {
        /// The slot's name at the level.
        slot: String,
        /// The new composite's name (absent: `<slot>_inside`, or
        /// `<level>_<slot>_inside`, then `_2`, `_3`, … while taken).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A slot's own inside's file (`composites/<name>.toml`, whose text
    /// has `inside_of`) set to `text` (absent: removed) while it holds
    /// `was` (absent: it does not exist): the inverse of `slot.inside` and
    /// of an interface edit that keeps the inside's exports in step.
    #[serde(rename = "composite.file")]
    CompositeFile {
        /// The composite's name.
        name: String,
        /// Its new text; absent removes the file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Its text now; absent: it does not exist.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        was: Option<String>,
    },
    /// ADR 0037, editable insides: an inner port added to the composite
    /// level's export `name` of `role` (a frame pin wired inside): an
    /// input's or a parameter's targets gain `port`, an output is `port`.
    #[serde(rename = "export.connect")]
    ExportConnect {
        /// `input`, `output` or `parameter`.
        role: crate::slot::Role,
        /// The export's name.
        name: String,
        /// `instance.port` at the level.
        port: String,
    },
    /// The inner port `port` taken from the composite level's export
    /// `name` of `role`; the export goes with its last port.
    #[serde(rename = "export.disconnect")]
    ExportDisconnect {
        /// `input`, `output` or `parameter`.
        role: crate::slot::Role,
        /// The export's name.
        name: String,
        /// `instance.port` at the level.
        port: String,
    },
    /// ADR 0046 §8: an instance made a slot holding what it was, with the
    /// same names: `module = "x"` becomes `slot = { … }, load = "x"`
    /// (`project = "…"` becomes `load = "…"`, `seed` and `set` kept), its
    /// interface every port a wire, an export or a host binding at the
    /// level names (auto, `{}`) and every parameter the entry sets, a wire
    /// reaches, an export names or a binding names. The plan does not
    /// change. The host also moves the panel controls on the instance into
    /// the slot's part (`controls`: their indexes among `ui/main.html`'s
    /// controls; absent: all).
    #[serde(rename = "slot.group")]
    SlotGroup {
        /// The instance's name at the level.
        instance: String,
        /// The host's: which of the main panel's controls on the instance
        /// move into the part.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        controls: Option<Vec<usize>>,
    },
    /// ADR 0046 §8: a presentation file under `ui/` (`main.html`,
    /// `windows.toml` or `parts/<key>.html`, named relative to `ui/`) set
    /// to `text` (absent: removed) while it holds `was` (absent: it does
    /// not exist): the inverse of `slot.group`'s panel move and of a
    /// slot's part removed with it.
    #[serde(rename = "panel.restore")]
    PanelRestore {
        /// The file, relative to `ui/`.
        file: String,
        /// Its new text; absent removes the file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Its text now; absent: it does not exist.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        was: Option<String>,
    },
    /// The inverse of an export op: keys of the level's `[exports]` set
    /// back to what they said (`path` from the file's root:
    /// `["exports", "<role table>", "<name>"]`, or `["exports"]`, the
    /// table itself, made or removed empty).
    #[serde(rename = "export.restore")]
    ExportRestore {
        /// The keys, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes: Vec<Change>,
    },
}

/// `ui/schematic.toml`'s edits of an op; `created`: the file is removed when
/// the edits leave it empty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// The edits.
    pub edits: Vec<Edit>,
    /// Remove the file when it is then empty.
    #[serde(default)]
    pub created: bool,
}

/// One text edit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "lowercase")]
pub enum Edit {
    /// Inserts `text` at `at`.
    Insert {
        /// Where.
        at: Pos,
        /// What.
        text: String,
    },
    /// Deletes exactly `text` at `at`.
    Delete {
        /// Where.
        at: Pos,
        /// What.
        text: String,
    },
    /// Removes the entry `item` as it is now, with exactly `before` and
    /// `after` around it.
    Cut {
        /// The entry.
        item: Item,
        /// The bytes just before it.
        before: String,
        /// The bytes just after it.
        after: String,
    },
}

/// A position: the end of `item` plus `offset` bytes into the gap after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pos {
    /// The anchor.
    pub item: Item,
    /// Bytes after its end.
    pub offset: usize,
}

/// An anchor: `instance:<name>` (`instance:<name>#<n>`: piece `n` of an
/// instance written as dotted keys among other entries), `connection:<index>`,
/// `table:<path>`, `array` (after the connections array's `[`) or `file`
/// (its start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// An instance entry (or a position entry in `ui/schematic.toml`), and
    /// which piece of it (0 for one written in one place).
    Instance(String, usize),
    /// A connection entry.
    Connection(usize),
    /// A table's header.
    Table(String),
    /// After the connections array's `[`.
    Array,
    /// The file's start.
    File,
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Instance(name, 0) => write!(f, "instance:{name}"),
            Item::Instance(name, piece) => write!(f, "instance:{name}#{piece}"),
            Item::Connection(k) => write!(f, "connection:{k}"),
            Item::Table(path) => write!(f, "table:{path}"),
            Item::Array => f.write_str("array"),
            Item::File => f.write_str("file"),
        }
    }
}

impl FromStr for Item {
    type Err = String;

    fn from_str(s: &str) -> Result<Item, String> {
        match s.split_once(':') {
            Some(("instance", name)) => match name.rsplit_once('#') {
                Some((name, piece)) => piece
                    .parse()
                    .map(|piece| Item::Instance(name.to_owned(), piece))
                    .map_err(|_| format!("bad item `{s}`")),
                None => Ok(Item::Instance(name.to_owned(), 0)),
            },
            Some(("connection", k)) => k
                .parse()
                .map(Item::Connection)
                .map_err(|_| format!("bad item `{s}`")),
            Some(("table", path)) => Ok(Item::Table(path.to_owned())),
            None if s == "array" => Ok(Item::Array),
            None if s == "file" => Ok(Item::File),
            _ => Err(format!("bad item `{s}`")),
        }
    }
}

impl Serialize for Item {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Item, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Why a patch was refused: a `docs/ui-protocol.md` error code and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchError {
    /// `bad-value`, `unknown-instance`, `unknown-module`, `does-not-load` or
    /// `write-failed`.
    pub code: &'static str,
    /// Why.
    pub message: String,
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

fn refused(code: &'static str, message: impl Into<String>) -> PatchError {
    PatchError {
        code,
        message: message.into(),
    }
}

fn bad(message: impl Into<String>) -> PatchError {
    refused("bad-value", message)
}

/// What a patch is applied with.
pub struct Context<'a> {
    /// The level: `""` the top level, else a composite's name.
    pub level: &'a str,
    /// A module type's parameters in declared order (a composite's exported
    /// ones), `None` for a module the project does not know.
    pub module: &'a dyn Fn(&str) -> Option<Vec<String>>,
    /// ADR 0037: the project's interface templates by name
    /// (`interfaces/<name>.toml`; the library's are known here).
    pub templates: &'a BTreeMap<String, Interface>,
    /// ADR 0037 §7: what a slot's content (`load`, as written) offers,
    /// `None` when it does not resolve: a Replace then keeps every `map` and
    /// `set` entry, and the load check judges them.
    pub content: &'a dyn Fn(&str) -> Option<ContentNames>,
    /// ADR 0037, editable insides: the project's file at `path`
    /// (`composites/<name>.toml`) as it is now, `None` when absent.
    pub file: &'a dyn Fn(&str) -> Option<String>,
    /// What a slot's content (`load`, as written) is copied from
    /// ([`crate::inside::source`]); `None` for a project or an unknown name.
    pub source: &'a dyn Fn(&str) -> Option<crate::inside::Source>,
    /// ADR 0046 §8: the ports and parameters of the level's instance
    /// `name` that the project's panels bind (`/name.<port>` at the top
    /// level, through every instance path of a composite level), by name:
    /// `slot.group` puts them into the interface. The caller's (the panels
    /// are `ui/`'s); empty when it knows none.
    pub bound: &'a dyn Fn(&str) -> Vec<String>,
}

/// A [`Context::bound`] that knows no panel binding.
pub fn no_bindings(_: &str) -> Vec<String> {
    Vec::new()
}

/// A patch applied: the level's new text, `ui/schematic.toml`'s (`None`:
/// absent), whether that changed, and the inverse ops.
#[derive(Debug)]
pub struct Patched {
    /// The level file.
    pub text: String,
    /// `ui/schematic.toml`; `None` when it is absent or removed.
    pub positions: Option<String>,
    /// Whether `ui/schematic.toml` changed.
    pub positions_changed: bool,
    /// The ops that undo the patch, in order.
    pub inverse: Vec<Op>,
    /// What a slot op says it did besides (a Replace's removed `map` and
    /// `set` entries).
    pub notes: Vec<String>,
    /// ADR 0037 §2: the slot ports renamed, `slot.old` → `slot.new` at the
    /// level, in order: the caller rewrites the panel bindings on them.
    pub renamed: Vec<(String, String)>,
    /// ADR 0037 §2, ADR 0041: the slot ports renamed (`slot.old`,
    /// `Some(slot.new)`) and removed (`slot.port`, `None`) at the level, in
    /// the order the ops did them (an op's removals before its renames): the
    /// caller moves or drops their snapshots' values in that order.
    pub follow: Vec<(String, Option<String>)>,
    /// ADR 0037, editable insides: the other files the patch writes (a
    /// slot's own inside made, removed, or its exports kept in step with
    /// the slot's interface), each with its text before and after.
    pub files: Vec<FileChange>,
}

/// Applies `ops` in order to `text` (the level file) and `positions`
/// (`ui/schematic.toml`, `None` when absent). Nothing is written; the whole
/// patch fails together.
pub fn apply(
    text: &str,
    positions: Option<&str>,
    ops: &[Op],
    ctx: &Context<'_>,
) -> Result<Patched, PatchError> {
    let mut patch = Patching {
        text: text.to_owned(),
        positions: positions.map(str::to_owned),
        positions_changed: false,
        ctx,
        notes: Vec::new(),
        renamed: Vec::new(),
        follow: Vec::new(),
        files: BTreeMap::new(),
    };
    map(&patch.text, &Scheme::LEVEL).map_err(|e| refused("does-not-load", e))?;
    let mut inverse = Vec::new();
    for op in ops {
        inverse.extend(patch.op(op)?);
    }
    inverse.reverse();
    Ok(Patched {
        text: patch.text,
        positions: patch.positions,
        positions_changed: patch.positions_changed,
        inverse,
        notes: patch.notes,
        renamed: patch.renamed,
        follow: patch.follow,
        files: inside::changed(patch.files),
    })
}

// The file model: entries and anchors, from toml_edit's spans.

/// Which entries a file holds: a level's instances and connections, or one
/// level's positions in `ui/schematic.toml`.
struct Scheme<'a> {
    entries: [&'a str; 2],
    depth: usize,
    connections: bool,
}

impl Scheme<'static> {
    const LEVEL: Scheme<'static> = Scheme {
        entries: ["instances", ""],
        depth: 1,
        connections: true,
    };
}

fn positions_scheme(level: &str) -> Scheme<'_> {
    if level.is_empty() {
        Scheme {
            entries: ["top", ""],
            depth: 1,
            connections: false,
        }
    } else {
        Scheme {
            entries: ["composite", level],
            depth: 2,
            connections: false,
        }
    }
}

struct Entry {
    name: String,
    /// Which piece of the instance (dotted keys among other entries).
    piece: usize,
    range: Range<usize>,
    section: bool,
}

struct Conn {
    range: Range<usize>,
    from: String,
    to: String,
    scale: Option<f64>,
}

enum EntriesTable {
    Explicit(usize),
    Implicit,
    Inline(Range<usize>),
}

#[derive(Default)]
struct Map {
    entries: Vec<Entry>,
    conns: Vec<Conn>,
    headers: Vec<(String, Range<usize>)>,
    /// After the connections array's `[`, and its `]`.
    array: Option<(usize, usize)>,
    aot: bool,
    wiring_header: Option<usize>,
    table: Option<EntriesTable>,
    settings_end: usize,
}

fn line_start(t: &str, at: usize) -> usize {
    t[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(t: &str, at: usize) -> usize {
    t[at..].find('\n').map_or(t.len(), |i| at + i + 1)
}

fn spans(item: &TomlItem) -> Option<Range<usize>> {
    match item {
        TomlItem::Value(v) => v.span(),
        _ => item.span(),
    }
}

/// The end of a table's own key/value lines (at least its header's end).
fn body_end(table: &Table, header_end: usize) -> usize {
    let mut end = header_end;
    for (key, _) in table.iter() {
        if let Some((_, item)) = table.get_key_value(key)
            && let Some(span) = spans(item)
        {
            end = end.max(span.end);
        }
    }
    end
}

fn map(t: &str, scheme: &Scheme<'_>) -> Result<Map, String> {
    let doc = Document::parse(t).map_err(|e| format!("does not parse: {}", e.message()))?;
    let mut m = Map::default();
    let mut dotted = Vec::new();
    walk(
        t,
        doc.as_table(),
        &mut Vec::new(),
        scheme,
        &mut m,
        &mut dotted,
    )?;
    pieces(&mut m, dotted);
    m.headers.sort_by_key(|(_, r)| r.start);
    m.entries.sort_by_key(|e| e.range.start);
    Ok(m)
}

/// The instances written as dotted keys (`osc.module = …`): each key line
/// is a piece of its own, wherever it is (other entries may lie between
/// them), so a removal and its undo never anchor inside one.
fn pieces(m: &mut Map, dotted: Vec<(String, Vec<Range<usize>>)>) {
    for (name, mut lines) in dotted {
        lines.sort_by_key(|r| r.start);
        for (piece, range) in lines.into_iter().enumerate() {
            m.entries.push(Entry {
                name: name.clone(),
                piece,
                range,
                section: false,
            });
        }
    }
}

fn walk(
    t: &str,
    table: &Table,
    path: &mut Vec<String>,
    scheme: &Scheme<'_>,
    m: &mut Map,
    dotted: &mut Vec<(String, Vec<Range<usize>>)>,
) -> Result<(), String> {
    for (key, item) in table.iter() {
        path.push(key.to_owned());
        let here: Vec<&str> = path.iter().map(String::as_str).collect();
        let entries = here.as_slice() == &scheme.entries[..scheme.depth];
        match item {
            TomlItem::Table(sub) => {
                let header = (!sub.is_implicit() && !sub.is_dotted())
                    .then(|| sub.span())
                    .flatten();
                if let Some(header) = &header {
                    m.headers.push((here.join("."), header.clone()));
                }
                if entries {
                    m.table = Some(match &header {
                        Some(h) => EntriesTable::Explicit(h.end),
                        None => EntriesTable::Implicit,
                    });
                    table_entries(t, sub, m, dotted)?;
                } else {
                    if here == ["project"] || (here == ["composite"] && scheme.connections) {
                        m.settings_end = body_end(sub, header.as_ref().map_or(0, |h| h.end));
                    }
                    if scheme.connections && here == ["wiring"] {
                        m.wiring_header = header.as_ref().map(|h| h.end);
                        connections(sub.get("connections"), m)?;
                    }
                    walk(t, sub, path, scheme, m, dotted)?;
                }
            }
            TomlItem::Value(value) => {
                if let Some(inline_table) = value.as_inline_table() {
                    if entries {
                        let span = value.span().ok_or("an entry has no span")?;
                        m.table = Some(EntriesTable::Inline(span));
                        for (name, _) in inline_table.iter() {
                            let (k, v) = inline_table.get_key_value(name).ok_or("no key")?;
                            let range =
                                k.span().ok_or("no span")?.start..v.span().ok_or("no span")?.end;
                            m.entries.push(Entry {
                                name: name.to_owned(),
                                piece: 0,
                                range,
                                section: false,
                            });
                        }
                    } else if scheme.connections
                        && here == ["wiring"]
                        && let Some(v) = inline_table.get("connections")
                    {
                        array_connections(v, m)?;
                    }
                }
            }
            _ => {}
        }
        path.pop();
    }
    Ok(())
}

fn table_entries(
    t: &str,
    table: &Table,
    m: &mut Map,
    dotted: &mut Vec<(String, Vec<Range<usize>>)>,
) -> Result<(), String> {
    for (name, item) in table.iter() {
        let (range, section) = match item {
            // `[instances.osc]`: from its header to its last value.
            TomlItem::Table(sub) if !sub.is_dotted() => {
                let header = sub.span().ok_or("a section has no span")?;
                (header.start..body_end(sub, header.end), true)
            }
            // `osc.module = …` lines, each from its first key to its value;
            // made pieces once every other entry is known.
            TomlItem::Table(sub) => {
                let mut lines = Vec::new();
                for (key, _) in sub.iter() {
                    let (k, v) = sub.get_key_value(key).ok_or("no key")?;
                    let at = k.span().ok_or("no span")?.start;
                    let line = line_start(t, at);
                    let start = line + (t[line..at].len() - t[line..at].trim_start().len());
                    lines.push(start..spans(v).ok_or("no span")?.end);
                }
                dotted.push((name.to_owned(), lines));
                continue;
            }
            TomlItem::Value(_) => {
                let (k, v) = table.get_key_value(name).ok_or("no key")?;
                (
                    k.span().ok_or("no span")?.start..spans(v).ok_or("no span")?.end,
                    false,
                )
            }
            _ => continue,
        };
        m.entries.push(Entry {
            name: name.to_owned(),
            piece: 0,
            range,
            section,
        });
    }
    Ok(())
}

fn connections(item: Option<&TomlItem>, m: &mut Map) -> Result<(), String> {
    match item {
        Some(TomlItem::ArrayOfTables(tables)) => {
            m.aot = true;
            for table in tables.iter() {
                let header = table.span().ok_or("a section has no span")?;
                let field = |key: &str| {
                    table
                        .get(key)
                        .and_then(TomlItem::as_str)
                        .unwrap_or("")
                        .to_owned()
                };
                let scale = table
                    .get("scale")
                    .and_then(TomlItem::as_value)
                    .and_then(value_number);
                m.conns.push(Conn {
                    range: header.start..body_end(table, header.end),
                    from: field("from"),
                    to: field("to"),
                    scale,
                });
            }
            Ok(())
        }
        Some(TomlItem::Value(value)) => array_connections(value, m),
        _ => Ok(()),
    }
}

fn array_connections(value: &Value, m: &mut Map) -> Result<(), String> {
    let Some(array) = value.as_array() else {
        return Ok(());
    };
    let span = array.span().ok_or("the connections array has no span")?;
    m.array = Some((span.start + 1, span.end - 1));
    for element in array.iter() {
        let range = element.span().ok_or("a connection has no span")?;
        let table = element.as_inline_table();
        let field = |key: &str| {
            table
                .and_then(|t| t.get(key))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let scale = table.and_then(|t| t.get("scale")).and_then(value_number);
        m.conns.push(Conn {
            range,
            from: field("from"),
            to: field("to"),
            scale,
        });
    }
    Ok(())
}

fn value_number(value: &Value) -> Option<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|i| i as f64))
}

fn extent(m: &Map, item: &Item) -> Result<Range<usize>, String> {
    match item {
        Item::Instance(name, piece) => m
            .entries
            .iter()
            .find(|e| &e.name == name && e.piece == *piece)
            .map(|e| e.range.clone())
            .ok_or_else(|| format!("no instance `{name}`")),
        Item::Connection(k) => m
            .conns
            .get(*k)
            .map(|c| c.range.clone())
            .ok_or_else(|| format!("no connection {k}")),
        Item::Table(path) => m
            .headers
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, r)| r.clone())
            .ok_or_else(|| format!("no table `{path}`")),
        Item::Array => m
            .array
            .map(|(open, _)| open..open)
            .ok_or_else(|| "no connections array".to_owned()),
        Item::File => Ok(0..0),
    }
}

fn entry_ranges(m: &Map) -> impl Iterator<Item = (Item, &Range<usize>)> {
    m.entries
        .iter()
        .map(|e| (Item::Instance(e.name.clone(), e.piece), &e.range))
        .chain(
            m.conns
                .iter()
                .enumerate()
                .map(|(k, c)| (Item::Connection(k), &c.range)),
        )
}

/// The nearest item edge at or before `p`, and `p`'s offset from it.
fn anchor(t: &str, m: &Map, p: usize) -> Result<Pos, String> {
    if p > t.len() {
        return Err("a position past the end".into());
    }
    let mut best = (Item::File, 0);
    let mut take = |item: Item, end: usize| {
        if end <= p && end >= best.1 {
            best = (item, end);
        }
    };
    for (path, r) in &m.headers {
        take(Item::Table(path.clone()), r.end);
    }
    if let Some((open, _)) = m.array {
        take(Item::Array, open);
    }
    for (item, r) in entry_ranges(m) {
        if r.start < p && p < r.end {
            return Err("a position inside an entry".into());
        }
        take(item, r.end);
    }
    Ok(Pos {
        item: best.0,
        offset: p - best.1,
    })
}

/// Where `at` is in `t`: inside it and on a character boundary (an
/// untrusted offset is checked, never trusted to index).
fn resolve(t: &str, m: &Map, at: &Pos) -> Result<usize, String> {
    extent(m, &at.item)?
        .end
        .checked_add(at.offset)
        .filter(|p| t.is_char_boundary(*p))
        .ok_or_else(|| "a position outside the text or inside a character".to_owned())
}

/// `t`'s bytes `start..start + len`, when that range is inside `t` on
/// character boundaries.
fn span_of(t: &str, start: usize, len: usize) -> Option<(Range<usize>, &str)> {
    let end = start.checked_add(len)?;
    t.get(start..end).map(|text| (start..end, text))
}

fn touches(m: &Map, r: &Range<usize>, except: Option<&Item>) -> bool {
    entry_ranges(m).any(|(item, e)| Some(&item) != except && e.start < r.end && r.start < e.end)
}

/// Applies `edit`; returns its inverse. `checked`: the edit is untrusted
/// (gap rules, `bad-value`).
fn apply_edit(
    t: &mut String,
    scheme: &Scheme<'_>,
    edit: &Edit,
    checked: bool,
) -> Result<Edit, String> {
    let m = map(t, scheme)?;
    let inverse = match edit {
        Edit::Insert { at, text } => {
            let p = resolve(t, &m, at)?;
            if entry_ranges(&m).any(|(_, r)| r.start < p && p < r.end) {
                return Err("an insert inside an entry".into());
            }
            t.insert_str(p, text);
            let after = map(t, scheme)?;
            let end = p + text.len();
            let inside: Vec<(Item, Range<usize>)> = entry_ranges(&after)
                .filter(|(_, r)| r.start < end && p < r.end || (r.start >= p && r.end <= end))
                .map(|(i, r)| (i, r.clone()))
                .collect();
            match inside.as_slice() {
                [] => Edit::Delete {
                    at: at.clone(),
                    text: text.clone(),
                },
                [(item, r)] if r.start >= p && r.end <= end => Edit::Cut {
                    item: item.clone(),
                    before: t[p..r.start].to_owned(),
                    after: t[r.end..end].to_owned(),
                },
                _ => return Err("an insert holds one entry at most, whole".into()),
            }
        }
        Edit::Delete { at, text } => {
            let p = resolve(t, &m, at)?;
            let r = match span_of(t, p, text.len()) {
                Some((r, there)) if there == text => r,
                _ => return Err("the bytes to delete differ".into()),
            };
            if checked && touches(&m, &r, None) {
                return Err("a delete crosses an entry".into());
            }
            t.replace_range(r, "");
            Edit::Insert {
                at: at.clone(),
                text: text.clone(),
            }
        }
        Edit::Cut {
            item,
            before,
            after,
        } => {
            if !matches!(item, Item::Instance(..) | Item::Connection(_)) {
                return Err("only an entry is cut".into());
            }
            let r = extent(&m, item)?;
            let b = r
                .start
                .checked_sub(before.len())
                .and_then(|s| span_of(t, s, before.len()));
            let a = span_of(t, r.end, after.len());
            let (b, a) = match (b, a) {
                (Some((b, there_b)), Some((a, there_a)))
                    if there_b == before && there_a == after =>
                {
                    (b, a)
                }
                _ => return Err("the bytes around the cut differ".into()),
            };
            if checked && (touches(&m, &b, Some(item)) || touches(&m, &a, Some(item))) {
                return Err("a cut reaches into another entry".into());
            }
            let removed = t[b.start..a.end].to_owned();
            t.replace_range(b.start..a.end, "");
            let at = anchor(t, &map(t, scheme)?, b.start)?;
            Edit::Insert { at, text: removed }
        }
    };
    map(t, scheme)?;
    Ok(inverse)
}

/// Applies untrusted `edits` in order; returns their inverse.
fn run_checked(
    t: &mut String,
    scheme: &Scheme<'_>,
    edits: &[Edit],
) -> Result<Vec<Edit>, PatchError> {
    let mut inverse = Vec::new();
    for edit in edits {
        inverse.push(apply_edit(t, scheme, edit, true).map_err(bad)?);
    }
    inverse.reverse();
    Ok(inverse)
}

// Forward rules: these alone choose formatting, and define what an edit may
// change (the forward-pair rule).

type Edits = Vec<Edit>;

fn insert_at(
    t: &mut String,
    scheme: &Scheme<'_>,
    p: usize,
    text: &str,
    inv: &mut Edits,
) -> Result<(), String> {
    let at = anchor(t, &map(t, scheme)?, p)?;
    inv.push(apply_edit(
        t,
        scheme,
        &Edit::Insert {
            at,
            text: text.to_owned(),
        },
        false,
    )?);
    Ok(())
}

fn leading_block(t: &str, own: usize) -> usize {
    let mut block = own;
    while block > 0 {
        let prev = line_start(t, block - 1);
        if t[prev..block].trim_start().starts_with('#') {
            block = prev;
        } else {
            break;
        }
    }
    block
}

fn owns_line(t: &str, r: &Range<usize>) -> bool {
    let rest = t[r.end..line_end(t, r.end)].trim();
    let rest = rest.strip_prefix(',').unwrap_or(rest).trim();
    t[line_start(t, r.start)..r.start].trim().is_empty()
        && (rest.is_empty() || rest.starts_with('#'))
}

/// The bytes a removal takes around an entry on its own lines: its leading
/// comment block (a section also one blank line above it) and the rest of
/// its last line, its trailing comment included.
fn line_cut(t: &str, r: &Range<usize>, section: bool) -> (String, String) {
    let mut block = leading_block(t, line_start(t, r.start));
    if section && block > 0 {
        let prev = line_start(t, block - 1);
        if t[prev..block].trim().is_empty() {
            block = prev;
        }
    }
    (
        t[block..r.start].to_owned(),
        t[r.end..line_end(t, r.end)].to_owned(),
    )
}

/// The separator a removal takes around an entry sharing its line: to the
/// next entry, else from the previous one, else up to the closing bracket.
/// A comment in the separator stays (a neighbour's line keeps its comment):
/// then only the comma after the entry and the spaces after that are taken,
/// or, before the closing bracket, only what follows the comment's line.
fn separator_cut(t: &str, ranges: &[Range<usize>], k: usize, close: usize) -> (String, String) {
    let r = &ranges[k];
    if let Some(next) = ranges.get(k + 1) {
        let between = &t[r.end..next.start];
        let comma = between.find(',');
        match (between.find('#'), comma) {
            (Some(hash), Some(comma)) if comma < hash => {
                let rest = &between[comma + 1..];
                let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                (String::new(), between[..comma + 1 + spaces].to_owned())
            }
            _ => (String::new(), between.to_owned()),
        }
    } else if k > 0 {
        let between = &t[ranges[k - 1].end..r.start];
        match between.rfind('\n') {
            Some(newline) if between.contains('#') => {
                (between[newline + 1..].to_owned(), String::new())
            }
            _ => (between.to_owned(), String::new()),
        }
    } else {
        let tail = &t[r.end..close.max(r.end)];
        (String::new(), tail.trim_end_matches([' ', '\t']).to_owned())
    }
}

fn cut_instance(
    t: &mut String,
    scheme: &Scheme<'_>,
    name: &str,
    inv: &mut Edits,
) -> Result<(), String> {
    let mut found = false;
    // Every piece of it (dotted keys among other entries), the last first.
    loop {
        let m = map(t, scheme)?;
        let Some(k) = m
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name == name)
            .max_by_key(|(_, e)| e.piece)
            .map(|(k, _)| k)
        else {
            break;
        };
        found = true;
        let entry = &m.entries[k];
        let (before, after) = match &m.table {
            Some(EntriesTable::Inline(span)) => {
                let ranges: Vec<Range<usize>> = m.entries.iter().map(|e| e.range.clone()).collect();
                separator_cut(t, &ranges, k, span.end - 1)
            }
            // A table's keys always start their own lines.
            _ => line_cut(t, &entry.range, entry.section),
        };
        let cut = Edit::Cut {
            item: Item::Instance(name.to_owned(), entry.piece),
            before,
            after,
        };
        inv.push(apply_edit(t, scheme, &cut, false)?);
    }
    if found {
        Ok(())
    } else {
        Err(format!("no instance `{name}`"))
    }
}

/// The bytes a removal of connection `k` takes around it.
fn connection_cut(t: &str, m: &Map, k: usize) -> Result<(String, String), String> {
    let r = m.conns.get(k).ok_or("no such connection")?.range.clone();
    Ok(if m.aot || owns_line(t, &r) {
        line_cut(t, &r, m.aot)
    } else {
        let ranges: Vec<Range<usize>> = m.conns.iter().map(|c| c.range.clone()).collect();
        separator_cut(t, &ranges, k, m.array.map_or(r.end, |(_, close)| close))
    })
}

fn cut_connection(t: &mut String, k: usize, inv: &mut Edits) -> Result<(), String> {
    let m = map(t, &Scheme::LEVEL)?;
    let (before, after) = connection_cut(t, &m, k)?;
    let cut = Edit::Cut {
        item: Item::Connection(k),
        before,
        after,
    };
    inv.push(apply_edit(t, &Scheme::LEVEL, &cut, false)?);
    Ok(())
}

/// connection.add of `entry` (an inline table, or a `[[wiring.connections]]`
/// section in that form), the last element of `connections`.
fn add_connection(t: &mut String, entry: &str, inv: &mut Edits) -> Result<(), String> {
    let s = &Scheme::LEVEL;
    let m = map(t, s)?;
    if m.aot {
        let last = m.conns.last().ok_or("no connection section")?.range.clone();
        return insert_at(t, s, line_end(t, last.end), &format!("\n{entry}\n"), inv);
    }
    if let Some((open, _)) = m.array {
        return match m.conns.last().map(|c| c.range.clone()) {
            None if t[open..line_end(t, open)].trim().is_empty() => {
                insert_at(t, s, line_end(t, open), &format!("  {entry},\n"), inv)
            }
            None => insert_at(t, s, open, &format!("\n  {entry},\n"), inv),
            Some(last) if owns_line(t, &last) => {
                let indent = t[line_start(t, last.start)..last.start].to_owned();
                let comma = t[last.end..]
                    .trim_start_matches([' ', '\t'])
                    .starts_with(',');
                if !comma {
                    insert_at(t, s, last.end, ",", inv)?;
                }
                let text = if comma {
                    format!("{indent}{entry},\n")
                } else {
                    format!("{indent}{entry}\n")
                };
                insert_at(t, s, line_end(t, last.end), &text, inv)
            }
            Some(last) => {
                let spaces =
                    t[last.end..].len() - t[last.end..].trim_start_matches([' ', '\t']).len();
                let p = last.end + spaces;
                if t[p..].starts_with(',') {
                    insert_at(t, s, p + 1, &format!("\n  {entry},\n"), inv)
                } else {
                    insert_at(t, s, last.end, &format!(",\n  {entry},\n"), inv)
                }
            }
        };
    }
    if let Some(header) = m.wiring_header {
        return insert_at(
            t,
            s,
            line_end(t, header),
            &format!("connections = [\n  {entry},\n]\n"),
            inv,
        );
    }
    let end = m
        .entries
        .iter()
        .map(|e| e.range.end)
        .max()
        .unwrap_or(m.settings_end);
    insert_at(
        t,
        s,
        line_end(t, end),
        &format!("\n[wiring]\nconnections = [\n  {entry},\n]\n"),
        inv,
    )
}

/// instance.add of `entry` (`name = { … }`), the last key of `[instances]`.
fn add_instance(
    t: &mut String,
    scheme: &Scheme<'_>,
    entry: &str,
    inv: &mut Edits,
) -> Result<(), String> {
    let m = map(t, scheme)?;
    let body_last = m
        .entries
        .iter()
        .filter(|e| !e.section)
        .map(|e| e.range.end)
        .max();
    let header = if scheme.connections {
        "instances".to_owned()
    } else {
        scheme.entries[..scheme.depth].join(".")
    };
    match (&m.table, body_last) {
        (Some(EntriesTable::Inline(span)), Some(end)) => {
            let _ = span;
            insert_at(t, scheme, end, &format!(", {entry}"), inv)
        }
        (Some(EntriesTable::Inline(span)), None) => {
            let open = span.start + 1;
            insert_at(t, scheme, open, &format!(" {entry} "), inv)
        }
        (Some(_), Some(end)) => insert_at(t, scheme, line_end(t, end), &format!("{entry}\n"), inv),
        (Some(EntriesTable::Explicit(header_end)), None) => {
            let header_end = *header_end;
            insert_at(
                t,
                scheme,
                line_end(t, header_end),
                &format!("{entry}\n"),
                inv,
            )
        }
        (Some(EntriesTable::Implicit), None) => {
            let end = m
                .entries
                .iter()
                .map(|e| e.range.end)
                .max()
                .unwrap_or(t.len());
            insert_at(
                t,
                scheme,
                line_end(t, end),
                &format!("\n[{header}]\n{entry}\n"),
                inv,
            )
        }
        (None, _) => {
            let end = line_end(t, m.settings_end);
            insert_at(t, scheme, end, &format!("\n[{header}]\n{entry}\n"), inv)
        }
    }
}

fn references(conn: &Conn, name: &str) -> bool {
    [&conn.from, &conn.to]
        .iter()
        .any(|e| e.split('.').next() == Some(name))
}

/// instance.remove: its connections at the level (last first), then it.
fn remove_instance(t: &mut String, name: &str, inv: &mut Edits) -> Result<(), String> {
    let m = map(t, &Scheme::LEVEL)?;
    let refs: Vec<usize> = m
        .conns
        .iter()
        .enumerate()
        .filter(|(_, c)| references(c, name))
        .map(|(k, _)| k)
        .collect();
    for k in refs.into_iter().rev() {
        cut_connection(t, k, inv)?;
    }
    cut_instance(t, &Scheme::LEVEL, name, inv)
}

/// Connection `k`'s text with `scale` set (`None`: removed).
fn scaled(entry: &str, aot: bool, scale: Option<f64>) -> Result<String, String> {
    let value = scale.map(number);
    if aot {
        let mut doc: DocumentMut = format!("{entry}\n")
            .parse()
            .map_err(|e: toml_edit::TomlError| e.message().to_owned())?;
        let table = doc
            .get_mut("wiring")
            .and_then(|w| w.get_mut("connections"))
            .and_then(TomlItem::as_array_of_tables_mut)
            .and_then(|a| a.get_mut(0))
            .ok_or("not a connection section")?;
        standard(table, "scale", value);
        let text = doc.to_string();
        return Ok(text.strip_suffix('\n').unwrap_or(&text).to_owned());
    }
    let mut doc: DocumentMut = format!("x = {entry}\n")
        .parse()
        .map_err(|e: toml_edit::TomlError| e.message().to_owned())?;
    let table = doc
        .get_mut("x")
        .and_then(TomlItem::as_inline_table_mut)
        .ok_or("not an inline table")?;
    inline(table, "scale", value, &["from", "to", "scale"]);
    let text = doc.to_string();
    let text = text.strip_prefix("x = ").ok_or("unexpected rendering")?;
    Ok(text.strip_suffix('\n').unwrap_or(text).to_owned())
}

/// connection.scale: the connection is cut as a removal cuts it and put
/// back in place with its new text (each step leaves a file that parses).
fn scale_connection(
    t: &mut String,
    k: usize,
    scale: Option<f64>,
    inv: &mut Edits,
) -> Result<(), String> {
    let m = map(t, &Scheme::LEVEL)?;
    let r = m.conns.get(k).ok_or("no such connection")?.range.clone();
    let new = scaled(&t[r.clone()], m.aot, scale)?;
    let (before, after) = connection_cut(t, &m, k)?;
    let start = r.start - before.len();
    let text = format!("{before}{new}{after}");
    let cut = Edit::Cut {
        item: Item::Connection(k),
        before,
        after,
    };
    inv.push(apply_edit(t, &Scheme::LEVEL, &cut, false)?);
    insert_at(t, &Scheme::LEVEL, start, &text, inv)
}

/// `name`'s position written as `schematic.move` writes it, a stale entry of
/// that name first removed.
fn add_position(
    t: &mut String,
    level: &str,
    name: &str,
    x: i64,
    y: i64,
    inv: &mut Edits,
) -> Result<(), String> {
    let scheme = positions_scheme(level);
    if map(t, &scheme)?.entries.iter().any(|e| e.name == name) {
        cut_instance(t, &scheme, name, inv)?;
    }
    let after = set_position(t, level, name, x, y)?;
    let prefix = t
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let rest = t.len() - prefix;
    let suffix = t[prefix..]
        .bytes()
        .rev()
        .zip(after[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(rest);
    if rest != suffix {
        return Err("ui/schematic.toml: the position is not a plain insertion".into());
    }
    if !t.is_char_boundary(prefix) || !after.is_char_boundary(after.len() - rest) {
        return Err("ui/schematic.toml: the position is not a plain insertion".into());
    }
    let inserted = after[prefix..after.len() - rest].to_owned();
    insert_at(t, &scheme, prefix, &inserted, inv)
}

fn remove_position(t: &mut String, level: &str, name: &str, inv: &mut Edits) -> Result<(), String> {
    let scheme = positions_scheme(level);
    if map(t, &scheme)?.entries.iter().any(|e| e.name == name) {
        cut_instance(t, &scheme, name, inv)?;
    }
    Ok(())
}

// The forward-pair rule (untrusted edits).

fn forward(
    t: &str,
    f: impl FnOnce(&mut String, &mut Edits) -> Result<(), String>,
) -> Option<String> {
    let mut t = t.to_owned();
    f(&mut t, &mut Vec::new()).ok().map(|()| t)
}

/// `with` holds instance `name` and `without` does not: is one exactly the
/// other's forward instance.remove / instance.add (its entry verbatim)?
fn instance_pair(with: &str, without: &str, name: &str) -> bool {
    let (Ok(m), Ok(n)) = (map(with, &Scheme::LEVEL), map(without, &Scheme::LEVEL)) else {
        return false;
    };
    let Some(entry) = m.entries.iter().find(|e| e.name == name) else {
        return false;
    };
    if n.entries.iter().any(|e| e.name == name) {
        return false;
    }
    let text = with[entry.range.clone()].to_owned();
    let whole = m.entries.iter().filter(|e| e.name == name).count() == 1;
    forward(with, |t, inv| remove_instance(t, name, inv)).as_deref() == Some(without)
        || (whole
            && !entry.section
            && forward(without, |t, inv| {
                add_instance(t, &Scheme::LEVEL, &text, inv)
            })
            .as_deref()
                == Some(with))
}

/// `with` holds connection `k` and `without` does not.
fn connection_pair(with: &str, without: &str, k: usize) -> bool {
    let (Ok(m), Ok(n)) = (map(with, &Scheme::LEVEL), map(without, &Scheme::LEVEL)) else {
        return false;
    };
    let Some(conn) = m.conns.get(k) else {
        return false;
    };
    if m.conns.len() != n.conns.len() + 1 {
        return false;
    }
    let text = with[conn.range.clone()].to_owned();
    forward(with, |t, inv| cut_connection(t, k, inv)).as_deref() == Some(without)
        || (k + 1 == m.conns.len()
            && forward(without, |t, inv| add_connection(t, &text, inv)).as_deref() == Some(with))
}

fn positions_pair(with: &str, without: &str, level: &str, name: &str) -> bool {
    let Some((x, y)) = read_positions(with)
        .ok()
        .and_then(|p| p.get(level).and_then(|l| l.get(name)).copied())
    else {
        return false;
    };
    forward(with, |t, inv| remove_position(t, level, name, inv)).as_deref() == Some(without)
        || forward(without, |t, inv| add_position(t, level, name, x, y, inv)).as_deref()
            == Some(with)
}

/// The connection an untrusted op's edits cut (in the text before them).
fn cut_connection_of(edits: &[Edit]) -> Option<usize> {
    edits.iter().find_map(|e| match e {
        Edit::Cut {
            item: Item::Connection(k),
            ..
        } => Some(*k),
        _ => None,
    })
}

// The ops.

struct Patching<'a> {
    text: String,
    positions: Option<String>,
    positions_changed: bool,
    ctx: &'a Context<'a>,
    notes: Vec<String>,
    renamed: Vec<(String, String)>,
    follow: Vec<(String, Option<String>)>,
    /// The other files written, by path: (as read, as now).
    files: BTreeMap<String, (Option<String>, Option<String>)>,
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn coordinate(value: Option<f64>, key: &str) -> Result<Option<i64>, PatchError> {
    match value {
        None => Ok(None),
        Some(v) if v.is_finite() && v.abs() <= 1e9 => Ok(Some(v.round() as i64)),
        Some(_) => Err(bad(format!("`{key}` must be a finite number, at most 1e9"))),
    }
}

fn written(e: String) -> PatchError {
    refused("write-failed", e)
}

fn rendered(key: &str, value: Value) -> String {
    let mut doc = DocumentMut::new();
    doc.insert(key, TomlItem::Value(value));
    let text = doc.to_string();
    text.strip_suffix('\n').unwrap_or(&text).to_owned()
}

/// `name = { module = "…", <params in declared order> }`.
fn instance_entry(
    name: &str,
    module: &str,
    params: &BTreeMap<String, f64>,
    order: &[String],
) -> String {
    let mut table = InlineTable::new();
    table.insert("module", Value::from(module));
    for key in order.iter().filter(|k| params.contains_key(*k)) {
        table.insert(key, number(params[key]));
    }
    for (key, value) in params.iter().filter(|(k, _)| !order.contains(k)) {
        table.insert(key, number(*value));
    }
    rendered(name, Value::InlineTable(table))
}

fn connection_entry(aot: bool, from: &str, to: &str, scale: Option<f64>) -> String {
    let scale = scale.filter(|s| *s != 1.0);
    if aot {
        let mut table = Table::new();
        table.insert("from", toml_edit::value(from));
        table.insert("to", toml_edit::value(to));
        if let Some(s) = scale {
            table.insert("scale", TomlItem::Value(number(s)));
        }
        let mut doc = DocumentMut::new();
        let mut array = toml_edit::ArrayOfTables::new();
        array.push(table);
        let mut wiring = Table::new();
        wiring.set_implicit(true);
        wiring.insert("connections", TomlItem::ArrayOfTables(array));
        doc.insert("wiring", TomlItem::Table(wiring));
        let text = doc.to_string();
        return text.trim_end_matches('\n').to_owned();
    }
    let mut table = InlineTable::new();
    table.insert("from", Value::from(from));
    table.insert("to", Value::from(to));
    if let Some(s) = scale {
        table.insert("scale", number(s));
    }
    rendered("x", Value::InlineTable(table))
        .strip_prefix("x = ")
        .unwrap_or_default()
        .to_owned()
}

/// An instance entry's module and numeric parameters.
fn instance_fields(t: &str, name: &str) -> (String, BTreeMap<String, f64>) {
    let mut module = String::new();
    let mut params = BTreeMap::new();
    if let Ok(doc) = Document::parse(t)
        && let Some(entry) = doc
            .get("instances")
            .and_then(TomlItem::as_table_like)
            .and_then(|i| i.get(name))
            .and_then(TomlItem::as_table_like)
    {
        for (key, item) in entry.iter() {
            match key {
                "module" => module = item.as_str().unwrap_or_default().to_owned(),
                "gpu" => {}
                _ => {
                    if let Some(v) = item.as_value().and_then(value_number) {
                        params.insert(key.to_owned(), v);
                    }
                }
            }
        }
    }
    (module, params)
}

impl Patching<'_> {
    fn level_map(&self) -> Result<Map, PatchError> {
        map(&self.text, &Scheme::LEVEL).map_err(|e| refused("does-not-load", e))
    }

    /// Where `name` is drawn at this level: none when `ui/schematic.toml` is
    /// missing or does not read (only an edit of it then fails, in
    /// [`Self::layout`]).
    fn position(&self, name: &str) -> Option<(i64, i64)> {
        let positions = read_positions(self.positions.as_deref()?).ok()?;
        positions.get(self.ctx.level)?.get(name).copied()
    }

    /// Edits `ui/schematic.toml` with `f`; returns the layout of the inverse.
    fn layout(
        &mut self,
        f: impl FnOnce(&mut String, &mut Edits) -> Result<(), String>,
    ) -> Result<Layout, PatchError> {
        let created = self.positions.is_none();
        let mut text = self.positions.clone().unwrap_or_default();
        if !text.is_empty() {
            read_positions(&text).map_err(written)?;
        }
        let mut inv = Vec::new();
        f(&mut text, &mut inv).map_err(written)?;
        inv.reverse();
        self.positions = Some(text);
        self.positions_changed = true;
        Ok(Layout {
            edits: inv,
            created,
        })
    }

    /// Untrusted layout edits, checked by the forward-pair rule; `adds`: the
    /// op holds `name` after them.
    fn checked_layout(
        &mut self,
        layout: &Layout,
        name: &str,
        adds: bool,
    ) -> Result<Layout, PatchError> {
        let level = self.ctx.level;
        let created = self.positions.is_none();
        let before = self.positions.clone().unwrap_or_default();
        let mut after = before.clone();
        let inverse = run_checked(&mut after, &positions_scheme(level), &layout.edits)?;
        if !after.is_empty() {
            read_positions(&after).map_err(bad)?;
        }
        let (with, without) = if adds {
            (&after, &before)
        } else {
            (&before, &after)
        };
        if !positions_pair(with, without, level, name) {
            return Err(bad("the layout edits are not this op's position"));
        }
        self.positions = (!(layout.created && after.is_empty())).then_some(after);
        self.positions_changed = true;
        Ok(Layout {
            edits: inverse,
            created,
        })
    }

    fn op(&mut self, op: &Op) -> Result<Vec<Op>, PatchError> {
        // ADR 0046 §7: a slot deleted takes its part with it, in the same
        // patch; the undo gives it back byte for byte. Only the forward op:
        // an inverse (`edits`) carries its own `panel.restore` (the redo of
        // a deletion's undo), or undoes a `slot.add`, whose slot had no
        // part when it was added.
        let slot_removed = match op {
            Op::InstanceRemove {
                name, edits: None, ..
            } if slot::is_slot(&self.text, name) => Some(name.clone()),
            _ => None,
        };
        let mut inverse = self.op_one(op)?;
        if let Some(slot) = slot_removed
            && let Some(restore) = self.remove_part(&slot)?
        {
            inverse.push(restore);
        }
        Ok(inverse)
    }

    fn op_one(&mut self, op: &Op) -> Result<Vec<Op>, PatchError> {
        let level = self.ctx.level;
        let one = match op {
            Op::InstanceAdd {
                name,
                module,
                params,
                x,
                y,
                edits: None,
                ..
            } => {
                if !valid_name(name) {
                    return Err(bad(format!("`{name}`: a name is [a-z][a-z0-9_]*")));
                }
                if self.level_map()?.entries.iter().any(|e| &e.name == name) {
                    return Err(bad(format!("the level already has an instance `{name}`")));
                }
                let order = (self.ctx.module)(module)
                    .ok_or_else(|| refused("unknown-module", format!("no module `{module}`")))?;
                if params.values().any(|v| !v.is_finite()) {
                    return Err(bad("parameter values must be finite"));
                }
                let (x, y) = (coordinate(*x, "x")?, coordinate(*y, "y")?);
                let entry = instance_entry(name, module, params, &order);
                let mut inv = Vec::new();
                add_instance(&mut self.text, &Scheme::LEVEL, &entry, &mut inv).map_err(written)?;
                inv.reverse();
                let layout = match (x, y) {
                    (Some(x), Some(y)) => {
                        Some(self.layout(|t, inv| add_position(t, level, name, x, y, inv))?)
                    }
                    (None, None) => None,
                    _ => return Err(bad("a position needs both `x` and `y`")),
                };
                Ok(Op::InstanceRemove {
                    name: name.clone(),
                    edits: Some(inv),
                    layout,
                })
            }
            Op::InstanceRemove {
                name, edits: None, ..
            } => {
                if !self.level_map()?.entries.iter().any(|e| &e.name == name) {
                    return Err(refused(
                        "unknown-instance",
                        format!("no instance `{name}` at this level"),
                    ));
                }
                let (module, params) = instance_fields(&self.text, name);
                let position = self.position(name);
                let mut inv = Vec::new();
                remove_instance(&mut self.text, name, &mut inv).map_err(written)?;
                inv.reverse();
                let layout = match position {
                    Some(_) => Some(self.layout(|t, inv| remove_position(t, level, name, inv))?),
                    None => None,
                };
                Ok(Op::InstanceAdd {
                    name: name.clone(),
                    module,
                    params,
                    x: position.map(|p| p.0 as f64),
                    y: position.map(|p| p.1 as f64),
                    edits: Some(inv),
                    layout,
                })
            }
            Op::ConnectionAdd {
                from,
                to,
                scale,
                edits: None,
            } => {
                let m = self.level_map()?;
                for end in [from, to] {
                    let (instance, port) = end
                        .split_once('.')
                        .ok_or_else(|| bad(format!("`{end}` is not `instance.port`")))?;
                    if port.is_empty() {
                        return Err(bad(format!("`{end}` is not `instance.port`")));
                    }
                    if !m.entries.iter().any(|e| e.name == instance) {
                        return Err(refused(
                            "unknown-instance",
                            format!("no instance `{instance}` at this level"),
                        ));
                    }
                }
                if scale.is_some_and(|s| !s.is_finite()) {
                    return Err(bad("`scale` must be a finite number"));
                }
                if m.conns.iter().any(|c| &c.from == from && &c.to == to) {
                    return Err(bad(format!("{from} → {to} is already connected")));
                }
                let entry = connection_entry(m.aot, from, to, *scale);
                let mut inv = Vec::new();
                add_connection(&mut self.text, &entry, &mut inv).map_err(written)?;
                inv.reverse();
                Ok(Op::ConnectionRemove {
                    from: from.clone(),
                    to: to.clone(),
                    edits: Some(inv),
                })
            }
            Op::ConnectionRemove {
                from,
                to,
                edits: None,
            } => {
                let m = self.level_map()?;
                let k = m
                    .conns
                    .iter()
                    .position(|c| &c.from == from && &c.to == to)
                    .ok_or_else(|| bad(format!("no connection {from} → {to}")))?;
                let scale = m.conns[k].scale;
                let mut inv = Vec::new();
                cut_connection(&mut self.text, k, &mut inv).map_err(written)?;
                inv.reverse();
                Ok(Op::ConnectionAdd {
                    from: from.clone(),
                    to: to.clone(),
                    scale,
                    edits: Some(inv),
                })
            }
            Op::ConnectionScale {
                from,
                to,
                scale,
                edits: None,
            } => {
                if !scale.is_finite() {
                    return Err(bad("`scale` must be a finite number"));
                }
                let m = self.level_map()?;
                let k = m
                    .conns
                    .iter()
                    .position(|c| &c.from == from && &c.to == to)
                    .ok_or_else(|| bad(format!("no connection {from} → {to}")))?;
                let old = m.conns[k].scale.unwrap_or(1.0);
                let mut inv = Vec::new();
                let new = (*scale != 1.0).then_some(*scale);
                scale_connection(&mut self.text, k, new, &mut inv).map_err(written)?;
                inv.reverse();
                Ok(Op::ConnectionScale {
                    from: from.clone(),
                    to: to.clone(),
                    scale: old,
                    edits: Some(inv),
                })
            }
            // Untrusted edits: the gap checks, then the forward-pair rule.
            Op::InstanceAdd {
                name,
                edits: Some(edits),
                layout,
                ..
            } => {
                let before = self.text.clone();
                let inv = run_checked(&mut self.text, &Scheme::LEVEL, edits)?;
                if !instance_pair(&self.text, &before, name) {
                    return Err(bad(format!("the edits are not instance.add `{name}`")));
                }
                let layout = layout
                    .as_ref()
                    .map(|l| self.checked_layout(l, name, true))
                    .transpose()?;
                Ok(Op::InstanceRemove {
                    name: name.clone(),
                    edits: Some(inv),
                    layout,
                })
            }
            Op::InstanceRemove {
                name,
                edits: Some(edits),
                layout,
            } => {
                let before = self.text.clone();
                let (module, params) = instance_fields(&before, name);
                let position = self.position(name);
                let inv = run_checked(&mut self.text, &Scheme::LEVEL, edits)?;
                if !instance_pair(&before, &self.text, name) {
                    return Err(bad(format!("the edits are not instance.remove `{name}`")));
                }
                let layout = layout
                    .as_ref()
                    .map(|l| self.checked_layout(l, name, false))
                    .transpose()?;
                Ok(Op::InstanceAdd {
                    name: name.clone(),
                    module,
                    params,
                    x: position.map(|p| p.0 as f64),
                    y: position.map(|p| p.1 as f64),
                    edits: Some(inv),
                    layout,
                })
            }
            Op::ConnectionAdd {
                from,
                to,
                scale,
                edits: Some(edits),
            } => {
                let before = self.text.clone();
                let inv = run_checked(&mut self.text, &Scheme::LEVEL, edits)?;
                let m = self.level_map()?;
                let k = cut_connection_of(&inv)
                    .filter(|k| {
                        m.conns
                            .get(*k)
                            .is_some_and(|c| &c.from == from && &c.to == to)
                    })
                    .ok_or_else(|| bad(format!("the edits do not add {from} → {to}")))?;
                if !connection_pair(&self.text, &before, k) {
                    return Err(bad(format!(
                        "the edits are not connection.add {from} → {to}"
                    )));
                }
                let _ = scale;
                Ok(Op::ConnectionRemove {
                    from: from.clone(),
                    to: to.clone(),
                    edits: Some(inv),
                })
            }
            Op::ConnectionRemove {
                from,
                to,
                edits: Some(edits),
            } => {
                let before = self.text.clone();
                let m = self.level_map()?;
                let inv = run_checked(&mut self.text, &Scheme::LEVEL, edits)?;
                let k = cut_connection_of(edits)
                    .filter(|k| {
                        m.conns
                            .get(*k)
                            .is_some_and(|c| &c.from == from && &c.to == to)
                    })
                    .ok_or_else(|| bad(format!("the edits do not remove {from} → {to}")))?;
                let scale = m.conns[k].scale;
                if !connection_pair(&before, &self.text, k) {
                    return Err(bad(format!(
                        "the edits are not connection.remove {from} → {to}"
                    )));
                }
                Ok(Op::ConnectionAdd {
                    from: from.clone(),
                    to: to.clone(),
                    scale,
                    edits: Some(inv),
                })
            }
            Op::ConnectionScale {
                from,
                to,
                scale,
                edits: Some(edits),
            } => {
                let before = self.text.clone();
                let m = self.level_map()?;
                let inv = run_checked(&mut self.text, &Scheme::LEVEL, edits)?;
                let k = cut_connection_of(edits)
                    .filter(|k| {
                        m.conns
                            .get(*k)
                            .is_some_and(|c| &c.from == from && &c.to == to)
                    })
                    .ok_or_else(|| bad(format!("the edits do not scale {from} → {to}")))?;
                let old = m.conns[k].scale;
                let new = (*scale != 1.0).then_some(*scale);
                let ok = forward(&before, |t, inv| scale_connection(t, k, new, inv)).as_deref()
                    == Some(self.text.as_str())
                    || forward(&self.text, |t, inv| scale_connection(t, k, old, inv)).as_deref()
                        == Some(before.as_str());
                if !ok
                    || self
                        .level_map()?
                        .conns
                        .get(k)
                        .is_none_or(|c| &c.from != from || &c.to != to)
                {
                    return Err(bad(format!(
                        "the edits are not connection.scale {from} → {to}"
                    )));
                }
                Ok(Op::ConnectionScale {
                    from: from.clone(),
                    to: to.clone(),
                    scale: old.unwrap_or(1.0),
                    edits: Some(inv),
                })
            }
            Op::SlotAdd {
                name,
                slot,
                load,
                x,
                y,
            } => {
                if !valid_name(name) {
                    return Err(bad(format!("`{name}`: a name is [a-z][a-z0-9_]*")));
                }
                if self.level_map()?.entries.iter().any(|e| &e.name == name) {
                    return Err(bad(format!("the level already has an instance `{name}`")));
                }
                let (x, y) = (coordinate(*x, "x")?, coordinate(*y, "y")?);
                let entry = slot::slot_entry(name, slot, load.as_deref())?;
                let mut inv = Vec::new();
                add_instance(&mut self.text, &Scheme::LEVEL, &entry, &mut inv).map_err(written)?;
                inv.reverse();
                let layout = match (x, y) {
                    (Some(x), Some(y)) => {
                        Some(self.layout(|t, inv| add_position(t, level, name, x, y, inv))?)
                    }
                    (None, None) => None,
                    _ => return Err(bad("a position needs both `x` and `y`")),
                };
                Ok(Op::InstanceRemove {
                    name: name.clone(),
                    edits: Some(inv),
                    layout,
                })
            }
            Op::SlotLoad {
                slot,
                load,
                map,
                set,
            } => {
                let edit = slot::SlotEdit::Load {
                    load,
                    map: map.as_ref(),
                    set: set.as_ref(),
                };
                return self.slot_edit(slot, edit);
            }
            Op::SlotUnload { slot } => return self.slot_edit(slot, slot::SlotEdit::Unload),
            Op::SlotMap { slot, map } => return self.slot_edit(slot, slot::SlotEdit::Map(map)),
            Op::SlotInterface {
                slot,
                add,
                remove,
                rename,
                retype,
                remove_wires,
                detach,
                template,
                ..
            } => {
                let edit = slot::SlotEdit::Interface {
                    add,
                    remove,
                    rename,
                    retype,
                    remove_wires: *remove_wires,
                    detach: *detach,
                    template: template.as_deref(),
                };
                return self.slot_edit(slot, edit);
            }
            Op::SlotRestore {
                slot,
                changes,
                wires,
            } => {
                let (changes, wires) = self.restore(slot, changes, wires)?;
                Ok(Op::SlotRestore {
                    slot: slot.clone(),
                    changes,
                    wires,
                })
            }
            Op::SlotInside { slot, name } => return self.slot_inside(slot, name.as_deref()),
            Op::CompositeFile { name, text, was } => self.composite_file(name, text, was),
            Op::ExportConnect { role, name, port } => self.export_edit(true, *role, name, port),
            Op::ExportDisconnect { role, name, port } => self.export_edit(false, *role, name, port),
            Op::ExportRestore { changes } => self.export_restore(changes),
            Op::SlotGroup { instance, .. } => return self.slot_group(instance),
            Op::PanelRestore { file, text, was } => self.panel_restore(file, text, was),
        };
        one.map(|op| vec![op])
    }
}

/// ADR 0037 §7: the contents `ops` would write into slots' `load`
/// (`slot.add`, `slot.load`, a `slot.restore` of `load`), as written.
pub fn loads_of(ops: &[Op]) -> Vec<String> {
    let mut loads = Vec::new();
    for op in ops {
        match op {
            Op::SlotAdd {
                load: Some(load), ..
            }
            | Op::SlotLoad { load, .. } => loads.push(load.clone()),
            Op::SlotRestore { changes, .. } => {
                for change in changes.iter().filter(|c| c.path == ["load"]) {
                    let read = change.value.as_deref().and_then(|text| {
                        toml::from_str::<toml::Table>(&format!("v = {text}")).ok()
                    });
                    if let Some(load) = read.as_ref().and_then(|t| t.get("v")?.as_str()) {
                        loads.push(load.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    loads
}

/// ADR 0037 §7: the projects the instances of `text` (a `project.toml` or a
/// composite's file) include (`project = "…"`) or load into slots (`load`
/// of an entry with `slot`, when it is a project path), as written.
pub fn projects_in(text: &str) -> Vec<String> {
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let entries = table.get("instances").and_then(toml::Value::as_table);
    for entry in entries.into_iter().flat_map(|t| t.values()) {
        let Some(entry) = entry.as_table() else {
            continue;
        };
        if let Some(project) = entry.get("project").and_then(toml::Value::as_str) {
            found.push(project.to_owned());
        }
        if entry.get("slot").is_some()
            && let Some(load) = entry.get("load").and_then(toml::Value::as_str)
            && crate::is_project_path(load)
        {
            found.push(load.to_owned());
        }
    }
    found
}

/// The instance names `ops` remove.
pub fn removed_instances(ops: &[Op]) -> Vec<&str> {
    ops.iter()
        .filter_map(|op| match op {
            Op::InstanceRemove { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod slot_tests;

#[cfg(test)]
mod inside_tests;

#[cfg(test)]
mod group_tests;
