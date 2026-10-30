//! A slot's own panel part (ADR 0046): `ui/parts/<key>.html`, a panel
//! fragment whose bindings are relative to the slot, its **root**.
//!
//! - The file and its owner: `parts/<slot>.html` for a top-level slot,
//!   `parts/<composite>.<slot>.html` for a slot in a composite's file (one
//!   part for every instance of it); its `<signal-panel part="…">` names the
//!   slot as `inside_of` does (`synth`, `rack/fx`).
//! - Relative addresses: `.<name>` names a port or parameter of the slot's
//!   interface, `/<path>.<port>` a port inside its content; rooted at
//!   `/synth` they are `/synth.<name>` and `/synth/<path>.<port>`, the
//!   ordinary absolute addresses every other mechanism takes.
//! - The panel tree: a panel's `<signal-panel>` content read and written
//!   as the panel editor does (`ui/lib/panel-tree.js`, within
//!   `ui/lib/panel-whitelist.json`), so Group into slot can move controls
//!   from `ui/main.html` into a new part and write both canonically.
//! - Card ids (ADR 0032, `ui/app/lib/cards.js`) and `ui/windows.toml`'s
//!   card lists, rewritten when a card moves into a part (`<slot>:<id>`).
//!
//! Presentation only: nothing here reaches a plan (hard rule 3).

use toml_edit::{DocumentMut, Item, Value};

use crate::bindings::{PANEL_ELEMENT, SLOT_ELEMENT, elements, panel_content};
use crate::panel_write::{CONTAINERS, class_token, decode, whitelist};

/// The folder of the parts, under `ui/`.
pub const PARTS_DIR: &str = "parts";

/// Whether `name` is a name of the project format: `[a-z][a-z0-9_]*`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `key` is a part's key: `<slot>` or `<composite>.<slot>`.
pub fn valid_key(key: &str) -> bool {
    match key.split_once('.') {
        Some((composite, slot)) => valid_name(composite) && valid_name(slot),
        None => valid_name(key),
    }
}

/// The key of a part file named relative to `ui/` (`parts/synth.html`).
pub fn key_of(file: &str) -> Option<&str> {
    let key = file
        .strip_prefix("parts/")
        .and_then(|f| f.strip_suffix(".html"))?;
    valid_key(key).then_some(key)
}

/// The part's file relative to `ui/`.
pub fn file_of(key: &str) -> String {
    format!("{PARTS_DIR}/{key}.html")
}

/// The owner a part's `<signal-panel part>` names: `synth`, `rack/fx`.
pub fn marker(key: &str) -> String {
    key.replacen('.', "/", 1)
}

/// The slot of a part as a slot resolution names it: `/synth` at the top
/// level, `rack/fx` in a composite's file.
pub fn slot_of(key: &str) -> String {
    match key.split_once('.') {
        Some((composite, slot)) => format!("{composite}/{slot}"),
        None => format!("/{key}"),
    }
}

/// A new part file: a comment naming it and its `<signal-panel part>`
/// holding `content` (a canonical panel content, `\n…\n`).
pub fn part_text(key: &str, content: &str) -> String {
    let slot = marker(key);
    format!(
        "<!-- The part of slot `{slot}` (ADR 0046): its own cards, bound inside the slot (.<name>: its interface; /<path>.<port>: inside its content). -->\n\
<{PANEL_ELEMENT} part=\"{slot}\">{content}</{PANEL_ELEMENT}>\n"
    )
}

/// The `part` attribute of `text`'s `<signal-panel>`, with its offset
/// (`None`: no panel element, or no such attribute).
pub fn marker_in(text: &str) -> Option<(String, usize)> {
    let panel = elements(text)
        .into_iter()
        .find(|e| e.name == PANEL_ELEMENT)?;
    let attribute = panel.attribute("part")?;
    Some(
        attribute
            .value
            .clone()
            .unwrap_or_else(|| (String::new(), panel.offset)),
    )
}

/// `address` as written in a part, rooted at `root` (`/synth`, `/rack1/fx`):
/// `.<name>` → `<root>.<name>`, `/<path>.<port>` → `<root>/<path>.<port>`.
/// Anything else lies outside the slot, which a part cannot name.
pub fn root_address(address: &str, root: &str) -> Result<String, String> {
    let outside = || {
        format!(
            "`{address}` is not inside the slot: a part binds `.<name>` (the slot's interface) or `/<path>.<port>` (inside its content); bind it from the host's panel"
        )
    };
    if let Some(name) = address.strip_prefix('.') {
        if !valid_name(name) {
            return Err(outside());
        }
        return Ok(format!("{root}.{name}"));
    }
    if let Some(inner) = address.strip_prefix('/') {
        let (path, port) = inner.rsplit_once('.').ok_or_else(outside)?;
        if port.is_empty() || path.split('/').any(|s| !valid_name(s)) {
            return Err(outside());
        }
        return Ok(format!("{root}{address}"));
    }
    Err(outside())
}

