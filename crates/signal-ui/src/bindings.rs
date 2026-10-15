//! The binding check of a project's panels (ADR 0023 §4,
//! `docs/ui-controls.md` "Panels"): every `bind` of a control in a
//! `ui/*.html` file against the plan. The port must exist, and its kind and
//! shape must fit the control: a fader, knob, toggle or 2D pad binds
//! controls (the top-level parameters `commit` writes to `project.toml`), a
//! value any parameter (a control or a plan parameter port), a button an
//! event input, a meter a signal input of the audio sink, a scope signal
//! outputs or inputs.
//!
//! `signal ui check` runs it on the project's files and the protocol's
//! `panel.check` on the running plan, so a panel shows at load the same
//! `file:line:column` diagnostics as the command.
//!
//! The HTML is not parsed into a tree: the scanner finds the start tags of
//! `signal-*` elements and their attributes, with byte offsets, skipping
//! comments and the content of `script`, `style`, `textarea` and `title`.
//! Attribute values are checked as a browser hands them to the component,
//! with character references decoded; a diagnostic still points at the
//! source.

use std::fs;
use std::path::{Path, PathBuf};

use signal_core::{Direction, PortAddress, PortShape, Registry, SinkKind};
use signal_graph::ExecutionPlan;
use signal_project::{Diagnostic, Project, Severity};

use crate::entities::ENTITIES;
use crate::panel::Panel;

/// The element that connects a panel and binds its controls.
pub const PANEL_ELEMENT: &str = "signal-panel";

/// The default panel's file in a project's `ui/`; without it a panel is
/// generated from `describe`.
pub const MAIN_PANEL: &str = "main.html";

/// An attribute of a start tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// Its name, in lower case.
    pub name: String,
    /// Its value with character references decoded, and the byte offset of
    /// its first character in the source; `None` for an attribute without `=`.
    pub value: Option<(String, usize)>,
    /// The source offset of each byte of the decoded value.
    pub offsets: Vec<usize>,
}

/// A `signal-*` start tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// Its name, in lower case (`signal-knob`).
    pub name: String,
    /// The byte offset of its `<`.
    pub offset: usize,
    /// Its attributes in order.
    pub attributes: Vec<Attribute>,
}

impl Element {
    /// The value of attribute `name` and its offset (the first, as HTML takes it).
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    /// The addresses in `bind` (space or comma separated, as the controls
    /// read it), each with its byte offset in the source.
    pub fn addresses(&self) -> Vec<(&str, usize)> {
        let Some(bind) = self.attribute("bind") else {
            return Vec::new();
        };
        let Some((value, start)) = bind.value.as_ref() else {
            return Vec::new();
        };
        let at = |i: usize| bind.offsets.get(i).copied().unwrap_or(*start + i);
        let mut found = Vec::new();
        let mut from = None;
        for (i, c) in value.char_indices().chain([(value.len(), ' ')]) {
            let separator = c.is_whitespace() || c == ',';
            match (from, separator) {
                (None, false) => from = Some(i),
                (Some(begin), true) => {
                    if let Some(address) = value.get(begin..i) {
                        found.push((address, at(begin)));
                    }
                    from = None;
                }
                _ => {}
            }
        }
        found
    }
}

/// What a control binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wants {
    /// Controls: top-level parameters, which `commit` writes on release.
    Controls,
    /// Parameters: a control or a plan parameter port (shown, never edited).
    Parameters,
    /// An event input of the plan.
    EventInput,
    /// A signal input of the audio sink.
    SinkInput,
    /// Signal outputs or inputs of the plan.
    SignalPorts,
    /// Vector outputs of the plan (M2.8-7, `<signal-canvas>`).
    VectorOutputs,
}

impl Wants {
    fn text(self) -> &'static str {
        match self {
            Wants::Controls => {
                "a control (a top-level parameter, /<instance>.<parameter>, which a release \
                 commits to project.toml)"
            }
            Wants::Parameters => "a parameter (a control or a plan parameter port)",
            Wants::EventInput => "an event input of the plan",
            Wants::SinkInput => "a signal input of the audio sink",
            Wants::SignalPorts => "a signal output or input of the plan",
            Wants::VectorOutputs => "a vector output of the plan",
        }
    }
}

