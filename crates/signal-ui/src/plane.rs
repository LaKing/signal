//! The Signal Project plane's file (ADR 0051 §4): `ui/plane.toml`, the
//! project's Stacks, each a vertical column of instruments in order. It is
//! presentation, like `ui/schematic.toml`: no address names a Stack, and no
//! plan, render or log reads it. The page (`ui/lib/plane-file.js`) reads,
//! checks and draws it; here is its check for `signal ui check`
//! (`plane.check`'s rules): a file that does not read, an unknown key, a
//! value of the wrong form, a Stack named twice, an entry naming no card of
//! `ui/main.html` (`card:<id>`) and no top-level slot with a part, and an
//! entry listed twice are errors. A card's id is its heading made a slug
//! (`panel_cards`), the page's rule. **One identity** (`entries`,
//! `canonical`; the page's `withEntries`, `canonicalEntry`): a card that
//! shows a top-level slot with a part is that slot's instrument, named by
//! the slot, so `card:<id>` of it and the slot's name are one entry. The warning for a top-level instance in
//! no instrument needs the page's assignment of instances to cards and is
//! the page's own (its "unstacked" tray).

use std::collections::{BTreeMap, BTreeSet};

use signal_project::{Diagnostic, Severity};

use crate::bindings::{Finding, PANEL_ELEMENT, SLOT_ELEMENT, Tag, scan};
use crate::panel_write::decode;

/// The file, in the project's `ui/`.
pub const PLANE_FILE: &str = "plane.toml";
/// The file's schema version.
pub const PLANE_SCHEMA: i64 = 1;
/// The largest coordinate or width, in plate units (as a plate's side).
pub const MAX_PU: i64 = 1000;
/// The migration form's prefix: a card of the panel drawn as an instrument.
pub const CARD_PREFIX: &str = "card:";

/// A card's id from its heading: lower case, every run of characters other
/// than `a-z` and `0-9` one `-`, none at the ends (the page's `cardSlug`).
pub fn card_slug(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

/// A card of a panel file: its id, its heading's text, and its slot (an
/// open slot card's, or a slot card's inside it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// The id an instrument entry names (`card:<id>`).
    pub id: String,
    /// The first heading's text (`h1` or `h2`), trimmed, spaces collapsed.
    pub heading: String,
    /// The slot it shows (`<signal-slot bind="/deck">`: `deck`).
    pub slot: Option<String>,
}