/// The inverse of [`root_address`] for the slot `root` (`/synth`): an
/// absolute address in the slot as a part writes it (`/synth.cutoff` →
/// `.cutoff`, `/synth/vcf.cutoff` → `/vcf.cutoff`), `None` outside it.
pub fn relative_address(address: &str, root: &str) -> Option<String> {
    let rest = address.strip_prefix(root)?;
    if let Some(name) = rest.strip_prefix('.') {
        return valid_name(name).then(|| format!(".{name}"));
    }
    let inner = rest.strip_prefix('/')?;
    let (path, port) = inner.rsplit_once('.')?;
    (!port.is_empty() && path.split('/').all(valid_name)).then(|| rest.to_owned())
}

/// The addresses of a `bind` attribute value, as the controls read it.
pub fn bind_list(bind: &str) -> Vec<&str> {
    bind.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect()
}

/// ADR 0046 §8: the ports and parameters of instance `name` of a level
/// that the host panels in `ui` bind, through each of the level's instance
/// paths `paths` (`""` for the top level), by name, in file and source
/// order (`slot.group` puts them into the interface).
pub fn bound_on(ui: &std::path::Path, paths: &[String], name: &str) -> Vec<String> {
    let prefixes: Vec<String> = paths.iter().map(|p| format!("{p}/{name}")).collect();
    let mut found: Vec<String> = Vec::new();
    for (_, _, address) in crate::bindings::references(ui, &prefixes) {
        for prefix in &prefixes {
            if let Some(port) = address
                .strip_prefix(prefix.as_str())
                .and_then(|r| r.strip_prefix('.'))
                && valid_name(port)
                && !found.iter().any(|f| f == port)
            {
                found.push(port.to_owned());
            }
        }
    }
    found
}

// The panel tree (`ui/lib/panel-tree.js`).

/// A container's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `div.signal-cards`, a grid of cards.
    Cards,
    /// `section.signal-card`.
    Card,
    /// `div.row`.
    Row,
    /// `div.signal-group`.
    Group,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Cards => "div.signal-cards",
            Kind::Card => "section.signal-card",
            Kind::Row => "div.row",
            Kind::Group => "div.signal-group",
        }
    }

    fn element(self) -> (&'static str, &'static str) {
        match self {
            Kind::Cards => ("div", "signal-cards"),
            Kind::Card => ("section", "signal-card"),
            Kind::Row => ("div", "row"),
            Kind::Group => ("div", "signal-group"),
        }
    }

    fn of_key(key: &str) -> Option<Kind> {
        [Kind::Cards, Kind::Card, Kind::Row, Kind::Group]
            .into_iter()
            .find(|k| k.key() == key)
    }
}

/// A node of a panel's content.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A container, its classes after the required one.
    Container {
        /// Which.
        kind: Kind,
        /// Its classes but the required one.
        classes: Vec<String>,
        /// A card's place on the plate (ADR 0047 §2), as written.
        place: Option<String>,
        /// What it holds.
        children: Vec<Node>,
    },
    /// A heading or description: `h1`, `h2`, `p`.
    Text {
        /// `h1`, `h2` or `p`.
        tag: String,
        /// Its classes.
        classes: Vec<String>,
        /// Its text, its runs of space one space.
        text: String,
    },
    /// A control, its attributes as written (`None`: bare).
    Control {
        /// `signal-knob`, …
        tag: String,
        /// Its attributes in order.
        attributes: Vec<(String, Option<String>)>,
    },
}

impl Node {
    /// A control's attribute.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        match self {
            Node::Control { attributes, .. } => attributes
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_deref().unwrap_or("")),
            _ => None,
        }
    }
}

/// `{line}, column {column}` of `offset` in `text`.
fn at(text: &str, offset: usize) -> String {
    let before = text.get(..offset).unwrap_or("");
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    format!("line {line}, column {column}")
}

/// A tag's name at the start of `s`: letters, digits and `-`, lower-cased.
fn tag_name(s: &str) -> Option<String> {
    let len = s
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
        .count();
    (len > 0 && s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic))
        .then(|| s.get(..len).unwrap_or("").to_ascii_lowercase())
}

/// An open element while reading.
struct Open {
    key: String,
    tag: String,
    node: Node,
}