/// The controls of `docs/ui-controls.md`: what each binds, and how many
/// addresses (at least, at most).
fn control(name: &str) -> Option<(Wants, usize, usize)> {
    Some(match name {
        "signal-fader" | "signal-knob" | "signal-toggle" => (Wants::Controls, 1, 1),
        "signal-value" => (Wants::Parameters, 1, 1),
        "signal-xy" => (Wants::Controls, 2, 2),
        "signal-button" => (Wants::EventInput, 1, 1),
        "signal-meter" => (Wants::SinkInput, 1, 1),
        "signal-scope" => (Wants::SignalPorts, 1, 2),
        "signal-canvas" => (Wants::VectorOutputs, 1, 3),
        _ => return None,
    })
}

const CONTROLS: &str = "signal-fader, signal-knob, signal-xy, signal-button, signal-toggle, \
                        signal-meter, signal-scope, signal-value, signal-canvas";

/// A diagnostic of the check, with the control it is about: its index among
/// the file's controls (the `signal-*` elements other than `<signal-panel>`,
/// in source order), which is how a loaded page finds the element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The control's index; `None` for the file as a whole.
    pub control: Option<usize>,
    /// What is wrong, where.
    pub diagnostic: Diagnostic,
}

/// One checked panel file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// The file, relative to the project folder (`ui/main.html`).
    pub file: String,
    /// Its controls.
    pub controls: usize,
    /// The addresses bound.
    pub bindings: usize,
    /// What is wrong.
    pub findings: Vec<Finding>,
}

impl Checked {
    /// Whether any finding is an error.
    pub fn failed(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.diagnostic.severity == Severity::Error)
    }
}

/// The `signal-*` start tags of an HTML text, in source order.
pub fn elements(text: &str) -> Vec<Element> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(lt) = text.get(at..).and_then(|rest| rest.find('<')) {
        let start = at + lt;
        let rest = text.get(start..).unwrap_or("");
        if rest.starts_with("<!--") {
            at = text
                .get(start + 4..)
                .and_then(|r| r.find("-->"))
                .map_or(text.len(), |end| start + 4 + end + 3);
            continue;
        }
        let name_len = rest
            .get(1..)
            .unwrap_or("")
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
            .count();
        if name_len == 0 || !bytes.get(start + 1).is_some_and(u8::is_ascii_alphabetic) {
            // A closing tag, a declaration, or a `<` in text.
            at = start + 1;
            continue;
        }
        let name = rest.get(1..1 + name_len).unwrap_or("").to_ascii_lowercase();
        let (attributes, end) = attributes(text, start + 1 + name_len);
        at = end;
        if matches!(name.as_str(), "script" | "style" | "textarea" | "title") {
            // Raw text up to the matching end tag.
            let close = format!("</{name}");
            at = text
                .get(at..)
                .and_then(|r| r.to_ascii_lowercase().find(&close))
                .map_or(text.len(), |i| at + i);
            continue;
        }
        if name.starts_with("signal-") {
            found.push(Element {
                name,
                offset: start,
                attributes,
            });
        }
    }
    found
}