/// The cards of the panel file `text` (the children of its
/// `div.signal-cards` in the `<signal-panel>`), in order, with the page's
/// ids: the heading's slug, else the slot's, else `card-<n>`; a repeated id
/// gets `-2`, `-3`, ….
pub fn panel_cards(text: &str) -> Vec<Card> {
    struct Open {
        heading: Option<(usize, String)>,
        heading_text: Option<String>,
        slot: Option<String>,
    }
    let mut in_panel = false;
    let mut grid = false;
    let mut depth = 0usize;
    let mut found: Vec<Open> = Vec::new();
    let mut current: Option<Open> = None;
    scan(text, |tag| match tag {
        Tag::Start {
            name,
            attributes,
            end,
            ..
        } => {
            let attribute = |n: &str| {
                attributes
                    .iter()
                    .find(|a| a.name == n)
                    .and_then(|a| a.value.as_ref())
                    .map(|(v, _)| v.clone())
            };
            if name == PANEL_ELEMENT {
                in_panel = true;
                return;
            }
            if !in_panel {
                return;
            }
            let classes = attribute("class").unwrap_or_default();
            if !grid {
                if name == "div"
                    && classes
                        .split_ascii_whitespace()
                        .any(|c| c == "signal-cards")
                {
                    grid = true;
                    depth = 0;
                }
                return;
            }
            depth += 1;
            if depth == 1 {
                current = Some(Open {
                    heading: None,
                    heading_text: None,
                    slot: None,
                });
            }
            if let Some(card) = current.as_mut() {
                if name == SLOT_ELEMENT && card.slot.is_none() {
                    card.slot = attribute("bind").map(|b| b.trim_start_matches('/').to_owned());
                }
                if (name == "h1" || name == "h2")
                    && card.heading.is_none()
                    && card.heading_text.is_none()
                {
                    card.heading = Some((end, name.clone()));
                }
            }
        }
        Tag::End { name, offset } => {
            if !grid {
                if name == PANEL_ELEMENT {
                    in_panel = false;
                }
                return;
            }
            if depth == 0 {
                // The grid itself ends.
                if name == "div" {
                    grid = false;
                    in_panel = false;
                }
                return;
            }
            if let Some(card) = current.as_mut()
                && let Some((start, tag)) = card.heading.take()
            {
                if tag == name {
                    let raw = text.get(start..offset).unwrap_or("");
                    let decoded = decode(raw).unwrap_or_else(|_| raw.to_owned());
                    card.heading_text =
                        Some(decoded.split_whitespace().collect::<Vec<_>>().join(" "));
                } else {
                    card.heading = Some((start, tag));
                }
            }
            depth -= 1;
            if depth == 0
                && let Some(card) = current.take()
            {
                found.push(card);
            }
        }
    });
    let mut used = BTreeSet::new();
    found
        .into_iter()
        .enumerate()
        .map(|(i, card)| {
            let heading = card.heading_text.unwrap_or_default();
            let mut base = card_slug(&heading);
            if base.is_empty() {
                base = card.slot.as_deref().map(card_slug).unwrap_or_default();
            }
            if base.is_empty() {
                base = format!("card-{}", i + 1);
            }
            let mut id = base.clone();
            let mut n = 2;
            while used.contains(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            used.insert(id.clone());
            Card {
                id,
                heading,
                slot: card.slot,
            }
        })
        .collect()
}

/// The one identity rule (ADR 0051 §4–5): each card's instrument entry. A
/// card that shows a top-level slot with a part (`slots`) is that slot's
/// instrument, its entry the slot's name; only the first card showing a
/// slot is. Every other card's entry is `card:<id>`.
pub fn entries(cards: &[Card], slots: &[String]) -> Vec<String> {
    let mut taken = BTreeSet::new();
    cards
        .iter()
        .map(|card| match &card.slot {
            Some(slot) if slots.contains(slot) && taken.insert(slot.clone()) => slot.clone(),
            _ => format!("{CARD_PREFIX}{}", card.id),
        })
        .collect()
}

/// Entry `entry` under the one identity rule: `card:<id>` of a card that is
/// a slot's instrument is the slot's name; any other as written.
pub fn canonical(entry: &str, cards: &[Card], entries: &[String]) -> String {
    if let Some(id) = entry.strip_prefix(CARD_PREFIX)
        && let Some(full) = cards
            .iter()
            .position(|c| c.id == id)
            .and_then(|i| entries.get(i))
    {
        return full.clone();
    }
    entry.to_owned()
}

/// Whether `name` is a Stack's name: `[a-z][a-z0-9_]*`.
fn stack_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The findings of `ui/plane.toml`'s `text` (`file` as shown) against the
/// project: `cards` (the panel's cards) and `slots` (its top-level slots
/// with a part). Each entry is read canonically (`canonical`) first.
/// Errors only; in file order of the checks.
pub fn check(file: &str, text: &str, panel: &[Card], slots: &[String]) -> Vec<Finding> {
    let known = entries(panel, slots);
    let cards: Vec<String> = panel.iter().map(|c| c.id.clone()).collect();
    let mut found = Vec::new();
    let mut say = |offset: usize, message: String| {
        found.push(Finding {
            control: None,
            diagnostic: Diagnostic::at(
                Severity::Error,
                file,
                text,
                offset,
                format!("plane.toml: {message}"),
            ),
        });
    };
    let table: toml::Table = match toml::from_str(text) {
        Ok(table) => table,
        Err(error) => {
            let at = error.span().map_or(0, |s| s.start);
            say(at, error.message().to_owned());
            return found;
        }
    };
    let at = |needle: &str| text.find(needle).unwrap_or(0);
    for key in table.keys() {
        if key != "schema_version" && key != "stack" {
            say(
                at(key),
                format!("unknown key \"{key}\" (expected schema_version, stack)"),
            );
        }
    }
    match table.get("schema_version") {
        None => say(0, "schema_version is missing".to_owned()),
        Some(toml::Value::Integer(v)) if *v == PLANE_SCHEMA => {}
        Some(_) => say(
            at("schema_version"),
            format!("schema_version must be {PLANE_SCHEMA}"),
        ),
    }
    let stacks: &[toml::Value] = match table.get("stack") {
        None => &[],
        Some(toml::Value::Array(list)) if list.iter().all(toml::Value::is_table) => list,
        Some(_) => {
            say(
                at("stack"),
                "stack must be an array of tables ([[stack]])".to_owned(),
            );
            &[]
        }
    };
    let card_ids: BTreeSet<&str> = cards.iter().map(String::as_str).collect();
    let slot_names: BTreeSet<&str> = slots.iter().map(String::as_str).collect();
    let mut names = BTreeSet::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    // Each [[stack]] header's offset, in order, so a Stack's findings point
    // at it: a header begins its line, so a comment naming one is none.
    let headers: Vec<usize> = text
        .match_indices("[[stack]]")
        .map(|(i, _)| i)
        .filter(|&i| {
            text[..i]
                .rsplit('\n')
                .next()
                .is_some_and(|before| before.trim().is_empty())
        })
        .collect();
    for (i, stack) in stacks.iter().enumerate() {
        let Some(stack) = stack.as_table() else {
            continue;
        };
        let here = headers.get(i).copied().unwrap_or(0);
        let where_ = format!("stack {}", i + 1);
        for key in stack.keys() {
            if !["name", "at", "width", "instruments"].contains(&key.as_str()) {
                say(
                    here,
                    format!(
                        "{where_}: unknown key \"{key}\" (expected name, at, width, instruments)"
                    ),
                );
            }
        }
        let name = match stack.get("name") {
            Some(toml::Value::String(name)) if stack_name(name) => name.clone(),
            _ => {
                say(
                    here,
                    format!("{where_}: name must be a name ([a-z][a-z0-9_]*)"),
                );
                String::new()
            }
        };
        if !name.is_empty() && !names.insert(name.clone()) {
            say(here, format!("the Stack \"{name}\" is named twice"));
        }
        if let Some(value) = stack.get("at") {
            let ok = value.as_array().is_some_and(|a| {
                a.len() == 2
                    && a.iter()
                        .all(|v| v.as_integer().is_some_and(|n| n.abs() <= MAX_PU))
            });
            if !ok {
                say(
                    here,
                    format!(
                        "{where_}: at must be two whole numbers of pu, [x, y], each within ±{MAX_PU}"
                    ),
                );
            }
        }
        if let Some(value) = stack.get("width")
            && !value
                .as_integer()
                .is_some_and(|n| (1..=MAX_PU).contains(&n))
        {
            say(
                here,
                format!("{where_}: width must be a whole number of pu, 1 to {MAX_PU}"),
            );
        }
        let entries: Vec<String> = match stack.get("instruments") {
            None => {
                say(here, format!("{where_}: instruments is missing"));
                Vec::new()
            }
            Some(toml::Value::Array(list)) if list.iter().all(toml::Value::is_str) => list
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            Some(_) => {
                say(
                    here,
                    format!("{where_}: instruments must be an array of names"),
                );
                Vec::new()
            }
        };
        for written in entries {
            let offset = text.find(&format!("\"{written}\"")).unwrap_or(here);
            let entry = canonical(&written, panel, &known);
            match seen.get(&entry) {
                Some(first) if *first == name => say(
                    offset,
                    format!("\"{entry}\" is listed twice (in \"{name}\")"),
                ),
                Some(first) => say(
                    offset,
                    format!("\"{entry}\" is listed twice (in \"{first}\" and \"{name}\")"),
                ),
                None => {
                    seen.insert(entry.clone(), name.clone());
                }
            }
            if let Some(id) = entry.strip_prefix(CARD_PREFIX) {
                if !card_ids.contains(id) {
                    let known = if cards.is_empty() {
                        String::new()
                    } else {
                        format!(" (cards: {})", cards.join(", "))
                    };
                    say(
                        offset,
                        format!("\"{entry}\" names no card of ui/main.html{known}"),
                    );
                }
            } else if !slot_names.contains(entry.as_str()) {
                say(
                    offset,
                    format!(
                        "\"{entry}\" names no top-level slot with a part (a card is \"card:<id>\")"
                    ),
                );
            }
        }
    }
    found
}

/// ADR 0046 §8 with ADR 0051 §4: `ui/plane.toml`'s `text` after Group
/// into slot `slot` renamed the main panel's cards (`renames`, old → new,
/// [`crate::parts::Grouped::cards`]), in the same write. An entry
/// `card:<old>` of a card moved into the slot's part (`<slot>:<id>`)
/// becomes the slot's name, the instrument that card now is, in its place;
/// a second such entry (two cards moved into one part) is removed, since
/// the one instrument is listed once; a card left whose id the move
/// shifted is renamed (`card:<new>`). All renames at once. `None` when
/// nothing changes or the file does not read.
pub fn rename_instruments(text: &str, renames: &[(String, String)], slot: &str) -> Option<String> {
    if renames.is_empty() {
        return None;
    }
    let mut doc: toml_edit::DocumentMut = text.parse().ok()?;
    let stacks = doc
        .get_mut("stack")
        .and_then(toml_edit::Item::as_array_of_tables_mut)?;
    let part = format!("{slot}:");
    let mut listed = stacks.iter().any(|stack| {
        stack
            .get("instruments")
            .and_then(toml_edit::Item::as_array)
            .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(slot)))
    });
    let mut changed = false;
    for stack in stacks.iter_mut() {
        let Some(list) = stack
            .get_mut("instruments")
            .and_then(toml_edit::Item::as_array_mut)
        else {
            continue;
        };
        let mut i = 0;
        while i < list.len() {
            let renamed = list
                .get(i)
                .and_then(toml_edit::Value::as_str)
                .and_then(|entry| entry.strip_prefix(CARD_PREFIX))
                .and_then(|id| renames.iter().find(|(old, _)| old == id))
                .map(|(_, new)| new.clone());
            let Some(new) = renamed else {
                i += 1;
                continue;
            };
            changed = true;
            let entry = if new.starts_with(&part) {
                (!std::mem::replace(&mut listed, true)).then(|| slot.to_owned())
            } else {
                Some(format!("{CARD_PREFIX}{new}"))
            };
            match (entry, list.get_mut(i)) {
                (Some(entry), Some(value)) => {
                    let decor = value.decor().clone();
                    let mut written = toml_edit::Value::from(entry);
                    *written.decor_mut() = decor;
                    *value = written;
                    i += 1;
                }
                _ => {
                    // The next entry takes its spacing and any comment before it.
                    let removed = list.remove(i);
                    if let (Some(prefix), Some(next)) =
                        (removed.decor().prefix().cloned(), list.get_mut(i))
                    {
                        next.decor_mut().set_prefix(prefix);
                    }
                }
            }
        }
    }
    changed.then(|| doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: &str = "<!doctype html>\n<html><head><title>x</title><link rel=\"stylesheet\" href=\"a.css\"></head><body>\n\
        <signal-panel>\n  <div class=\"signal-cards\">\n    <section class=\"signal-card wide\"><h1>studio</h1><p>notes</p></section>\n\
        <section class=\"signal-card\"><h2>RHYTHM &amp; COMPOSER</h2><div class=\"row\"><signal-knob bind=\"/a.b\"></signal-knob></div></section>\n\
        <section class=\"signal-card\"><signal-slot bind=\"/deck_a\"></signal-slot></section>\n\
        <signal-slot bind=\"/deck_b\"></signal-slot>\n\
        <section class=\"signal-card\"><div class=\"row\"></div></section>\n\
        <section class=\"signal-card\"><h2>  Out </h2></section>\n<section class=\"signal-card\"><h2>out</h2></section>\n\
        </div>\n</signal-panel>\n<script type=\"module\">const s = '<section class=\"signal-card\"><h2>no</h2></section>';</script>\n</body></html>\n";

    #[test]
    fn card_ids_are_the_pages() {
        let ids: Vec<String> = panel_cards(PANEL).into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "studio",
                "rhythm-composer",
                "deck-a",
                "deck-b",
                "card-5",
                "out",
                "out-2"
            ]
        );
        assert_eq!(card_slug("  Tape · Echo!  "), "tape-echo");
        assert_eq!(card_slug("MONO SYNTH"), "mono-synth");
    }

    #[test]
    fn the_studios_plane_checks_clean() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/studio/ui");
        let panel = signal_project::read_regular_to_string(root.join("main.html")).unwrap();
        let plane = signal_project::read_regular_to_string(root.join(PLANE_FILE)).unwrap();
        let found = panel_cards(&panel);
        let cards: Vec<String> = found.iter().map(|c| c.id.clone()).collect();
        assert_eq!(
            cards,
            [
                "studio",
                "transport",
                "mono-synth",
                "rhythm-composer",
                "tape-echo",
                "master"
            ]
        );
        assert_eq!(check("ui/plane.toml", &plane, &found, &[]), Vec::new());
    }

    fn card(id: &str, slot: Option<&str>) -> Card {
        Card {
            id: id.to_owned(),
            heading: String::new(),
            slot: slot.map(str::to_owned),
        }
    }

    #[test]
    fn a_slot_with_a_part_is_one_entry_whichever_form_names_it() {
        let cards = [
            card("melody", None),
            card("synth", Some("synth")),
            card("deck-a", Some("deck_a")),
        ];
        let slots = ["synth".to_owned()];
        assert_eq!(
            entries(&cards, &slots),
            ["card:melody", "synth", "card:deck-a"]
        );
        let messages = |instruments: &str| -> Vec<String> {
            let text = format!(
                "schema_version = 1\n[[stack]]\nname = \"a\"\ninstruments = [{instruments}]\n"
            );
            check("ui/plane.toml", &text, &cards, &slots)
                .into_iter()
                .map(|f| f.diagnostic.message)
                .collect()
        };
        assert_eq!(
            messages("\"card:melody\", \"synth\", \"card:deck-a\""),
            Vec::<String>::new()
        );
        assert_eq!(
            messages("\"card:synth\""),
            Vec::<String>::new(),
            "the slot card's migration form names the slot"
        );
        assert_eq!(
            messages("\"card:synth\", \"synth\""),
            ["plane.toml: \"synth\" is listed twice (in \"a\")"]
        );
        assert_eq!(
            messages("\"deck_a\""),
            [
                "plane.toml: \"deck_a\" names no top-level slot with a part (a card is \"card:<id>\")"
            ],
            "a slot without a part is drawn by its card"
        );
    }

    #[test]
    fn what_the_file_gets_wrong_is_an_error() {
        let cards = [card("x", None)];
        let slots = ["deck".to_owned()];
        let messages = |text: &str| -> Vec<String> {
            check("ui/plane.toml", text, &cards, &slots)
                .into_iter()
                .map(|f| f.diagnostic.message)
                .collect()
        };
        assert_eq!(
            messages(
                "schema_version = 1\n[[stack]]\nname = \"a\"\ninstruments = [\"card:x\", \"deck\"]\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            messages("schema_version = 2\n"),
            ["plane.toml: schema_version must be 1"]
        );
        assert_eq!(messages(""), ["plane.toml: schema_version is missing"]);
        assert_eq!(
            messages("schema_version = 1\nzoom = 1\n"),
            ["plane.toml: unknown key \"zoom\" (expected schema_version, stack)"]
        );
        assert_eq!(
            messages(
                "schema_version = 1\n[[stack]]\nname = \"a\"\ninstruments = [\"card:x\", \"card:nope\", \"ghost\", \"card:x\"]\n[[stack]]\nname = \"a\"\nwidth = 0\nat = [1]\ninstruments = [\"deck\", \"deck\"]\n"
            ),
            [
                "plane.toml: \"card:nope\" names no card of ui/main.html (cards: x)",
                "plane.toml: \"ghost\" names no top-level slot with a part (a card is \"card:<id>\")",
                "plane.toml: \"card:x\" is listed twice (in \"a\")",
                "plane.toml: the Stack \"a\" is named twice",
                "plane.toml: stack 2: at must be two whole numbers of pu, [x, y], each within ±1000",
                "plane.toml: stack 2: width must be a whole number of pu, 1 to 1000",
                "plane.toml: \"deck\" is listed twice (in \"a\")",
            ]
        );
        assert_eq!(
            messages("schema_version = 1\n[[stack]]\nname = \"Primary\"\ninstruments = []\n"),
            ["plane.toml: stack 1: name must be a name ([a-z][a-z0-9_]*)"]
        );
        assert_eq!(
            check("ui/plane.toml", "[[stack]\n", &cards, &slots).len(),
            1
        );
    }

    #[test]
    fn a_comment_naming_a_stack_header_shifts_no_finding() {
        let text = "# one [[stack]] per Stack\nschema_version = 1\n[[stack]] # [[stack]]\nname = \"a\"\ninstruments = []\n[[stack]]\nname = \"b\"\nwidth = 0\ninstruments = []\n";
        let found = check("ui/plane.toml", text, &[], &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].diagnostic.line, 6, "at the second Stack's header");
    }

    #[test]
    fn group_into_slot_keeps_the_instruments() {
        let text = "schema_version = 1\n\n# the column\n[[stack]]\nname = \"primary\"\ninstruments = [\n  \"card:transport\",\n  \"card:tape-echo\", # the echo\n  \"card:echo-2\",\n  \"card:out-2\",\n]\n\n[[stack]]\nname = \"side\"\ninstruments = [\"card:spare\", \"card:master\"]\n";
        let renames = [
            ("tape-echo".to_owned(), "echo:tape-echo".to_owned()),
            ("echo-2".to_owned(), "echo:echo-2".to_owned()),
            ("out-2".to_owned(), "out".to_owned()),
            ("spare".to_owned(), "echo:spare".to_owned()),
        ];
        let new = rename_instruments(text, &renames, "echo").unwrap();
        assert_eq!(
            new,
            "schema_version = 1\n\n# the column\n[[stack]]\nname = \"primary\"\ninstruments = [\n  \"card:transport\",\n  \"echo\", # the echo\n  \"card:out\",\n]\n\n[[stack]]\nname = \"side\"\ninstruments = [\"card:master\"]\n"
        );
        // Nothing renamed, or no file that reads: no write.
        assert_eq!(rename_instruments(text, &[], "echo"), None);
        assert_eq!(
            rename_instruments(text, &[("x".to_owned(), "y".to_owned())], "echo"),
            None
        );
        assert_eq!(rename_instruments("[[stack", &renames, "echo"), None);
        // The slot listed already: the moved card's entry goes.
        let listed = "schema_version = 1\n[[stack]]\nname = \"a\"\ninstruments = [\"echo\", \"card:tape-echo\"]\n";
        assert_eq!(
            rename_instruments(listed, &renames, "echo").unwrap(),
            "schema_version = 1\n[[stack]]\nname = \"a\"\ninstruments = [\"echo\"]\n"
        );
    }
}