/// The content of a `<signal-panel>` as a tree, read as the panel editor
/// reads an existing file (`parseContent`'s read mode: comments skipped,
/// everything else within the whitelist), or why not.
pub fn parse(html: &str) -> Result<Vec<Node>, String> {
    let list = whitelist();
    let mut roots: Vec<Node> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let fail = |offset: usize, message: String| format!("{}: {message}", at(html, offset));
    let mut i = 0;
    while i < html.len() {
        let rest = html.get(i..).unwrap_or("");
        let lt = rest.find('<').map_or(html.len(), |p| i + p);
        if lt > i {
            let chunk = html.get(i..lt).unwrap_or("");
            let text = decode(chunk).map_err(|m| fail(i, m))?;
            match stack.last_mut() {
                Some(Open {
                    node: Node::Text { text: t, .. },
                    ..
                }) => t.push_str(&text),
                _ if !text.trim().is_empty() => {
                    return Err(fail(
                        i,
                        format!(
                            "text \"{}\" is outside a heading or description",
                            text.trim()
                        ),
                    ));
                }
                _ => {}
            }
        }
        if lt >= html.len() {
            break;
        }
        i = lt;
        let rest = html.get(i..).unwrap_or("");
        if rest.starts_with("<!--") {
            let end = rest
                .get(4..)
                .and_then(|r| r.find("-->"))
                .ok_or_else(|| fail(i, "an unterminated comment".to_owned()))?;
            i += 4 + end + 3;
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let Some(name) = tag_name(after) else {
                return Err(fail(i, "a malformed end tag".to_owned()));
            };
            let close = after.get(name.len()..).unwrap_or("").trim_start();
            if !close.starts_with('>') {
                return Err(fail(i, format!("</{name}> is not closed")));
            }
            match stack.pop() {
                Some(open) if open.tag == name => {
                    let node = open.node;
                    match stack.last_mut() {
                        Some(Open {
                            node: Node::Container { children, .. },
                            ..
                        }) => children.push(node),
                        Some(_) => {
                            return Err(fail(i, format!("</{name}> closes inside a leaf")));
                        }
                        None => roots.push(node),
                    }
                }
                _ => {
                    return Err(fail(
                        i,
                        format!("</{name}> does not close the open element"),
                    ));
                }
            }
            i = html.len() - close.len() + 1;
            continue;
        }
        let Some(tag) = tag_name(rest.get(1..).unwrap_or("")) else {
            return Err(fail(
                i,
                format!(
                    "\"{}\" is not a tag the editor writes",
                    rest.chars().take(2).collect::<String>()
                ),
            ));
        };
        let begin = i;
        i += 1 + tag.len();
        let mut attributes: Vec<(String, Option<String>, usize)> = Vec::new();
        loop {
            let here = html.get(i..).unwrap_or("");
            let spaced = here.len() - here.trim_start().len();
            i += spaced;
            match html.as_bytes().get(i) {
                None => return Err(fail(begin, format!("<{tag}> is not closed"))),
                Some(b'>') => {
                    i += 1;
                    break;
                }
                _ => {}
            }
            if spaced == 0 {
                return Err(fail(
                    i,
                    format!("<{tag}>: attributes are separated by spaces"),
                ));
            }
            let here = html.get(i..).unwrap_or("");
            let len = here
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | '>' | '/' | '=' | '<')
                })
                .unwrap_or(here.len());
            if len == 0 {
                return Err(fail(
                    i,
                    format!("<{tag}>: a stray character where an attribute belongs"),
                ));
            }
            let name = here.get(..len).unwrap_or("").to_ascii_lowercase();
            let offset = i;
            i += len;
            let eq = html.get(i..).unwrap_or("").trim_start();
            let value = if let Some(after) = eq.strip_prefix('=') {
                let quoted = after.trim_start();
                let Some(inner) = quoted.strip_prefix('"') else {
                    return Err(fail(
                        offset,
                        format!("<{tag} {name}>: the value is not in double quotes"),
                    ));
                };
                let Some(close) = inner.find('"') else {
                    return Err(fail(
                        offset,
                        format!("<{tag} {name}>: the value is not closed"),
                    ));
                };
                let start = html.len() - inner.len();
                i = start + close + 1;
                Some(decode(inner.get(..close).unwrap_or("")).map_err(|m| fail(start, m))?)
            } else {
                None
            };
            attributes.push((name, value, offset));
        }
        let classes: Vec<String> = attributes
            .iter()
            .find(|a| a.0 == "class")
            .and_then(|a| a.1.as_deref())
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let key = if list.controls.contains_key(&tag) {
            Some("control".to_owned())
        } else if matches!(tag.as_str(), "h1" | "h2" | "p") {
            Some(tag.clone())
        } else {
            let keys: Vec<&str> = CONTAINERS
                .iter()
                .filter(|(_, element, class)| *element == tag && classes.iter().any(|c| c == class))
                .map(|(key, _, _)| *key)
                .collect();
            keys.first()
                .filter(|_| keys.len() == 1)
                .map(|k| (*k).to_owned())
        };
        let Some(key) = key else {
            return Err(fail(
                begin,
                format!("<{tag}> is not an element the editor writes"),
            ));
        };
        let parent = stack.last().map_or("panel", |o| o.key.as_str());
        if !list.fits(parent, &key, &tag) {
            return Err(fail(begin, format!("<{tag}> is not allowed in {parent}")));
        }
        let empty = Vec::new();
        let permitted = match key.as_str() {
            "control" => list.controls.get(&tag).unwrap_or(&empty),
            _ => list.containers.get(&key).map_or(&empty, |(a, _)| a),
        };
        for (n, (name, value, offset)) in attributes.iter().enumerate() {
            if attributes.iter().take(n).any(|a| a.0 == *name) {
                return Err(fail(*offset, format!("<{tag}>: {name} is given twice")));
            }
            if !permitted.contains(name) {
                return Err(fail(
                    *offset,
                    format!("<{tag} {name}>: not an attribute it takes"),
                ));
            }
            if value.is_none() && !list.boolean.contains(name) {
                return Err(fail(*offset, format!("<{tag} {name}>: needs a value")));
            }
        }
        for (n, class) in classes.iter().enumerate() {
            if !class_token(class) || classes.iter().take(n).any(|c| c == class) {
                return Err(fail(
                    begin,
                    format!("<{tag}>: the class \"{class}\" is not a lowercase name given once"),
                ));
            }
        }
        let node = if key == "control" {
            let mut values: Vec<(String, Option<String>)> = Vec::new();
            for (name, value, _) in attributes {
                if name == "class" {
                    if !classes.is_empty() {
                        values.push((name, Some(classes.join(" "))));
                    }
                    continue;
                }
                let bare = list.boolean.contains(&name);
                values.push((
                    name,
                    if bare {
                        None
                    } else {
                        Some(value.unwrap_or_default())
                    },
                ));
            }
            Node::Control {
                tag: tag.clone(),
                attributes: values,
            }
        } else if matches!(key.as_str(), "h1" | "h2" | "p") {
            Node::Text {
                tag: key.clone(),
                classes,
                text: String::new(),
            }
        } else {
            let kind = Kind::of_key(&key).ok_or_else(|| fail(begin, format!("<{tag}>")))?;
            let required = kind.element().1;
            let place = attributes
                .iter()
                .find(|(name, _, _)| name == "place")
                .and_then(|(_, value, _)| value.clone());
            Node::Container {
                kind,
                classes: classes.into_iter().filter(|c| c != required).collect(),
                place,
                children: Vec::new(),
            }
        };
        stack.push(Open { key, tag, node });
    }
    if let Some(open) = stack.last() {
        return Err(format!("<{}> is not closed", open.tag));
    }
    fn trim(node: &mut Node) {
        match node {
            Node::Text { text, .. } => {
                *text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            Node::Container { children, .. } => children.iter_mut().for_each(trim),
            Node::Control { .. } => {}
        }
    }
    roots.iter_mut().for_each(trim);
    Ok(roots)
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_value(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

/// A class attribute as the editor writes it: the required name, then
/// `wide`, `scope`, then the rest.
fn class_text(required: Option<&str>, classes: &[String]) -> String {
    let mut ordered: Vec<&str> = required.into_iter().collect();
    for first in ["wide", "scope"] {
        if classes.iter().any(|c| c == first) {
            ordered.push(first);
        }
    }
    ordered.extend(
        classes
            .iter()
            .map(String::as_str)
            .filter(|c| *c != "wide" && *c != "scope"),
    );
    if ordered.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", escape_value(&ordered.join(" ")))
    }
}

/// The canonical text of a panel's content (`serialize` of
/// `ui/lib/panel-tree.js`): a line per element, two spaces per level from
/// one, attributes in the whitelist's order.
pub fn serialize(nodes: &[Node]) -> String {
    let list = whitelist();
    let rank = |name: &str| {
        list.order
            .iter()
            .position(|o| o == name)
            .unwrap_or(list.order.len())
    };
    fn write(
        node: &Node,
        depth: usize,
        lines: &mut Vec<String>,
        rank: &dyn Fn(&str) -> usize,
        boolean: &[String],
    ) {
        let pad = "  ".repeat(depth);
        match node {
            Node::Control { tag, attributes } => {
                let mut names: Vec<&(String, Option<String>)> = attributes.iter().collect();
                names.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then(a.0.cmp(&b.0)));
                let text: String = names
                    .iter()
                    .map(|(n, v)| {
                        if boolean.contains(n) {
                            format!(" {n}")
                        } else {
                            format!(" {n}=\"{}\"", escape_value(v.as_deref().unwrap_or("")))
                        }
                    })
                    .collect();
                lines.push(format!("{pad}<{tag}{text}></{tag}>"));
            }
            Node::Text { tag, classes, text } => {
                lines.push(format!(
                    "{pad}<{tag}{}>{}</{tag}>",
                    class_text(None, classes),
                    escape_text(text)
                ));
            }
            Node::Container {
                kind,
                classes,
                place,
                children,
            } => {
                let (tag, name) = kind.element();
                let place = place
                    .as_deref()
                    .map(|p| format!(" place=\"{}\"", escape_value(p)))
                    .unwrap_or_default();
                let open = format!("{pad}<{tag}{}{place}>", class_text(Some(name), classes));
                if children.is_empty() {
                    lines.push(format!("{open}</{tag}>"));
                    return;
                }
                lines.push(open);
                for child in children {
                    write(child, depth + 1, lines, rank, boolean);
                }
                lines.push(format!("{pad}</{tag}>"));
            }
        }
    }
    let mut lines = Vec::new();
    for node in nodes {
        write(node, 1, &mut lines, &rank, &list.boolean);
    }
    let mut out = String::from("\n");
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The controls of `nodes` in document order.
pub fn controls(nodes: &[Node]) -> Vec<&Node> {
    fn walk<'n>(node: &'n Node, found: &mut Vec<&'n Node>) {
        match node {
            Node::Control { .. } => found.push(node),
            Node::Container { children, .. } => children.iter().for_each(|c| walk(c, found)),
            Node::Text { .. } => {}
        }
    }
    let mut found = Vec::new();
    nodes.iter().for_each(|n| walk(n, &mut found));
    found
}