/// The attributes of a start tag from byte `at` (after its name), and the
/// offset after its `>`.
fn attributes(text: &str, mut at: usize) -> (Vec<Attribute>, usize) {
    let bytes = text.as_bytes();
    let mut found: Vec<Attribute> = Vec::new();
    let skip_space = |mut at: usize| {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    loop {
        at = skip_space(at);
        match bytes.get(at) {
            None => return (found, text.len()),
            Some(b'>') => return (found, at + 1),
            Some(b'/') => {
                at += 1;
                continue;
            }
            Some(_) => {}
        }
        let begin = at;
        while bytes
            .get(at)
            .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/'))
        {
            at += 1;
        }
        let name = text.get(begin..at).unwrap_or("").to_ascii_lowercase();
        let after = skip_space(at);
        let value = if bytes.get(after) == Some(&b'=') {
            let start = skip_space(after + 1);
            match bytes.get(start) {
                Some(&quote) if quote == b'"' || quote == b'\'' => {
                    let end = text
                        .get(start + 1..)
                        .and_then(|r| r.find(quote as char))
                        .map_or(text.len(), |i| start + 1 + i);
                    at = (end + 1).min(text.len());
                    Some((start + 1, end))
                }
                _ => {
                    let mut end = start;
                    while bytes
                        .get(end)
                        .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                    {
                        end += 1;
                    }
                    at = end;
                    Some((start, end))
                }
            }
        } else {
            at = after.max(at);
            None
        };
        if at == begin {
            // Nothing consumed: a stray character.
            at += 1;
            continue;
        }
        if !name.is_empty() && !found.iter().any(|a| a.name == name) {
            let (value, offsets) = match value {
                Some((start, end)) => {
                    let (decoded, offsets) = decode(text.get(start..end).unwrap_or(""), start);
                    (Some((decoded, start)), offsets)
                }
                None => (None, Vec::new()),
            };
            found.push(Attribute {
                name,
                value,
                offsets,
            });
        }
    }
}

/// The numeric references HTML replaces (0x80 to 0x9F, read as
/// windows-1252), as the tokenizer's table.
const C1: [(u32, char); 27] = [
    (0x80, '\u{20ac}'),
    (0x82, '\u{201a}'),
    (0x83, '\u{192}'),
    (0x84, '\u{201e}'),
    (0x85, '\u{2026}'),
    (0x86, '\u{2020}'),
    (0x87, '\u{2021}'),
    (0x88, '\u{2c6}'),
    (0x89, '\u{2030}'),
    (0x8a, '\u{160}'),
    (0x8b, '\u{2039}'),
    (0x8c, '\u{152}'),
    (0x8e, '\u{17d}'),
    (0x91, '\u{2018}'),
    (0x92, '\u{2019}'),
    (0x93, '\u{201c}'),
    (0x94, '\u{201d}'),
    (0x95, '\u{2022}'),
    (0x96, '\u{2013}'),
    (0x97, '\u{2014}'),
    (0x98, '\u{2dc}'),
    (0x99, '\u{2122}'),
    (0x9a, '\u{161}'),
    (0x9b, '\u{203a}'),
    (0x9c, '\u{153}'),
    (0x9e, '\u{17e}'),
    (0x9f, '\u{178}'),
];

/// The longest name of the named-reference table that `rest` (the text
/// after an `&`) starts with, and its value.
fn named(rest: &str) -> Option<(&'static str, &'static str)> {
    let longest = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b';')
        .count()
        .min(32);
    (1..=longest).rev().find_map(|len| {
        let prefix = rest.get(..len)?;
        ENTITIES
            .binary_search_by(|(name, _)| name.as_bytes().cmp(prefix.as_bytes()))
            .ok()
            .and_then(|i| ENTITIES.get(i).copied())
    })
}

/// `raw` (an attribute value at source offset `start`) with its character
/// references decoded as the HTML tokenizer decodes them in an attribute
/// value, and the source offset of each decoded byte. A named reference is
/// the longest name of the WHATWG table; one without `;` followed by `=` or
/// an ASCII letter or digit is left as written (the attribute-value rule).
fn decode(raw: &str, start: usize) -> (String, Vec<usize>) {
    let mut decoded = String::with_capacity(raw.len());
    let mut offsets = Vec::with_capacity(raw.len());
    let mut push = |text: &str, at: usize, decoded: &mut String| {
        decoded.push_str(text);
        offsets.extend(std::iter::repeat_n(start + at, text.len()));
    };
    let mut i = 0;
    while let Some(c) = raw.get(i..).and_then(|r| r.chars().next()) {
        let rest = raw.get(i + 1..).unwrap_or("");
        let reference: Option<(String, usize)> = if c != '&' {
            None
        } else if let Some(number) = rest.strip_prefix('#') {
            let (hex, digits) = match number.strip_prefix(['x', 'X']) {
                Some(digits) => (true, digits),
                None => (false, number),
            };
            let len = digits
                .bytes()
                .take_while(|b| {
                    if hex {
                        b.is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    }
                })
                .count();
            (len > 0).then(|| {
                let text = digits.get(..len).unwrap_or("");
                // Too large for u32 is above 0x10FFFF too: U+FFFD.
                let code = u32::from_str_radix(text, if hex { 16 } else { 10 }).unwrap_or(0);
                let value = C1
                    .iter()
                    .find(|(c1, _)| *c1 == code)
                    .map(|(_, c)| *c)
                    .or_else(|| char::from_u32(code).filter(|&c| c != '\0'))
                    .unwrap_or('\u{fffd}');
                // `&`, `#`, the `x`, the digits and an optional `;`.
                let semicolon = digits.get(len..).is_some_and(|r| r.starts_with(';'));
                (
                    value.to_string(),
                    2 + usize::from(hex) + len + usize::from(semicolon),
                )
            })
        } else {
            named(rest).and_then(|(name, value)| {
                let next = rest.as_bytes().get(name.len());
                let kept = !name.ends_with(';')
                    && next.is_some_and(|b| *b == b'=' || b.is_ascii_alphanumeric());
                (!kept).then(|| (value.to_owned(), 1 + name.len()))
            })
        };
        match reference {
            Some((value, used)) => {
                push(&value, i, &mut decoded);
                i += used;
            }
            None => {
                let mut buffer = [0; 4];
                push(c.encode_utf8(&mut buffer), i, &mut decoded);
                i += c.len_utf8();
            }
        }
    }
    (decoded, offsets)
}

/// What the check binds against: the plan and the parameter panel's controls.
pub struct Targets<'a> {
    /// The plan.
    pub plan: &'a ExecutionPlan,
    /// The project's controls (top-level parameters).
    pub panel: &'a Panel,
}

/// How an address resolves.
enum Found {
    /// A control or plan parameter, with its range; `control` when
    /// `commit` can write it (a top-level parameter).
    Parameter { min: f64, max: f64, control: bool },
    /// A port of the plan.
    Port {
        direction: Direction,
        shape: PortShape,
        sink: bool,
    },
}

impl Targets<'_> {
    fn find(&self, port: &PortAddress) -> Option<Found> {
        let instance = port.instance();
        if let Some(name) = instance.as_str().strip_prefix('/')
            && !name.contains('/')
            && let Some(target) = self.panel.target(name, port.port())
        {
            return Some(Found::Parameter {
                min: target.min,
                max: target.max,
                control: true,
            });
        }
        let planned = self.plan.instances.iter().find(|i| i.address == instance)?;
        let descriptor = &planned.descriptor;
        if let Some(p) = descriptor.parameters.iter().find(|p| p.name == port.port()) {
            return Some(Found::Parameter {
                min: p.range.min,
                max: p.range.max,
                control: false,
            });
        }
        descriptor
            .ports
            .iter()
            .find(|p| p.name == port.port())
            .map(|p| Found::Port {
                direction: p.direction,
                shape: p.shape,
                sink: descriptor.sink == Some(SinkKind::Audio),
            })
    }

    /// Why `port` names nothing: what its instance is and has.
    fn unknown(&self, port: &PortAddress) -> String {
        let instance = port.instance();
        let text = instance.as_str();
        let top = text.strip_prefix('/').filter(|n| !n.contains('/'));
        let mut names: Vec<String> = Vec::new();
        if let Some(view) = top.and_then(|name| {
            self.panel
                .view(&Default::default())
                .into_iter()
                .find(|v| v.name == name)
        }) {
            names.extend(view.parameters.into_iter().map(|p| p.name));
        }
        if let Some(planned) = self.plan.instances.iter().find(|i| i.address == instance) {
            let descriptor = &planned.descriptor;
            for name in descriptor
                .ports
                .iter()
                .map(|p| p.name)
                .chain(descriptor.parameters.iter().map(|p| p.name))
            {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
        }
        if !names.is_empty() {
            return format!("unknown port {port}: {text} has {}", names.join(", "));
        }
        let prefix = format!("{text}/");
        if self
            .plan
            .instances
            .iter()
            .any(|i| i.address.as_str().starts_with(&prefix))
        {
            return format!(
                "unknown port {port}: {text} is a composite; bind one of its parameters, or a \
                 port of a primitive inside it ({text}/…)"
            );
        }
        format!("unknown port {port}: the plan has no instance {text}")
    }
}