// Card ids (`ui/app/lib/cards.js`).

/// A heading's slug: lower-case letters and digits, the rest as single
/// `-`, at most 64 characters, else `card`. Latin letters with marks lose
/// them, as the page's NFKD does.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        let folded = fold(c);
        for c in folded.chars() {
            let c = c.to_ascii_lowercase();
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
    }
    let cut: String = out.chars().take(64).collect();
    let cut = cut.trim_matches('-').to_owned();
    if cut.is_empty() {
        "card".to_owned()
    } else {
        cut
    }
}

/// A Latin letter without its marks (the common ones NFKD decomposes).
fn fold(c: char) -> String {
    const TABLE: [(&str, char); 14] = [
        ("àáâãäåāăą", 'a'),
        ("ÀÁÂÃÄÅĀĂĄ", 'A'),
        ("çćĉċč", 'c'),
        ("ÇĆĈĊČ", 'C'),
        ("èéêëēĕėęě", 'e'),
        ("ÈÉÊËĒĔĖĘĚ", 'E'),
        ("ìíîïĩīĭį", 'i'),
        ("ÌÍÎÏĨĪĬĮ", 'I'),
        ("ñńņň", 'n'),
        ("òóôõöōŏő", 'o'),
        ("ÒÓÔÕÖŌŎŐ", 'O'),
        ("ùúûüũūŭůűų", 'u'),
        ("ÙÚÛÜŨŪŬŮŰŲ", 'U'),
        ("ýÿ", 'y'),
    ];
    for (from, to) in TABLE {
        if from.contains(c) {
            return to.to_string();
        }
    }
    c.to_string()
}