fn describe_port(direction: Direction, shape: PortShape, sink: bool) -> String {
    let shape = match shape {
        PortShape::Signal => "signal",
        PortShape::Events => "event",
        PortShape::Vector(_) => "vector",
        _ => "other",
    };
    let direction = match direction {
        Direction::In => "input",
        Direction::Out => "output",
    };
    let sink = if sink { " of the audio sink" } else { "" };
    format!("a {shape} {direction}{sink}")
}

/// Checks the panel `text` of `file` (relative to the project folder)
/// against `targets`.
pub fn check(file: &str, text: &str, targets: &Targets<'_>) -> Checked {
    let mut findings = Vec::new();
    let mut controls = 0;
    let mut bindings = 0;
    let mut panel = false;
    let at = |offset: usize, severity, message: String| {
        Diagnostic::at(severity, file, text, offset, message)
    };
    for element in elements(text) {
        if element.name == PANEL_ELEMENT {
            panel = true;
            continue;
        }
        let index = controls;
        controls += 1;
        let mut finding = |offset: usize, severity, message: String| {
            findings.push(Finding {
                control: Some(index),
                diagnostic: at(offset, severity, message),
            });
        };
        let Some((wants, least, most)) = control(&element.name) else {
            finding(
                element.offset,
                Severity::Error,
                format!(
                    "unknown control <{}>; the controls are {CONTROLS}",
                    element.name
                ),
            );
            continue;
        };
        let name = &element.name;
        // The Setup's input has no port: `source="input"` meters it by channel.
        if wants == Wants::SinkInput
            && element
                .attribute("source")
                .and_then(|a| a.value.as_ref())
                .is_some_and(|(v, _)| v == "input")
        {
            continue;
        }
        let Some(bind) = element.attribute("bind") else {
            let severity = if wants == Wants::Controls {
                Severity::Warning
            } else {
                Severity::Error
            };
            let message = if wants == Wants::Controls {
                format!("<{name}> has no bind: it edits only its own value, nothing in the project")
            } else {
                format!("<{name}> has no bind: it needs {}", wants.text())
            };
            finding(element.offset, severity, message);
            continue;
        };
        let addresses = element.addresses();
        let value_at = bind.value.as_ref().map_or(element.offset, |(_, at)| *at);
        if addresses.len() < least || addresses.len() > most {
            let wanted = match (least, most) {
                (1, 1) => "one address".to_owned(),
                (2, 2) => "two addresses (x and y)".to_owned(),
                (l, m) => format!("{l} to {m} addresses"),
            };
            finding(
                value_at,
                Severity::Error,
                format!("<{name}> binds {wanted}; bind has {}", addresses.len()),
            );
            continue;
        }
        let mut ranges = Vec::new();
        for (address, offset) in addresses {
            bindings += 1;
            let port: PortAddress = match address.parse() {
                Ok(port) => port,
                Err(error) => {
                    finding(
                        offset,
                        Severity::Error,
                        format!("'{address}' is not a port address: {error}"),
                    );
                    continue;
                }
            };
            let wrong = |what: String| {
                format!(
                    "wrong kind: {port} is {what}; <{name}> binds {}",
                    wants.text()
                )
            };
            match (targets.find(&port), wants) {
                (None, _) => finding(offset, Severity::Error, targets.unknown(&port)),
                (
                    Some(Found::Parameter { min, max, control }),
                    Wants::Controls | Wants::Parameters,
                ) if control || wants == Wants::Parameters => {
                    ranges.push((min, max));
                }
                (Some(Found::Parameter { .. }), Wants::Controls) => {
                    let composite = port
                        .instance()
                        .segments()
                        .next()
                        .map(|s| format!("/{s}"))
                        .unwrap_or_default();
                    finding(
                        offset,
                        Severity::Error,
                        format!(
                            "not editable: {port} is a parameter inside the composite \
                             {composite}, which `commit` cannot write; <{name}> binds a control \
                             (a top-level parameter, /<instance>.<parameter>): export it from \
                             the composite, or show it with <signal-value>"
                        ),
                    );
                }
                (Some(Found::Parameter { .. }), _) => {
                    finding(offset, Severity::Error, wrong("a parameter".to_owned()));
                }
                (
                    Some(Found::Port {
                        direction,
                        shape,
                        sink,
                    }),
                    wants,
                ) => {
                    let fits = match wants {
                        Wants::Controls | Wants::Parameters => false,
                        Wants::EventInput => {
                            direction == Direction::In && shape == PortShape::Events
                        }
                        Wants::SinkInput => {
                            direction == Direction::In && shape == PortShape::Signal && sink
                        }
                        Wants::SignalPorts => shape == PortShape::Signal,
                        Wants::VectorOutputs => {
                            direction == Direction::Out && matches!(shape, PortShape::Vector(_))
                        }
                    };
                    if !fits {
                        finding(
                            offset,
                            Severity::Error,
                            wrong(describe_port(direction, shape, sink)),
                        );
                    }
                }
            }
        }
        // A log taper needs 0 < min < max, else the control is linear.
        if let Some(taper) = element.attribute("taper")
            && taper.value.as_ref().is_some_and(|(v, _)| v == "log")
        {
            let number = |key: &str| {
                element
                    .attribute(key)
                    .and_then(|a| a.value.as_ref())
                    .and_then(|(v, _)| v.trim().parse::<f64>().ok())
            };
            for (min, max) in &ranges {
                let (min, max) = (number("min").unwrap_or(*min), number("max").unwrap_or(*max));
                if !(min > 0.0 && min < max) {
                    let offset = taper.value.as_ref().map_or(element.offset, |(_, at)| *at);
                    finding(
                        offset,
                        Severity::Warning,
                        format!(
                            "taper=\"log\" needs 0 < min < max (here {min} to {max}); \
                             the control is linear"
                        ),
                    );
                    break;
                }
            }
        }
    }
    if !panel {
        findings.push(Finding {
            control: None,
            diagnostic: at(
                0,
                Severity::Warning,
                format!(
                    "no <{PANEL_ELEMENT}>: nothing connects this page's controls to the project"
                ),
            ),
        });
    }
    Checked {
        file: file.to_owned(),
        controls,
        bindings,
        findings,
    }
}