/// The ids of cards titled `titles`, in order: each title's slug, made
/// unique with `-2`, `-3`, … (`cardIds` of the page, for cards without an
/// `id`).
pub fn card_ids(titles: &[String]) -> Vec<String> {
    let mut taken: Vec<String> = Vec::new();
    for title in titles {
        let base = slug(title);
        let mut id = base.clone();
        let mut n = 2;
        while taken.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        taken.push(id);
    }
    taken
}

/// A card's title as the page reads it: its first heading's text; an open
/// slot card's, its slot's name.
fn card_title(node: &Node) -> Option<String> {
    match node {
        Node::Container {
            kind: Kind::Card,
            children,
            ..
        } => Some(
            children
                .iter()
                .find_map(|c| match c {
                    Node::Text { tag, text, .. } if tag != "p" => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
        ),
        Node::Control { tag, .. } if tag == SLOT_ELEMENT => Some(
            node.attribute("bind")
                .unwrap_or("")
                .trim()
                .trim_start_matches('/')
                .to_owned(),
        ),
        _ => None,
    }
}

/// The cards of a panel's content in the page's order.
fn cards_of(nodes: &[Node]) -> Vec<&Node> {
    let mut cards = Vec::new();
    for node in nodes {
        if let Node::Container {
            kind: Kind::Cards,
            children,
            ..
        } = node
        {
            cards.extend(children.iter().filter(|c| card_title(c).is_some()));
        }
    }
    cards
}

/// The id of the open slot card of slot `name` (`ui/app/lib/cards.js`):
/// `slot.<name>`, which no heading's slug (no `.`) nor a part's card id
/// (`<slot>:<id>`) can be, so it never shifts another card's id.
pub fn slot_card_id(name: &str) -> String {
    format!("slot.{name}")
}

/// The ids of a host panel's cards, in order (`cardsOf` of the page): an
/// open slot card's `slot.<name>`; every other card its heading's slug,
/// made unique among those.
pub fn host_card_ids(cards: &[&Node]) -> Vec<String> {
    let headed: Vec<String> = cards
        .iter()
        .filter(|c| !matches!(c, Node::Control { .. }))
        .filter_map(|c| card_title(c))
        .collect();
    let mut slugs = card_ids(&headed).into_iter();
    cards
        .iter()
        .map(|c| match c {
            Node::Control { .. } => slot_card_id(&card_title(c).unwrap_or_default()),
            _ => slugs.next().unwrap_or_default(),
        })
        .collect()
}

// Group into slot: the host's controls on the instance moved into its part.

/// What Group into slot does to `ui/main.html` (ADR 0046 §8).
#[derive(Clone, Debug, PartialEq)]
pub struct Grouped {
    /// The main panel's new content.
    pub host: String,
    /// The part's content, when a control moved.
    pub part: Option<String>,
    /// Card ids that changed: a card moved into the part, old →
    /// `<slot>:<id>`; a card left whose id the move changed (a later card
    /// of the same heading), old → new.
    pub cards: Vec<(String, String)>,
    /// How many controls moved.
    pub moved: usize,
}

/// Whether control `node` binds only inside `root` (every address of its
/// `bind`, at least one), and those addresses made relative.
fn movable(node: &Node, root: &str) -> Option<String> {
    let Node::Control { tag, .. } = node else {
        return None;
    };
    if tag == SLOT_ELEMENT {
        return None;
    }
    let bind = node.attribute("bind")?;
    let addresses = bind_list(bind);
    if addresses.is_empty() {
        return None;
    }
    let relative: Option<Vec<String>> = addresses
        .iter()
        .map(|a| relative_address(a, root))
        .collect();
    relative.map(|r| r.join(" "))
}

/// `node` with only the controls `keep` says yes to (by their index in
/// document order, counted from `*next`); empty rows and groups dropped.
fn filtered(node: &Node, keep: &dyn Fn(usize) -> bool, next: &mut usize) -> Option<Node> {
    match node {
        Node::Control { .. } => {
            let index = *next;
            *next += 1;
            keep(index).then(|| node.clone())
        }
        Node::Text { .. } => Some(node.clone()),
        Node::Container {
            kind,
            classes,
            place,
            children,
        } => {
            let kept: Vec<Node> = children
                .iter()
                .filter_map(|c| filtered(c, keep, next))
                .collect();
            let holds = kept.iter().any(|c| !matches!(c, Node::Text { .. }));
            let gone = matches!(kind, Kind::Row | Kind::Group) && !holds;
            (!gone).then(|| Node::Container {
                kind: *kind,
                classes: classes.clone(),
                place: place.clone(),
                children: kept,
            })
        }
    }
}

/// The number of controls under `node`.
fn count(node: &Node) -> usize {
    match node {
        Node::Control { .. } => 1,
        Node::Container { children, .. } => children.iter().map(count).sum(),
        Node::Text { .. } => 0,
    }
}

/// `node` with every control's bind made relative to `root` (the moved
/// ones bind only inside it).
fn rebound(node: Node, root: &str) -> Node {
    match node {
        Node::Control { tag, attributes } => Node::Control {
            attributes: attributes
                .into_iter()
                .map(|(n, v)| {
                    if n == "bind" {
                        let relative = v.as_deref().and_then(|b| {
                            bind_list(b)
                                .iter()
                                .map(|a| relative_address(a, root))
                                .collect::<Option<Vec<String>>>()
                        });
                        (n, relative.map(|r| r.join(" ")).or(v))
                    } else {
                        (n, v)
                    }
                })
                .collect(),
            tag,
        },
        // ADR 0047: a place is the host plate's; in the part the card has none.
        Node::Container {
            kind,
            classes,
            children,
            ..
        } => Node::Container {
            kind,
            classes,
            place: None,
            children: children.into_iter().map(|c| rebound(c, root)).collect(),
        },
        text => text,
    }
}

/// The open slot card of slot `name`.
pub fn slot_card(name: &str) -> Node {
    placed_slot_card(name, None)
}

/// The open slot card of slot `name` at `place` on the plate (ADR 0047:
/// the place of the card it replaces), or with none.
fn placed_slot_card(name: &str, place: Option<&str>) -> Node {
    let mut attributes = vec![("bind".to_owned(), Some(format!("/{name}")))];
    if let Some(place) = place {
        attributes.push(("place".to_owned(), Some(place.to_owned())));
    }
    Node::Control {
        tag: SLOT_ELEMENT.to_owned(),
        attributes,
    }
}

/// Group into slot on the main panel's content `html` (ADR 0046 §8): the
/// controls whose every bind lies in the top-level instance `instance`
/// (those of `select` among them, by their index among the panel's
/// controls, when given) move into the part, binds made relative, labels,
/// skins, rows and groups kept. A card left with nothing but moved
/// controls is replaced in its place by the open slot card, and the part
/// keeps its heading (and so its id); a card that keeps other controls
/// keeps them, and the open slot card is inserted after it. With nothing
/// to move, the open slot card goes at the end of the first grid. `Err`:
/// the content is not one the editor writes.
pub fn group_content(
    html: &str,
    instance: &str,
    select: Option<&[usize]>,
) -> Result<Grouped, String> {
    let nodes = parse(html)?;
    let root = format!("/{instance}");
    let all = controls(&nodes);
    let moving: Vec<bool> = all
        .iter()
        .enumerate()
        .map(|(i, c)| movable(c, &root).is_some() && select.is_none_or(|s| s.contains(&i)))
        .collect();
    let moved = moving.iter().filter(|m| **m).count();
    let old_ids = host_card_ids(&cards_of(&nodes));
    let mut host: Vec<Node> = Vec::new();
    let mut part_cards: Vec<Node> = Vec::new();
    // Each host card: its old id, whether it went whole into the part.
    let mut whole: Vec<(String, usize)> = Vec::new();
    let mut placed = false;
    let mut next = 0;
    let mut card_index = 0;
    // The host's cards after the move, each with the card it was (none: the new slot card).
    let mut origins: Vec<Option<usize>> = Vec::new();
    for node in &nodes {
        let Node::Container {
            kind: Kind::Cards,
            classes,
            place: grid_place,
            children,
        } = node
        else {
            next += count(node);
            host.push(node.clone());
            continue;
        };
        let mut kept: Vec<Node> = Vec::new();
        for child in children {
            let is_card = card_title(child).is_some();
            let index = card_index;
            let id = if is_card {
                card_index += 1;
                old_ids.get(index).cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let origin = is_card.then_some(index);
            let start = next;
            let n = count(child);
            next += n;
            let ours: Vec<usize> = (start..start + n)
                .filter(|i| moving.get(*i).copied().unwrap_or(false))
                .collect();
            if ours.is_empty()
                || !matches!(
                    child,
                    Node::Container {
                        kind: Kind::Card,
                        ..
                    }
                )
            {
                kept.push(child.clone());
                if is_card {
                    origins.push(origin);
                }
                continue;
            }
            let mut from = start;
            let part = filtered(child, &|i| ours.contains(&i), &mut from);
            if let Some(part) = part {
                part_cards.push(rebound(part, &root));
            }
            if ours.len() == n {
                // Nothing but moved controls: the part keeps it.
                whole.push((id, part_cards.len() - 1));
                if !placed {
                    // In its place on the plate (ADR 0047).
                    let place = match child {
                        Node::Container { place, .. } => place.as_deref(),
                        _ => None,
                    };
                    kept.push(placed_slot_card(instance, place));
                    origins.push(None);
                    placed = true;
                }
            } else {
                let mut from = start;
                if let Some(rest) = filtered(child, &|i| !ours.contains(&i), &mut from) {
                    kept.push(rest);
                    origins.push(origin);
                }
                if !placed {
                    kept.push(slot_card(instance));
                    origins.push(None);
                    placed = true;
                }
            }
        }
        host.push(Node::Container {
            kind: Kind::Cards,
            classes: classes.clone(),
            place: grid_place.clone(),
            children: kept,
        });
    }
    if !placed {
        match host.iter_mut().find_map(|n| match n {
            Node::Container {
                kind: Kind::Cards,
                children,
                ..
            } => Some(children),
            _ => None,
        }) {
            Some(children) => {
                children.push(slot_card(instance));
                origins.push(None);
            }
            None => host.push(Node::Container {
                kind: Kind::Cards,
                classes: Vec::new(),
                place: None,
                children: vec![slot_card(instance)],
            }),
        }
    }
    let part_titles: Vec<String> = part_cards.iter().filter_map(card_title).collect();
    let part_ids = card_ids(&part_titles);
    let mut cards: Vec<(String, String)> = whole
        .into_iter()
        .filter(|(id, _)| !id.is_empty())
        .filter_map(|(id, at)| {
            part_ids
                .get(at)
                .map(|new| (id, format!("{instance}:{new}")))
        })
        .collect();
    // A card left keeps its window: an id the move shifted is renamed too.
    let new_ids = host_card_ids(&cards_of(&host));
    for (new, origin) in new_ids.iter().zip(&origins) {
        if let Some(old) = origin.and_then(|i| old_ids.get(i))
            && old != new
        {
            cards.push((old.clone(), new.clone()));
        }
    }
    let part = (!part_cards.is_empty()).then(|| {
        serialize(&[Node::Container {
            kind: Kind::Cards,
            classes: Vec::new(),
            place: None,
            children: part_cards,
        }])
    });
    Ok(Grouped {
        host: serialize(&host),
        part,
        cards,
        moved,
    })
}

/// The panel file `text` with its `<signal-panel>` content replaced.
pub fn with_content(text: &str, content: &str) -> Option<String> {
    let (start, end) = panel_content(text).ok()?;
    Some(crate::panel_write::replace(text, (start, end), content))
}

/// `ui/windows.toml`'s `text` with every card id of `renames` in a
/// window's `cards` rewritten (old → new, all at once); a window that now
/// holds a card of slot `slot`'s part (`<slot>:…`) holds its open slot card
/// too, before it, so the header stays with its cards. `None` when nothing
/// changes.
pub fn rename_cards(
    text: &str,
    renames: &[(String, String)],
    slot: Option<&str>,
) -> Option<String> {
    if renames.is_empty() {
        return None;
    }
    let mut doc: DocumentMut = text.parse().ok()?;
    let mut changed = false;
    let layouts = doc.get_mut("layout").and_then(Item::as_table_like_mut)?;
    for (_, entry) in layouts.iter_mut() {
        let Some(windows) = entry
            .as_table_like_mut()
            .and_then(|t| t.get_mut("windows"))
            .and_then(Item::as_array_of_tables_mut)
        else {
            continue;
        };
        for window in windows.iter_mut() {
            let Some(cards) = window.get_mut("cards").and_then(Item::as_array_mut) else {
                continue;
            };
            let mut first_part = None;
            for (i, card) in cards.iter_mut().enumerate() {
                let Some(id) = card.as_str() else {
                    continue;
                };
                if let Some((_, new)) = renames.iter().find(|(old, _)| old == id) {
                    if slot.is_some_and(|s| new.starts_with(&format!("{s}:"))) {
                        first_part.get_or_insert(i);
                    }
                    let decor = card.decor().clone();
                    let mut value = Value::from(new.as_str());
                    *value.decor_mut() = decor;
                    *card = value;
                    changed = true;
                }
            }
            if let (Some(at), Some(slot)) = (first_part, slot) {
                let header = slot_card_id(slot);
                if !cards.iter().any(|c| c.as_str() == Some(header.as_str())) {
                    // In the place of the card it goes before, with its spacing.
                    let prefix = cards
                        .get(at)
                        .and_then(|c| c.decor().prefix().cloned())
                        .unwrap_or_default();
                    let mut value = Value::from(header.as_str());
                    value.decor_mut().set_prefix(prefix);
                    cards.insert_formatted(at, value);
                    if let Some(next) = cards.get_mut(at + 1) {
                        next.decor_mut().set_prefix(" ");
                    }
                }
            }
        }
    }
    changed.then(|| doc.to_string())
}

// A part's controls on its slot's interface (ADR 0046 §7).

/// The controls of part `text` that bind `.<name>` of a name of `names`:
/// how each is shown (its label, else its tag and bind).
pub fn controls_on(text: &str, names: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for element in elements(text) {
        if element.name == PANEL_ELEMENT {
            continue;
        }
        let hit = element.addresses().iter().any(|(a, _)| {
            a.strip_prefix('.')
                .is_some_and(|n| names.iter().any(|m| m == n))
        });
        if hit {
            let label = element
                .attribute("label")
                .and_then(|a| a.value.as_ref())
                .map(|(v, _)| v.clone())
                .filter(|v| !v.is_empty());
            let bind = element
                .attribute("bind")
                .and_then(|a| a.value.as_ref())
                .map(|(v, _)| v.clone())
                .unwrap_or_default();
            found.push(label.unwrap_or_else(|| format!("<{}> {bind}", element.name)));
        }
    }
    found
}

/// Part `text` without its controls on `.<name>` of `names` (empty rows
/// and groups dropped), its content written canonically; `None` when it
/// has none, `Err` when its content is not one the editor writes.
pub fn without_controls(text: &str, names: &[String]) -> Result<Option<String>, String> {
    let (start, end) = panel_content(text)?;
    let nodes = parse(text.get(start..end).unwrap_or(""))?;
    let hit = |node: &Node| {
        node.attribute("bind").is_some_and(|b| {
            bind_list(b).iter().any(|a| {
                a.strip_prefix('.')
                    .is_some_and(|n| names.iter().any(|m| m == n))
            })
        })
    };
    let all = controls(&nodes);
    let drop: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, c)| hit(c))
        .map(|(i, _)| i)
        .collect();
    if drop.is_empty() {
        return Ok(None);
    }
    let mut next = 0;
    let kept: Vec<Node> = nodes
        .iter()
        .filter_map(|n| filtered(n, &|i| !drop.contains(&i), &mut next))
        .collect();
    Ok(with_content(text, &serialize(&kept)))
}

/// A part's controls as `graph` and `describe` give them (ADR 0046 §6,
/// §11), rooted at `root`: `{tag, label, bind (as written), binds (the
/// rooted addresses; an address that cannot be rooted is left out),
/// attributes}` in source order.
pub fn rooted_controls(text: &str, root: &str) -> Vec<serde_json::Value> {
    let mut list = Vec::new();
    for element in elements(text) {
        if element.name == PANEL_ELEMENT {
            continue;
        }
        let value = |name: &str| {
            element
                .attribute(name)
                .and_then(|a| a.value.as_ref())
                .map(|(v, _)| v.clone())
        };
        let bind = value("bind").unwrap_or_default();
        let binds: Vec<String> = bind_list(&bind)
            .iter()
            .filter_map(|a| root_address(a, root).ok())
            .collect();
        let attributes: serde_json::Map<String, serde_json::Value> = element
            .attributes
            .iter()
            .map(|a| {
                (
                    a.name.clone(),
                    serde_json::json!(a.value.as_ref().map_or("", |(v, _)| v.as_str())),
                )
            })
            .collect();
        list.push(serde_json::json!({
            "tag": element.name,
            "label": value("label"),
            "bind": bind,
            "binds": binds,
            "attributes": attributes,
        }));
    }
    list
}

#[cfg(test)]
mod tests;