/// The panel files of a project's `ui/` folder (`*.html`, not hidden), sorted.
pub fn panel_files(ui: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match fs::read_dir(ui) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name.ends_with(".html") && path.is_file() {
                    files.push(path);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    files.sort();
    Ok(files)
}

/// Checks every panel of the project in `dir` (loaded as `project`) against
/// its plan: `signal ui check`.
pub fn check_project(
    dir: &Path,
    project: &Project,
    registry: &Registry,
) -> Result<Vec<Checked>, String> {
    let plan = project
        .plan(registry)
        .map_err(|_| "the project does not plan".to_owned())?
        .plan;
    let panel = Panel::new(project, registry);
    let targets = Targets {
        plan: &plan,
        panel: &panel,
    };
    let ui = crate::project_ui(dir);
    let mut checked = Vec::new();
    for path in panel_files(&ui).map_err(|e| format!("{}: {e}", ui.display()))? {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        checked.push(check(&format!("ui/{name}"), &text, &targets));
    }
    Ok(checked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scanner_finds_signal_tags_and_their_attribute_offsets() {
        let text = "<!doctype html>\n<!-- <signal-knob bind=\"/x.y\"> -->\n<p>a < b</p>\n\
                    <script>const t = '<signal-fader bind=\"/no\">';</script>\n\
                    <SIGNAL-KNOB  bind = '/a.freq'  label=decay bipolar></signal-knob>\n\
                    <signal-xy bind=\"/a.freq, /b.freq\"/>";
        let found = elements(text);
        assert_eq!(found.len(), 2, "{found:?}");
        let knob = &found[0];
        assert_eq!(knob.name, "signal-knob");
        let (value, at) = knob.attribute("bind").unwrap().value.clone().unwrap();
        assert_eq!(value, "/a.freq");
        assert_eq!(&text[at..at + 7], "/a.freq");
        assert_eq!(
            knob.attribute("label").unwrap().value.as_ref().unwrap().0,
            "decay"
        );
        assert_eq!(knob.attribute("bipolar").unwrap().value, None);
        let xy = &found[1];
        let addresses = xy.addresses();
        assert_eq!(
            addresses.iter().map(|a| a.0).collect::<Vec<_>>(),
            ["/a.freq", "/b.freq"]
        );
        for (address, at) in addresses {
            assert_eq!(&text[at..at + address.len()], address);
        }
    }

    #[test]
    fn an_unclosed_tag_or_comment_ends_the_scan_without_panicking() {
        for text in [
            "<signal-knob bind=\"/a",
            "<!-- <signal-knob",
            "<signal-",
            "<",
            "<a b='",
        ] {
            let _ = elements(text);
        }
        assert_eq!(elements("<signal-knob bind=/a.b").len(), 1);
    }

    /// auditor-1-004: a bind is read as the browser decodes it; each address
    /// keeps its position in the source.
    #[test]
    fn character_references_are_decoded_and_addresses_keep_their_source_offsets() {
        let text = "<signal-xy bind=\"&#x2F;a&period;freq&Tab;/b&#46freq &bogus;\">";
        let xy = &elements(text)[0];
        let (value, _) = xy.attribute("bind").unwrap().value.clone().unwrap();
        assert_eq!(value, "/a.freq\t/b.freq &bogus;");
        let addresses = xy.addresses();
        assert_eq!(
            addresses.iter().map(|a| a.0).collect::<Vec<_>>(),
            ["/a.freq", "/b.freq", "&bogus;"]
        );
        assert_eq!(
            &text[addresses[0].1..],
            &text[text.find("&#x2F;").unwrap()..]
        );
        assert_eq!(&text[addresses[1].1..addresses[1].1 + 2], "/b");
        assert_eq!(&text[addresses[2].1..addresses[2].1 + 7], "&bogus;");
        assert_eq!(decode("&#0;&#x110000;", 0).0, "\u{fffd}\u{fffd}");
    }

    /// The whole WHATWG table, and its attribute-value rule for legacy names
    /// without `;`, as a browser applies them.
    #[test]
    fn named_references_follow_the_html_table_and_attribute_rules() {
        let decoded = |raw: &str| decode(raw, 0).0;
        // auditor-1-004's reproducer, and another name no whitelist had.
        assert_eq!(decoded("/filter.cutoff&UnderBar;hz"), "/filter.cutoff_hz");
        assert_eq!(decoded("&sol;out&period;gain&excl;&fjlig;"), "/out.gain!fj");
        // A legacy name without `;`: decoded at the end or before another
        // character, left as written before `=` or an ASCII letter or digit.
        assert_eq!(decoded("a&amp b"), "a& b");
        assert_eq!(decoded("&amp"), "&");
        assert_eq!(decoded("&ampx"), "&ampx");
        assert_eq!(decoded("&amp1"), "&amp1");
        assert_eq!(decoded("&amp="), "&amp=");
        assert_eq!(decoded("&notit;"), "&notit;");
        assert_eq!(decoded("&noti"), "&noti");
        assert_eq!(decoded("&not-"), "¬-");
        // A name with `;` is decoded whatever follows; the longest name wins.
        assert_eq!(decoded("&amp;x"), "&x");
        assert_eq!(decoded("&notin;"), "∉");
        // Not a name in the table: kept.
        assert_eq!(decoded("&UnderBarx;"), "&UnderBarx;");
        // Numeric references in 0x80–0x9F are read as windows-1252.
        assert_eq!(decoded("&#x80;&#150;"), "€–");
        // The table is sorted, for the binary search.
        assert!(
            ENTITIES
                .windows(2)
                .all(|w| w[0].0.as_bytes() < w[1].0.as_bytes())
        );
        assert_eq!(ENTITIES.len(), 2231);
        // The auditor's binding passes the check's parse: positions stay the source's.
        let text = "<signal-xy bind=\"/filter.cutoff&UnderBar;hz /filter.q\">";
        let found = elements(text);
        let addresses = found[0].addresses();
        assert_eq!(addresses[0].0, "/filter.cutoff_hz");
        assert_eq!(addresses[1].0, "/filter.q");
        assert_eq!(&text[addresses[1].1..addresses[1].1 + 9], "/filter.q");
    }
}
