//! The tools (ADR 0029 §2; `docs/agent-interface.md` "Tools") and the
//! resources. Each tool has a JSON Schema, a plain description, a
//! structured result and a short text summary. Those that observe or change
//! the running project send UI-protocol messages (`docs/ui-protocol.md`);
//! `render`, `check`, `ui_check`, `library` and `replay` run the same code as
//! `signal render`, `signal ui check`, `signal modules` and `signal replay`,
//! reading the project's files and writing only under `--out`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use signal_project::patch::{self, Op};
use signal_project::vendor;
use signal_project::{Library, Project, SourceFile, located, read_sources};
use signal_ui::bindings::{MAIN_PANEL, check_project};
use signal_ui::panel_write::sha256_hex;
use signal_ui::project_ui;

use super::Context;
use super::client::Frame;
use super::gain::{self, Gains, Live};
use crate::render::{RenderArgs, render_project};
use crate::replay::{ReplayArgs, replay_session};

/// What a tool's group allows under `--read-only`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Inspect,
    Adjust,
    Patch,
    Text,
    Observe,
    Render,
    Check,
    Session,
}

impl Group {
    fn read_only(self) -> bool {
        matches!(self, Group::Inspect | Group::Observe | Group::Render)
    }
}

/// A tool's answer: its structured result and text summary, or why it was
/// refused (with the server's error, a conflict's current text, …).
type Outcome = Result<(Value, String), Refused>;

struct Refused {
    message: String,
    detail: Value,
}

fn refused(message: impl Into<String>) -> Refused {
    Refused {
        message: message.into(),
        detail: Value::Null,
    }
}

/// A UI-protocol error as a refusal: its code and message, the whole error
/// as detail (a `conflict`'s current `base` and `text`, `diagnostics`, …).
fn protocol(error: Value) -> Refused {
    let code = error["code"].as_str().unwrap_or("error");
    let message = error["message"].as_str().unwrap_or("");
    Refused {
        message: format!("{code}: {message}"),
        detail: error,
    }
}

struct Tool {
    name: &'static str,
    group: Group,
    description: &'static str,
    schema: fn() -> Value,
    run: fn(&Context, &Value) -> Outcome,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn none() -> Value {
    json!({"type": "object", "additionalProperties": false})
}

const TOOLS: &[Tool] = &[
    // Inspect.
    Tool {
        name: "project_info",
        group: Group::Inspect,
        description: "What is playing: the project's name, folder, sample rate, frame and plan version, its controls (top-level parameters with ranges and live values), how this server runs (in process or attached, driver, --max-gain, --read-only), each file's SHA-256 (the `base` a patch or write names), and each std.audio_out gain with whether it is modulated or above the ceiling.",
        schema: none,
        run: project_info,
    },
    Tool {
        name: "graph",
        group: Group::Inspect,
        description: "The project's structure as written, not flattened: instances with their ports and parameters, connections with their scales and converters, every composite's inside, the schematic positions, and `files` (each file's SHA-256, the `base` of a patch). `level`: only the top level (\"\") or one composite.",
        schema: || {
            object(
                json!({"level": {"type": "string", "description": "\"\" for the top level, else a composite's name"}}),
                &[],
            )
        },
        run: graph,
    },
    Tool {
        name: "modules",
        group: Group::Inspect,
        description: "The module types an instance can be added from: std primitives (with their formula, ports, parameters and ranges, category), the project's composites and the built-in library's (`lib.*`). `category` or `query` (a substring of the name or description) narrows the list.",
        schema: || {
            object(
                json!({"category": {"type": "string"}, "query": {"type": "string"}}),
                &[],
            )
        },
        run: modules,
    },
    Tool {
        name: "describe_instance",
        group: Group::Inspect,
        description: "One instance of the running project (address like `/osc` or `/drums/kick`): every primitive at or inside it with its ports and parameters (kind, unit, range, default, base), its controls, and its entry in the graph.",
        schema: || object(json!({"address": {"type": "string"}}), &["address"]),
        run: describe_instance,
    },
    Tool {
        name: "read_file",
        group: Group::Inspect,
        description: "A project text file and its SHA-256 (`base`): `project.toml`, `composites/<name>.toml`, `patterns/<name>.toml`, `ui/<name>.html` or `ui/schematic.toml`.",
        schema: || object(json!({"file": {"type": "string"}}), &["file"]),
        run: read_file,
    },
    Tool {
        name: "stats",
        group: Group::Inspect,
        description: "The server's counters: the frame, each client's dropped frames, evictions, meter/scope/vector/command overflows, clipped samples, writes.",
        schema: none,
        run: stats,
    },
    // Play and adjust.
    Tool {
        name: "set_parameter",
        group: Group::Adjust,
        description: "Sets a parameter live, as a panel knob does (clamped to its range): a control (`/out.gain`, `/drums.kick_decay`) or a plan parameter port. Not written to the file until `commit`. Answers once applied, with the effective frame.",
        schema: || {
            object(
                json!({"address": {"type": "string"}, "value": {"type": "number"}}),
                &["address", "value"],
            )
        },
        run: set_parameter,
    },
    Tool {
        name: "ramp_parameter",
        group: Group::Adjust,
        description: "Ramps a parameter live to `to` over `ms` milliseconds (or `frames`; default 5 ms), clamped to its range.",
        schema: || {
            object(
                json!({"address": {"type": "string"}, "to": {"type": "number"}, "ms": {"type": "number", "minimum": 0}, "frames": {"type": "integer", "minimum": 0}}),
                &["address", "to"],
            )
        },
        run: ramp_parameter,
    },
    Tool {
        name: "trigger",
        group: Group::Adjust,
        description: "Sends a trigger to an event input of the running plan (`/env.trigger`).",
        schema: || object(json!({"address": {"type": "string"}}), &["address"]),
        run: trigger,
    },
    Tool {
        name: "transport",
        group: Group::Adjust,
        description: "`stop` stops the run; `start` plays the project again from frame 0 from its files; `tempo` sets (or with `ms`, ramps) the project's tempo: every top-level control that reaches the `bpm` of a std.transport or std.playhead (lib.transport's `/transport.bpm`) and that no connection drives, all of them together.",
        schema: || {
            object(
                json!({"action": {"type": "string", "enum": ["start", "stop", "tempo"]}, "bpm": {"type": "number"}, "ms": {"type": "number", "minimum": 0}}),
                &["action"],
            )
        },
        run: transport,
    },
    Tool {
        name: "commit",
        group: Group::Adjust,
        description: "Writes a control's value into project.toml (only that key changes; atomic), as the panel's commit does: `value` (default: its live value), or null to remove the key (back to the default).",
        schema: || {
            object(
                json!({"address": {"type": "string"}, "value": {"type": ["number", "null"]}}),
                &["address"],
            )
        },
        run: commit,
    },
    Tool {
        name: "revert",
        group: Group::Adjust,
        description: "Drops a parameter's unsaved live value: each of its ports gets the file's value again.",
        schema: || object(json!({"address": {"type": "string"}}), &["address"]),
        run: revert,
    },
    // Edit the graph.
    Tool {
        name: "patch",
        group: Group::Patch,
        description: "Edits one level's file as text, as the schematic does, applied live by the reload: ops `instance.add` {name, module, params?, x?, y?}, `instance.remove` {name}, `connection.add` {from, to, scale?} (\"inst.port\", no leading /), `connection.remove` {from, to}, `connection.scale` {from, to, scale}. `level`: \"\" (project.toml, default) or a composite. `base`: the file's SHA-256 from graph.files (default: the current one). The reply's `inverse` undoes it.",
        schema: || {
            object(
                json!({"level": {"type": "string"}, "base": {"type": "string"}, "ops": {"type": "array", "minItems": 1, "items": {"type": "object"}}}),
                &["ops"],
            )
        },
        run: patch_tool,
    },
    Tool {
        name: "schematic_move",
        group: Group::Patch,
        description: "Moves instances in the schematic (ui/schematic.toml, presentation only): `instance`, `x`, `y`, or `moves` [{instance, x, y}], at `level` (\"\" default).",
        schema: || {
            object(
                json!({"level": {"type": "string"}, "instance": {"type": "string"}, "x": {"type": "number"}, "y": {"type": "number"}, "moves": {"type": "array", "items": {"type": "object"}}}),
                &[],
            )
        },
        run: schematic_move,
    },
    Tool {
        name: "vendor",
        group: Group::Patch,
        description: "Copies a built-in library composite (`lib.<name>`, see `library`) and the library composites it uses into the project's composites/ as its own text, and points the project's references at the copies. `as`: the copy's name.",
        schema: || {
            object(
                json!({"composite": {"type": "string"}, "as": {"type": "string"}}),
                &["composite"],
            )
        },
        run: vendor_tool,
    },
    // Edit text.
    Tool {
        name: "write_pattern",
        group: Group::Text,
        description: "Replaces patterns/<name>.toml (a notes or steps pattern, project-format.md §12) with `text`; applied live, a playing loop keeping its position. `base`: the SHA-256 it was read with (read_file), or null for a new pattern; a `conflict` returns the current text.",
        schema: || {
            object(
                json!({"name": {"type": "string"}, "text": {"type": "string"}, "base": {"type": ["string", "null"]}}),
                &["name", "text", "base"],
            )
        },
        run: write_pattern,
    },
    Tool {
        name: "write_panel",
        group: Group::Text,
        description: "Replaces the content of the one <signal-panel> element of ui/<file> (default main.html) with `html` (the panel whitelist: controls, cards, rows, headings, text); every byte outside it is kept. `base`: the file's SHA-256, or null for a main.html that does not exist yet.",
        schema: || {
            object(
                json!({"file": {"type": "string"}, "html": {"type": "string"}, "base": {"type": ["string", "null"]}}),
                &["html", "base"],
            )
        },
        run: write_panel,
    },
    Tool {
        name: "write_file",
        group: Group::Text,
        description: "Replaces a project file with `text`: project.toml, composites/<name>.toml (null `base` creates one) or patterns/<name>.toml. Checked as a reload would load it, written atomically, applied live. `base`: the SHA-256 it was read with (read_file); a `conflict` returns the current text.",
        schema: || {
            object(
                json!({"file": {"type": "string"}, "text": {"type": "string"}, "base": {"type": ["string", "null"]}}),
                &["file", "text", "base"],
            )
        },
        run: write_file,
    },
    // Observe.
    Tool {
        name: "meter",
        group: Group::Observe,
        description: "Levels over `window_ms` (default 500): the audio sink's output channels (`source`: output, default, or input), per channel peak and RMS, linear and dBFS; or with `ports`, signal outputs read through scope streams at 4800 points/s (a decimated capture: its peak can read below the true sample peak; window at least 54 ms).",
        schema: || {
            object(
                json!({"source": {"type": "string", "enum": ["output", "input"]}, "ports": {"type": "array", "items": {"type": "string"}, "maxItems": 8}, "window_ms": {"type": "number", "minimum": 1, "maximum": 10000}}),
                &[],
            )
        },
        run: meter,
    },
    Tool {
        name: "scope",
        group: Group::Observe,
        description: "A window of a signal output (`/osc.out`) or input (`/out.l`) as numbers: `rate` points per second (default 4800) over `window_ms` (default 200; at least 256/rate s, a stream's block), returned downsampled to `points` (default 256, at most 1024), with peak and RMS.",
        schema: || {
            object(
                json!({"port": {"type": "string"}, "rate": {"type": "number", "minimum": 1}, "window_ms": {"type": "number", "minimum": 1, "maximum": 10000}, "points": {"type": "integer", "minimum": 1, "maximum": 1024}}),
                &["port"],
            )
        },
        run: scope,
    },
    Tool {
        name: "read_stream",
        group: Group::Observe,
        description: "The sequencers' playheads: every std.step_seq8 `step` (0..255) and std.note_seq `position` (beats) output, or the signal outputs in `ports`, read over `window_ms` (default 1000; at least 256/rate s) at `rate` points per second (default 512): first, last, minimum and maximum, with the frame and each std.transport's and std.playhead's bpm.",
        schema: || {
            object(
                json!({"ports": {"type": "array", "items": {"type": "string"}, "maxItems": 8}, "rate": {"type": "number", "minimum": 1}, "window_ms": {"type": "number", "minimum": 1, "maximum": 10000}}),
                &[],
            )
        },
        run: read_stream,
    },
    Tool {
        name: "render",
        group: Group::Render,
        description: "Renders the project's files as they are now, offline (never played): `seconds` (default 2, at most 600). Returns the metrics, the manifest's hashes and the path of audio.wav, written under the server's --out folder. Unsaved live values are not in the files, so not in the render.",
        schema: || {
            object(
                json!({"seconds": {"type": "number", "minimum": 0, "maximum": 600}, "seed": {"type": "integer", "minimum": 0}}),
                &[],
            )
        },
        run: render,
    },
    // Checks and library.
    Tool {
        name: "ui_check",
        group: Group::Check,
        description: "Checks the project's panels (ui/*.html): every bind names a port of the plan whose kind and shape fit its control (`signal ui check`).",
        schema: none,
        run: ui_check,
    },
    Tool {
        name: "check",
        group: Group::Check,
        description: "Loads and plans the project's files as they are now: ok, or every diagnostic (file:line:column), and the plan's warnings.",
        schema: none,
        run: check,
    },
    Tool {
        name: "library",
        group: Group::Check,
        description: "The built-in library (`lib.*` composites): each with its category, description, content hash, ports and parameters, and the library's version and changelog.",
        schema: none,
        run: library,
    },
    // Sessions.
    Tool {
        name: "session_start",
        group: Group::Session,
        description: "Starts recording a session (docs/session-format.md): the project plays again from frame 0, recording every edit. Refused while parameters are unsaved, unless `discard` (their live values are lost).",
        schema: || object(json!({"discard": {"type": "boolean"}}), &[]),
        run: session_start,
    },
    Tool {
        name: "session_stop",
        group: Group::Session,
        description: "Ends the recorded run: the session and the audio it played are written under --out, and the project plays again (not recording).",
        schema: none,
        run: session_stop,
    },
    Tool {
        name: "replay",
        group: Group::Session,
        description: "Replays a recorded session offline (default: the last one) and checks it: `identical` is true when the replay succeeds and its audio hash equals the recorded run's.",
        schema: || object(json!({"session": {"type": "string"}}), &[]),
        run: replay,
    },
];

/// Whether `tool` is offered here: `--read-only` keeps the inspect, observe
/// and render tools; the session tools need the in-process host.
fn offered(context: &Context, tool: &Tool) -> bool {
    if context.read_only && !tool.group.read_only() {
        return false;
    }
    tool.group != Group::Session || lock(context).control.is_some()
}

fn lock<'c>(context: &'c Context) -> std::sync::MutexGuard<'c, super::Link> {
    context
        .link
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `tools/list`.
pub(crate) fn list(context: &Context) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|tool| offered(context, tool))
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.schema)(),
            })
        })
        .collect()
}

/// `tools/call`: `None` for a tool not offered here.
pub(crate) fn call(context: &Context, name: &str, arguments: &Value) -> Option<Value> {
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name && offered(context, tool))?;
    let outcome = match validate(&(tool.schema)(), arguments, "arguments") {
        Err(message) => Err(refused(format!("invalid arguments: {message}"))),
        Ok(()) => (tool.run)(context, arguments),
    };
    Some(match outcome {
        Ok((structured, summary)) => {
            let (structured, text) = capped(name, structured);
            let mut content = vec![json!({"type": "text", "text": summary})];
            if let Some(text) = text {
                content.push(json!({"type": "text", "text": text}));
            }
            json!({
                "content": content,
                "structuredContent": structured,
                "isError": false,
            })
        }
        Err(Refused { message, detail }) => {
            let mut result = json!({
                "content": [{"type": "text", "text": message}],
                "isError": true,
            });
            if !detail.is_null() {
                result["structuredContent"] = json!({"error": detail});
                if let Some(content) = result["content"].as_array_mut() {
                    content.push(json!({"type": "text", "text": detail.to_string()}));
                }
            }
            result
        }
    })
}

/// The most bytes of JSON a tool's `structuredContent` holds
/// (`docs/agent-interface.md` "Limits"): a larger result keeps its small
/// fields, and each large one is replaced by `{"omitted": true, "bytes",
/// "items"}`, with `truncated: true` and how to ask for less.
pub(crate) const MAX_RESULT: usize = 256 << 10;

/// The most bytes of JSON also given as text in `content`, after the
/// summary (for clients that read no `structuredContent`).
pub(crate) const MAX_RESULT_TEXT: usize = 32 << 10;

/// `structured` within [`MAX_RESULT`], and its JSON text when that is
/// within [`MAX_RESULT_TEXT`].
fn capped(tool: &str, structured: Value) -> (Value, Option<String>) {
    let text = structured.to_string();
    let bytes = text.len();
    let structured = if bytes <= MAX_RESULT {
        structured
    } else {
        let mut fields: Vec<(String, Value, usize)> = match structured {
            Value::Object(fields) => fields
                .into_iter()
                .map(|(key, value)| {
                    let size = value.to_string().len();
                    (key, value, size)
                })
                .collect(),
            other => vec![("result".to_owned(), other, bytes)],
        };
        // The smallest fields are kept first, until the cap.
        fields.sort_by_key(|(_, _, size)| *size);
        let mut kept = Map::new();
        let mut used = 512;
        for (key, value, size) in fields {
            if used + size + key.len() + 4 <= MAX_RESULT {
                used += size + key.len() + 4;
                kept.insert(key, value);
            } else {
                let items = match &value {
                    Value::Array(items) => items.len(),
                    Value::Object(fields) => fields.len(),
                    _ => 1,
                };
                kept.insert(key, json!({"omitted": true, "bytes": size, "items": items}));
            }
        }
        kept.insert("truncated".to_owned(), json!(true));
        kept.insert("bytes".to_owned(), json!(bytes));
        kept.insert("limit".to_owned(), json!(MAX_RESULT));
        kept.insert(
            "guidance".to_owned(),
            json!(match tool {
                "graph" => "the graph is larger than the limit: ask for one `level` at a time",
                "modules" => "narrow the list with `category` or `query`",
                "describe_instance" => "describe a narrower address (an instance inside it)",
                _ => "the result is larger than the limit: ask for less (read_file reads one file whole)",
            }),
        );
        Value::Object(kept)
    };
    let text = if bytes <= MAX_RESULT_TEXT {
        Some(text)
    } else {
        None
    };
    (structured, text)
}

/// Checks `value` against the subset of JSON Schema the tools use.
fn validate(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    if let Some(kinds) = schema.get("type") {
        let kinds: Vec<&str> = match kinds {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let fits = kinds.iter().any(|kind| match *kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "number" => value.as_f64().is_some_and(f64::is_finite),
            "integer" => value.is_u64() || value.is_i64(),
            _ => false,
        });
        if !fits {
            return Err(format!("{at} must be {}", kinds.join(" or ")));
        }
    }
    if let (Some(allowed), false) = (
        schema.get("enum").and_then(Value::as_array),
        value.is_null(),
    ) && !allowed.contains(value)
    {
        return Err(format!(
            "{at} must be one of {}",
            Value::Array(allowed.clone())
        ));
    }
    if let Some(number) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && number < min
        {
            return Err(format!("{at} must be at least {min}"));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && number > max
        {
            return Err(format!("{at} must be at most {max}"));
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            return Err(format!("{at} needs at least {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            return Err(format!("{at} takes at most {max} items"));
        }
        if let Some(item) = schema.get("items") {
            for (i, element) in items.iter().enumerate() {
                validate(item, element, &format!("{at}[{i}]"))?;
            }
        }
    }
    if let Some(fields) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !fields.contains_key(required) {
                return Err(format!("{at} needs `{required}`"));
            }
        }
        for (key, field) in fields {
            match properties.and_then(|p| p.get(key)) {
                Some(property) => validate(property, field, &format!("{at}.{key}"))?,
                None if schema.get("additionalProperties") == Some(&json!(false)) => {
                    return Err(format!("{at} has no field `{key}`"));
                }
                None => {}
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers.

/// A UI-protocol request on the current connection.
fn ask(context: &Context, message: Value) -> Result<Value, Refused> {
    lock(context).client.request(message).map_err(protocol)
}

/// The reply without its `type` and `id`.
fn bare(mut reply: Value) -> Value {
    if let Some(fields) = reply.as_object_mut() {
        fields.remove("type");
        fields.remove("id");
    }
    reply
}

fn text_arg<'v>(arguments: &'v Value, key: &str) -> &'v str {
    arguments.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `/name` for `name`: addresses start with a slash.
fn address(arguments: &Value) -> String {
    let address = text_arg(arguments, "address");
    if address.starts_with('/') {
        address.to_owned()
    } else {
        format!("/{address}")
    }
}

/// The instance of a port address (`/a/b.port` → `/a/b`).
fn instance_of(port: &str) -> String {
    port.rsplit_once('.')
        .map_or_else(|| port.to_owned(), |(i, _)| i.to_owned())
}

fn sources(context: &Context) -> Result<Vec<SourceFile>, Refused> {
    read_sources(&context.dir).map_err(|e| refused(e.to_string()))
}

// ---------------------------------------------------------------------------
// The gain ceiling: the state before a change, and the check.

/// The live bases of `gains`' ports the running plan has (a `parameters`
/// stream's first frame), and the running project, which must be the files'
/// (no reload pending).
fn live_gains(context: &Context, gains: &Gains) -> Result<BTreeMap<String, f64>, Refused> {
    let graph = ask(context, json!({"type": "graph"}))?;
    same_project(context, &graph)?;
    let files = graph.get("files").and_then(Value::as_object);
    for file in sources(context)?
        .iter()
        .filter(|s| s.path == "project.toml" || s.path.starts_with("composites/"))
    {
        let level = if file.path == "project.toml" {
            String::new()
        } else {
            file.path
                .trim_start_matches("composites/")
                .trim_end_matches(".toml")
                .to_owned()
        };
        let running = files.and_then(|f| f.get(&level)).and_then(Value::as_str);
        if running.is_some_and(|r| r != sha256_hex(file.text.as_bytes())) {
            return Err(refused(
                "the project is reloading (its files differ from what plays); try again in a moment",
            ));
        }
    }
    let describe = ask(context, json!({"type": "describe"}))?;
    let known: BTreeSet<&str> = describe["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|i| i["parameters"].as_array().into_iter().flatten())
        .filter_map(|p| p["address"].as_str())
        .collect();
    let addresses: Vec<&str> = gains.addresses().filter(|a| known.contains(a)).collect();
    if addresses.is_empty() || describe["playing"] == json!(false) {
        // Nothing plays: the bases are the plan's.
        return Ok(BTreeMap::new());
    }
    let reply = ask(
        context,
        json!({"type": "subscribe", "stream": "parameters", "addresses": addresses, "rate": 60}),
    )?;
    let sub = reply["sub"].as_u64().unwrap_or(0) as u32;
    let until = Instant::now() + Duration::from_secs(3);
    let mut found = None;
    while found.is_none() && Instant::now() < until {
        let frames = lock(context).client.frames(sub);
        found = frames.iter().rev().find_map(|frame| {
            let values = frame.f64s();
            let bases: Vec<f64> = values.chunks(2).map(|pair| pair[0]).collect();
            (bases.len() == addresses.len() && bases.iter().all(|b| b.is_finite())).then_some(bases)
        });
        thread::sleep(Duration::from_millis(5));
    }
    unsubscribe(context, sub);
    let bases = found.ok_or_else(|| refused("the live gain is not known yet; try again"))?;
    Ok(addresses
        .into_iter()
        .map(str::to_owned)
        .zip(bases)
        .collect())
}

/// Refuses when the server no longer plays the folder this server names
/// (`graph.projectId`, the SHA-256 of its canonical path): attached, the
/// person can open another project in Signal.app, and every check here
/// reads the folder's files.
fn same_project(context: &Context, graph: &Value) -> Result<(), Refused> {
    if plays(&context.dir, graph) {
        Ok(())
    } else {
        Err(refused(format!(
            "refused: the server no longer plays {} (another project was opened); restart `signal mcp` for the project it plays",
            context.dir.display()
        )))
    }
}

/// Whether `graph` (a UI-protocol `graph` reply) is of the folder `dir`.
pub(crate) fn plays(dir: &Path, graph: &Value) -> bool {
    let id = sha256_hex(dir.as_os_str().as_encoded_bytes());
    graph.get("projectId").and_then(Value::as_str) == Some(id.as_str())
}

/// [`same_project`] for a change that no gain check covers (`write_panel`,
/// `schematic_move`, `transport`): attached only, since in process the
/// host plays this folder alone.
fn still_same_project(context: &Context) -> Result<(), Refused> {
    if context.mode != "attached" {
        return Ok(());
    }
    let graph = ask(context, json!({"type": "graph"}))?;
    same_project(context, &graph)
}

/// A refusal because a project does not load and plan: code
/// `does-not-load`, as the UI protocol's, with its diagnostics.
fn does_not_load(message: String, why: &str) -> Refused {
    Refused {
        message,
        detail: json!({"code": "does-not-load", "diagnostics": why.lines().collect::<Vec<_>>()}),
    }
}

/// The gain ports of the project's files as they are now; refused when
/// they do not load and plan: no change is judged then (fail closed).
fn current_gains(context: &Context) -> Result<Gains, Refused> {
    Gains::of(sources(context)?, Some(&context.dir), context.registry).map_err(|why| {
        does_not_load(
            format!(
                "refused: the project's files do not load and plan as they are now, so no change can be checked against --max-gain (fix the files first; `check` shows why):\n{why}"
            ),
            &why,
        )
    })
}

fn unsubscribe(context: &Context, sub: u32) {
    let link = lock(context);
    link.client
        .send(&json!({"type": "unsubscribe", "sub": sub}));
    link.client.forget(sub);
}

/// Checks a change of live values (`set`, `ramp`, `commit`, `revert`,
/// `trigger`): `ports` reached, each with the base it gets (`None`: an
/// event, or a base that does not change).
fn guard_live(context: &Context, ports: &[(String, Option<f64>)]) -> Result<(), Refused> {
    let gains = current_gains(context)?;
    let live_before = live_gains(context, &gains)?;
    let mut live = Live::default();
    for (port, value) in ports {
        match value {
            Some(value) if gains.is_gain(port) => {
                live.bases.insert(port.clone(), *value);
            }
            _ => {
                live.instances.insert(instance_of(port));
            }
        }
    }
    gain::check(context.max_gain, Some(&gains), &gains, &live_before, &live).map_err(refused)
}

/// Checks an edit of the files: `after`, the project's files as it would
/// leave them.
fn guard_files(context: &Context, after: Vec<SourceFile>) -> Result<(), Refused> {
    let before = current_gains(context)?;
    let after = Gains::of(after, Some(&context.dir), context.registry).map_err(|why| {
        does_not_load(
            format!(
                "does-not-load: the project would not load and plan with this change, so nothing is written:\n{why}"
            ),
            &why,
        )
    })?;
    let live_before = live_gains(context, &before)?;
    gain::check(
        context.max_gain,
        Some(&before),
        &after,
        &live_before,
        &Live::default(),
    )
    .map_err(refused)
}

/// Checks a restart from the files (`transport` start, `session_start`):
/// every unsaved parameter goes back to the file's value, so each gain port
/// among them gets its file value (a lowered live gain is raised, as a
/// `revert` would raise it) and every other port's instance changes.
fn guard_restart(context: &Context) -> Result<(), Refused> {
    let gains = current_gains(context)?;
    let describe = ask(context, json!({"type": "describe"}))?;
    let mut ports = Vec::new();
    for address in describe["unsaved"]["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["address"].as_str())
    {
        for port in ports_of(context, address)? {
            let file = gains.file_value(&port);
            ports.push((port, file));
        }
    }
    guard_live(context, &ports)
}

/// The project's files with `path` holding `text`.
fn replaced(mut files: Vec<SourceFile>, path: &str, text: &str) -> Vec<SourceFile> {
    files.retain(|s| s.path != path);
    files.push(SourceFile {
        path: path.to_owned(),
        text: text.to_owned(),
    });
    files
}

/// The ports a control or plan parameter address reaches (`describe`).
fn ports_of(context: &Context, address: &str) -> Result<Vec<String>, Refused> {
    let describe = ask(context, json!({"type": "describe"}))?;
    let control = describe["controls"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["address"] == json!(address));
    Ok(match control {
        Some(control) => control["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => vec![address.to_owned()],
    })
}

// ---------------------------------------------------------------------------
// Inspect.

fn project_info(context: &Context, _: &Value) -> Outcome {
    let describe = ask(context, json!({"type": "describe"}))?;
    let graph = ask(context, json!({"type": "graph"}))?;
    let pong = ask(context, json!({"type": "ping"}))?;
    let gains = Gains::of(sources(context)?, Some(&context.dir), context.registry).ok();
    let live = match &gains {
        Some(gains) => live_gains(context, gains).unwrap_or_default(),
        None => BTreeMap::new(),
    };
    let instances = describe["instances"].as_array().map_or(0, Vec::len);
    let project = describe["project"].clone();
    let playing = describe["playing"] != json!(false);
    // ADR 0040: whether the folder's file system gives the writer lock now
    // (null when the folder cannot be opened).
    let writer_lock = signal_project::transaction::locks_work(&context.dir).ok();
    let structured = json!({
        "project": project,
        "dir": context.dir.display().to_string(),
        "sampleRate": describe["sampleRate"],
        "playing": playing,
        "frame": pong["frame"],
        "planVersion": describe["planVersion"],
        "mode": context.mode,
        "driver": context.driver,
        "maxGain": context.max_gain,
        "outputCeiling": context.ceiling,
        "readOnly": context.read_only,
        "writerLock": writer_lock,
        "out": context.out.display().to_string(),
        "files": graph["files"],
        "instances": instances,
        "controls": describe["controls"],
        "sinks": describe["sinks"],
        "unsaved": describe["unsaved"],
        "gain": gains.map(|g| g.report(&live, context.max_gain)).unwrap_or_default(),
    });
    let summary = format!(
        "{} at {} Hz, {}; {} primitive instances; {} ({} driver); max gain {}{}{}",
        project.as_str().unwrap_or("?"),
        describe["sampleRate"],
        if playing { "playing" } else { "stopped" },
        instances,
        context.mode,
        context.driver,
        context.max_gain,
        if context.read_only { "; read-only" } else { "" },
        if writer_lock == Some(false) {
            "; no writer lock on this volume now: writes go on unlocked (ADR 0040)"
        } else {
            ""
        }
    );
    Ok((structured, summary))
}

fn graph(context: &Context, arguments: &Value) -> Outcome {
    let graph = bare(ask(context, json!({"type": "graph"}))?);
    let Some(level) = arguments.get("level").and_then(Value::as_str) else {
        let summary = format!(
            "{}: {} top-level instances, {} connections, {} composites",
            graph["project"].as_str().unwrap_or("?"),
            graph["top"]["instances"].as_array().map_or(0, Vec::len),
            graph["top"]["connections"].as_array().map_or(0, Vec::len),
            graph["composites"].as_object().map_or(0, Map::len)
        );
        return Ok((graph, summary));
    };
    let inside = if level.is_empty() {
        graph["top"].clone()
    } else {
        graph["composites"][level].clone()
    };
    if inside.is_null() {
        return Err(refused(format!("the project has no composite `{level}`")));
    }
    let summary = format!(
        "level `{level}`: {} instances, {} connections",
        inside["instances"].as_array().map_or(0, Vec::len),
        inside["connections"].as_array().map_or(0, Vec::len)
    );
    Ok((
        json!({
            "level": level,
            "planVersion": graph["planVersion"],
            "files": graph["files"],
            "inside": inside,
            "positions": graph["layout"]["positions"][level],
        }),
        summary,
    ))
}

fn modules(context: &Context, arguments: &Value) -> Outcome {
    let reply = ask(context, json!({"type": "modules"}))?;
    let category = arguments.get("category").and_then(Value::as_str);
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let keep = |entry: &&Value| {
        category.is_none_or(|c| entry["category"].as_str() == Some(c))
            && query.as_deref().is_none_or(|q| {
                entry["module"]
                    .as_str()
                    .is_some_and(|m| m.to_lowercase().contains(q))
                    || entry["description"]
                        .as_str()
                        .is_some_and(|d| d.to_lowercase().contains(q))
            })
    };
    let pick = |key: &str| -> Vec<Value> {
        reply[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter(keep)
            .cloned()
            .collect()
    };
    let (primitives, composites) = (pick("primitives"), pick("composites"));
    let summary = format!(
        "{} primitives, {} composites",
        primitives.len(),
        composites.len()
    );
    Ok((
        json!({"primitives": primitives, "composites": composites}),
        summary,
    ))
}

fn describe_instance(context: &Context, arguments: &Value) -> Outcome {
    let address = address(arguments);
    let describe = ask(context, json!({"type": "describe"}))?;
    let inside = |a: &str| a == address || a.starts_with(&format!("{address}/"));
    let instances: Vec<Value> = describe["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["address"].as_str().is_some_and(inside))
        .cloned()
        .collect();
    let controls: Vec<Value> = describe["controls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| {
            c["address"]
                .as_str()
                .is_some_and(|a| instance_of(a) == address)
        })
        .cloned()
        .collect();
    if instances.is_empty() {
        return Err(refused(format!(
            "the running project has no instance {address}"
        )));
    }
    let graph = ask(context, json!({"type": "graph"}))?;
    let entry = graph["top"]["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["address"] == json!(address))
        .cloned()
        .unwrap_or(Value::Null);
    let summary = format!(
        "{address}: {} primitive instances, {} controls",
        instances.len(),
        controls.len()
    );
    Ok((
        json!({"address": address, "graph": entry, "instances": instances, "controls": controls}),
        summary,
    ))
}

fn read_file(context: &Context, arguments: &Value) -> Outcome {
    let file = text_arg(arguments, "file");
    let reply = if let Some(name) = file
        .strip_prefix("patterns/")
        .and_then(|f| f.strip_suffix(".toml"))
    {
        ask(context, json!({"type": "pattern.read", "name": name}))?
    } else if file == "ui/schematic.toml" {
        let path = context.dir.join(file);
        let linked = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
        if linked(&context.dir.join("ui")) || linked(&path) {
            return Err(refused(format!("{file} is reached through a link")));
        }
        match fs::read_to_string(&path) {
            Ok(text) => {
                json!({"file": file, "exists": true, "base": sha256_hex(text.as_bytes()), "text": text})
            }
            Err(_) => json!({"file": file, "exists": false, "base": null, "text": null}),
        }
    } else if let Some(panel) = file.strip_prefix("ui/") {
        ask(context, json!({"type": "panel.read", "file": panel}))?
    } else {
        ask(context, json!({"type": "file.read", "file": file}))?
    };
    let reply = bare(reply);
    let summary = if reply["exists"] == json!(true) {
        format!(
            "{file}: {} bytes, base {}",
            reply["text"].as_str().map_or(0, str::len),
            reply["base"].as_str().unwrap_or("")
        )
    } else {
        format!("{file} does not exist")
    };
    Ok((reply, summary))
}

fn stats(context: &Context, _: &Value) -> Outcome {
    let reply = bare(ask(context, json!({"type": "stats"}))?);
    let summary = format!(
        "frame {}, {} clients, clipped {}",
        reply["frame"],
        reply["clients"].as_array().map_or(0, Vec::len),
        reply["clipped"]
    );
    Ok((reply, summary))
}

// ---------------------------------------------------------------------------
// Play and adjust.

fn acked(reply: Value, what: String) -> Outcome {
    let summary = format!(
        "{what}: {} at frame {}",
        reply["outcome"].as_str().unwrap_or("?"),
        reply["frame"]
    );
    Ok((bare(reply), summary))
}

fn set_parameter(context: &Context, arguments: &Value) -> Outcome {
    let address = address(arguments);
    let value = arguments["value"].as_f64().unwrap_or(0.0);
    let ports = ports_of(context, &address)?;
    guard_live(
        context,
        &ports
            .into_iter()
            .map(|p| (p, Some(value)))
            .collect::<Vec<_>>(),
    )?;
    let reply = ask(
        context,
        json!({"type": "set", "address": address, "value": value}),
    )?;
    acked(reply, format!("set {address} to {value}"))
}

fn ramp_parameter(context: &Context, arguments: &Value) -> Outcome {
    let address = address(arguments);
    let to = arguments["to"].as_f64().unwrap_or(0.0);
    let ports = ports_of(context, &address)?;
    guard_live(
        context,
        &ports.into_iter().map(|p| (p, Some(to))).collect::<Vec<_>>(),
    )?;
    let mut message = json!({"type": "ramp", "address": address, "to": to});
    for key in ["ms", "frames"] {
        if let Some(value) = arguments.get(key) {
            message[key] = value.clone();
        }
    }
    let reply = ask(context, message)?;
    acked(reply, format!("ramp {address} to {to}"))
}

fn trigger(context: &Context, arguments: &Value) -> Outcome {
    let address = address(arguments);
    guard_live(context, &[(address.clone(), None)])?;
    let reply = ask(context, json!({"type": "trigger", "address": address}))?;
    acked(reply, format!("trigger {address}"))
}

fn transport(context: &Context, arguments: &Value) -> Outcome {
    let action = text_arg(arguments, "action");
    still_same_project(context)?;
    if action == "start" {
        guard_restart(context)?;
    }
    if action != "tempo" {
        let kind = if action == "start" {
            "transport.play"
        } else {
            "transport.stop"
        };
        let reply = bare(ask(context, json!({"type": kind}))?);
        let summary = if reply["playing"] == json!(true) {
            "playing from frame 0".to_owned()
        } else {
            "stopped".to_owned()
        };
        return Ok((reply, summary));
    }
    let Some(bpm) = arguments.get("bpm").and_then(Value::as_f64) else {
        return Err(refused("tempo needs `bpm`"));
    };
    let describe = ask(context, json!({"type": "describe"}))?;
    let targets = tempo_targets(&describe);
    if targets.controls.is_empty() {
        let mut why = Vec::new();
        if !targets.skipped.is_empty() {
            why.push(format!(
                "a connection drives {} (a value set by hand would not be what plays)",
                targets.skipped.join(", ")
            ));
        }
        if !targets.unreached.is_empty() {
            why.push(format!(
                "no top-level control reaches {} (export it from its composite)",
                targets.unreached.join(", ")
            ));
        }
        return Err(refused(if targets.transports.is_empty() {
            "the project has no transport (no std.transport and no std.playhead, as in lib.transport): it has no tempo to set".to_owned()
        } else {
            format!("no tempo to set: {}", why.join("; "))
        }));
    }
    let addresses: Vec<String> = targets.controls.iter().map(|(a, _)| a.clone()).collect();
    guard_live(
        context,
        &targets
            .controls
            .iter()
            .flat_map(|(_, ports)| ports.iter().map(|p| (p.clone(), Some(bpm))))
            .collect::<Vec<_>>(),
    )?;
    let ms = arguments.get("ms").and_then(Value::as_f64).unwrap_or(0.0);
    // One stamp for every target, so several transports change on the same
    // frame and stay in step (a playhead integrates its bpm: changes a block
    // apart would leave them a lasting phase apart). The stamp is ahead of
    // the current frame by enough for every request to arrive before it:
    // `TEMPO_LEAD_S` for each target, and once more, or the controller's
    // latency (`pong.latency`, two device buffers) instead of that once
    // more when it is longer, so a large device buffer does not make the
    // later targets late.
    let rate = describe["sampleRate"]
        .as_f64()
        .filter(|r| *r >= 1.0)
        .unwrap_or(48_000.0);
    let pong = ask(context, json!({"type": "ping"}))?;
    let now = pong["frame"].as_u64().unwrap_or(0);
    let latency = pong["latency"].as_u64().unwrap_or(0);
    let per_target = (TEMPO_LEAD_S * rate).ceil() as u64;
    let at = now + per_target.max(latency) + per_target * addresses.len() as u64;
    let mut acks = Vec::new();
    for address in &addresses {
        let message = if ms > 0.0 {
            json!({"type": "ramp", "address": address, "to": bpm, "ms": ms, "at": at})
        } else {
            json!({"type": "set", "address": address, "value": bpm, "at": at})
        };
        acks.push(bare(ask(context, message)?));
    }
    let late: Vec<String> = addresses
        .iter()
        .zip(&acks)
        .filter(|(_, ack)| ack["late"] == json!(true))
        .map(|(a, _)| a.clone())
        .collect();
    let mut summary = format!("tempo {bpm} bpm on {} at frame {at}", addresses.join(", "));
    if !late.is_empty() {
        summary.push_str(&format!(
            "; late (applied after frame {at}, so out of step): {}",
            late.join(", ")
        ));
    }
    if !targets.skipped.is_empty() {
        summary.push_str(&format!(
            "; skipped (wired): {}",
            targets.skipped.join(", ")
        ));
    }
    if !targets.unreached.is_empty() {
        summary.push_str(&format!(
            "; not reached by a top-level control: {}",
            targets.unreached.join(", ")
        ));
    }
    Ok((
        json!({
            "bpm": bpm,
            "at": at,
            "addresses": addresses,
            "skipped": targets.skipped,
            "unreached": targets.unreached,
            "late": late,
            "acks": acks,
        }),
        summary,
    ))
}

/// How far ahead of the current frame `transport` `tempo` stamps its
/// changes, in seconds for each target and once more (all on one frame).
const TEMPO_LEAD_S: f64 = 0.05;

/// The modules whose `bpm` is a project's tempo: the tick clock and the
/// playhead (`lib.transport` is a std.playhead, ADR 0033).
const TEMPO_MODULES: [&str; 2] = ["std.transport", "std.playhead"];

/// What `transport` `tempo` sets in a `describe` message, by the rule
/// Space's `transport.toggle` follows for `run` (ADR 0039 §6,
/// `ui/lib/transport-toggle.js`).
struct TempoTargets {
    /// The `bpm` parameter ports of every std.transport and std.playhead.
    transports: Vec<String>,
    /// The top-level controls (`describe.controls`) whose ports reach one
    /// and that no connection drives, in `describe`'s order, each with its
    /// ports: every one is set, so several transports stay in step.
    controls: Vec<(String, Vec<String>)>,
    /// The controls that reach one but are `modulated` (a connection drives
    /// them: a value set by hand would not be what plays).
    skipped: Vec<String>,
    /// The transports' bpm ports no top-level control reaches (inside a
    /// composite that does not export it): never set.
    unreached: Vec<String>,
    /// Each reached bpm port's control: its address and live base.
    reach: BTreeMap<String, (String, Value)>,
}

fn tempo_targets(describe: &Value) -> TempoTargets {
    let transports: Vec<String> = describe["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| TEMPO_MODULES.iter().any(|m| i["module"] == json!(m)))
        .filter_map(bpm_port)
        .collect();
    let mut controls = Vec::new();
    let mut skipped = Vec::new();
    let mut reach = BTreeMap::new();
    for control in describe["controls"].as_array().into_iter().flatten() {
        let Some(address) = control["address"].as_str() else {
            continue;
        };
        let ports: Vec<String> = control["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let reached: Vec<&String> = ports.iter().filter(|p| transports.contains(p)).collect();
        if reached.is_empty() {
            continue;
        }
        for port in reached {
            reach
                .entry(port.clone())
                .or_insert_with(|| (address.to_owned(), control["base"].clone()));
        }
        if control["modulated"] == json!(true) {
            skipped.push(address.to_owned());
        } else {
            controls.push((address.to_owned(), ports));
        }
    }
    let unreached = transports
        .iter()
        .filter(|p| !reach.contains_key(*p))
        .cloned()
        .collect();
    TempoTargets {
        transports,
        controls,
        skipped,
        unreached,
        reach,
    }
}

/// The `bpm` parameter port of a `describe` instance.
fn bpm_port(instance: &Value) -> Option<String> {
    let bpm = instance["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == json!("bpm"))
        .and_then(|p| p["address"].as_str())
        .map(str::to_owned);
    bpm.or_else(|| Some(format!("{}.bpm", instance["address"].as_str()?)))
}

fn commit(context: &Context, arguments: &Value) -> Outcome {
    let address = address(arguments);
    let describe = ask(context, json!({"type": "describe"}))?;
    let control = describe["controls"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["address"] == json!(address))
        .cloned();
    let value = match arguments.get("value") {
        Some(value) => value.clone(),
        None => control.as_ref().map_or(Value::Null, |c| c["base"].clone()),
    };
    if let Some(control) = &control {
        let ports: Vec<String> = control["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let written = value
            .as_f64()
            .or_else(|| control["default"].as_f64())
            .unwrap_or(0.0);
        // The file's own value is the baseline for what is written there
        // (`docs/agent-interface.md` "Safety").
        {
            let gains = current_gains(context)?;
            let file = control["inFile"]
                .as_bool()
                .filter(|in_file| *in_file)
                .and_then(|_| file_value(context, control));
            for port in ports.iter().filter(|p| gains.is_gain(p)) {
                let file = file.unwrap_or_else(|| control["default"].as_f64().unwrap_or(1.0));
                if !gain::commit_allowed(context.max_gain, written, file) {
                    return Err(refused(format!(
                        "refused: this would write {port} = {written} into project.toml, above --max-gain {} and above the file's {file}. Start `signal mcp` with a higher --max-gain to allow it.",
                        context.max_gain
                    )));
                }
            }
        }
        guard_live(
            context,
            &ports
                .into_iter()
                .map(|p| (p, Some(written)))
                .collect::<Vec<_>>(),
        )?;
    }
    let reply = bare(ask(
        context,
        json!({"type": "commit", "address": address, "value": value}),
    )?);
    let summary = format!(
        "committed {address} = {} to {}",
        reply["value"],
        reply["file"].as_str().unwrap_or("project.toml")
    );
    Ok((reply, summary))
}

/// A control's value in project.toml (the plan's base for its first port).
fn file_value(context: &Context, control: &Value) -> Option<f64> {
    let project = Project::from_sources_in(sources(context).ok()?, &context.dir).ok()?;
    let plan = project.plan(context.registry).ok()?.plan;
    let port = control["ports"].get(0)?.as_str()?;
    let (instance, parameter) = port.rsplit_once('.')?;
    plan.instances
        .iter()
        .find(|i| i.address.to_string() == instance)?
        .parameters
        .get(parameter)
        .copied()
}

fn revert(context: &Context, arguments: &Value) -> Outcome {
    let address = address(arguments);
    let ports = ports_of(context, &address)?;
    {
        let gains = current_gains(context)?;
        let project = Project::from_sources_in(sources(context)?, &context.dir).ok();
        let plan = project
            .and_then(|p| p.plan(context.registry).ok())
            .map(|p| p.plan);
        let file = |port: &str| -> Option<f64> {
            let (instance, parameter) = port.rsplit_once('.')?;
            plan.as_ref()?
                .instances
                .iter()
                .find(|i| i.address.to_string() == instance)?
                .parameters
                .get(parameter)
                .copied()
        };
        let reached: Vec<(String, Option<f64>)> = ports
            .iter()
            .map(|p| (p.clone(), file(p).filter(|_| gains.is_gain(p))))
            .collect();
        guard_live(context, &reached)?;
    }
    let reply = bare(ask(context, json!({"type": "revert", "address": address}))?);
    let summary = format!("reverted {address} to the file's {}", reply["value"]);
    Ok((reply, summary))
}

// ---------------------------------------------------------------------------
// Edit the graph.

fn patch_tool(context: &Context, arguments: &Value) -> Outcome {
    let level = text_arg(arguments, "level").to_owned();
    let file = if level.is_empty() {
        "project.toml".to_owned()
    } else {
        format!("composites/{level}.toml")
    };
    let files = sources(context)?;
    let Some(before) = files
        .iter()
        .find(|s| s.path == file)
        .map(|s| s.text.clone())
    else {
        return Err(refused(format!("the project has no composite `{level}`")));
    };
    let current = sha256_hex(before.as_bytes());
    let base = match arguments.get("base").and_then(Value::as_str) {
        Some(base) => base.to_owned(),
        None => current.clone(),
    };
    let ops: Vec<Op> = serde_json::from_value(arguments["ops"].clone())
        .map_err(|e| refused(format!("invalid arguments: `ops`: {e}")))?;
    // Nothing is judged while the files do not load (fail closed).
    current_gains(context)?;
    // Only the version checked is ever sent: another `base` is a conflict
    // here, as the server would answer.
    if base != current {
        return Err(Refused {
            message: format!("conflict: {file} changed since the version the patch names"),
            detail: json!({"code": "conflict", "level": level, "file": file, "base": current}),
        });
    }
    // The edited files, as the server would write them, for the gain check.
    {
        let project = Project::from_sources_in(files.clone(), &context.dir).ok();
        let order = |module: &str| -> Option<Vec<String>> {
            if let Some(order) = signal_project::library_parameters(module) {
                return Some(order);
            }
            if let Some(t) = context.registry.get(module) {
                return Some(
                    t.descriptor
                        .parameters
                        .iter()
                        .map(|p| p.name.to_owned())
                        .collect(),
                );
            }
            project
                .as_ref()
                .and_then(|p| p.composites.iter().find(|c| c.name == module))
                .map(|c| {
                    c.exports
                        .parameters
                        .iter()
                        .map(|e| e.name.clone())
                        .collect()
                })
        };
        let ctx = patch::Context {
            level: &level,
            module: &order,
        };
        // A patch that cannot be simulated here is not sent (fail closed).
        let patched = patch::apply(&before, None, &ops, &ctx)
            .map_err(|e| refused(format!("{}: {}", e.code, e.message)))?;
        guard_files(context, replaced(files, &file, &patched.text))?;
    }
    let reply = bare(ask(
        context,
        json!({"type": "patch", "level": level, "base": base, "ops": arguments["ops"]}),
    )?);
    let summary = format!(
        "patched {} ({} ops); new base {}",
        reply["file"].as_str().unwrap_or(&file),
        ops.len(),
        reply["base"].as_str().unwrap_or("")
    );
    Ok((reply, summary))
}

fn schematic_move(context: &Context, arguments: &Value) -> Outcome {
    still_same_project(context)?;
    let mut message = json!({"type": "schematic.move", "level": text_arg(arguments, "level")});
    for key in ["instance", "x", "y", "moves"] {
        if let Some(value) = arguments.get(key) {
            message[key] = value.clone();
        }
    }
    let reply = bare(ask(context, message)?);
    let summary = format!(
        "moved in {}",
        reply["file"].as_str().unwrap_or("ui/schematic.toml")
    );
    Ok((reply, summary))
}

fn vendor_tool(context: &Context, arguments: &Value) -> Outcome {
    let composite = text_arg(arguments, "composite");
    let name = arguments.get("as").and_then(Value::as_str);
    let read = ask(
        context,
        json!({"type": "file.read", "file": "project.toml"}),
    )?;
    // Nothing is judged while the files do not load, and what cannot be
    // prepared here is not sent (fail closed).
    current_gains(context)?;
    let prepared = vendor::prepare(&context.dir, composite, name, Library::builtin())
        .map_err(|e| refused(format!("not vendored: {e}")))?;
    guard_files(context, prepared.after())?;
    let mut message = json!({"type": "vendor", "composite": composite, "base": read["base"]});
    if let Some(name) = name {
        message["as"] = json!(name);
    }
    let reply = bare(ask(context, message)?);
    let summary = format!(
        "vendored {composite}: copied {}",
        reply["copied"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok((reply, summary))
}

// ---------------------------------------------------------------------------
// Edit text.

fn written(reply: Value, file: &str) -> Outcome {
    let reply = bare(reply);
    let summary = format!(
        "wrote {file}{}; new base {}",
        if reply["created"] == json!(true) {
            " (created)"
        } else {
            ""
        },
        reply["base"].as_str().unwrap_or("")
    );
    Ok((reply, summary))
}

fn write_pattern(context: &Context, arguments: &Value) -> Outcome {
    let name = text_arg(arguments, "name");
    let text = text_arg(arguments, "text");
    let file = format!("patterns/{name}.toml");
    guard_files(context, replaced(sources(context)?, &file, text))?;
    let reply = ask(
        context,
        json!({"type": "pattern.write", "name": name, "text": text, "base": arguments["base"]}),
    )?;
    written(reply, &file)
}

fn write_panel(context: &Context, arguments: &Value) -> Outcome {
    still_same_project(context)?;
    let file = match text_arg(arguments, "file") {
        "" => MAIN_PANEL,
        file => file.strip_prefix("ui/").unwrap_or(file),
    };
    let reply = ask(
        context,
        json!({"type": "panel.write", "file": file, "html": arguments["html"], "base": arguments["base"]}),
    )?;
    written(reply, &format!("ui/{file}"))
}

fn write_file(context: &Context, arguments: &Value) -> Outcome {
    let file = text_arg(arguments, "file");
    let text = text_arg(arguments, "text");
    if let Some(name) = file
        .strip_prefix("patterns/")
        .and_then(|f| f.strip_suffix(".toml"))
    {
        let mut pattern = arguments.clone();
        pattern["name"] = json!(name);
        if let Some(fields) = pattern.as_object_mut() {
            fields.remove("file");
        }
        return write_pattern(context, &pattern);
    }
    if file.starts_with("ui/") {
        return Err(refused(
            "ui/ files are written with write_panel (a panel's <signal-panel> content) or schematic_move (positions)",
        ));
    }
    guard_files(context, replaced(sources(context)?, file, text))?;
    let reply = ask(
        context,
        json!({"type": "file.write", "file": file, "text": text, "base": arguments["base"]}),
    )?;
    written(reply, file)
}

// ---------------------------------------------------------------------------
// Observe.

/// The window in milliseconds, at least one 256-point block at `rate`.
fn window(arguments: &Value, default: f64, rate: f64) -> Result<Duration, Refused> {
    let window = arguments
        .get("window_ms")
        .and_then(Value::as_f64)
        .unwrap_or(default);
    let minimum = 256.0 / rate * 1000.0;
    if window < minimum {
        return Err(refused(format!(
            "window_ms must be at least {} at {rate} points per second: a scope stream delivers blocks of 256 points",
            minimum.ceil()
        )));
    }
    Ok(Duration::from_secs_f64(window / 1000.0))
}

/// Subscribes to `stream`, collects its frames over `window` (and until at
/// least one arrived, within a bound), then unsubscribes.
fn collect(
    context: &Context,
    stream: Value,
    window: Duration,
    block: Duration,
) -> Result<(Value, Vec<Frame>), Refused> {
    let mut message = stream.clone();
    message["type"] = json!("subscribe");
    let reply = ask(context, message)?;
    let sub = reply["sub"].as_u64().unwrap_or(0) as u32;
    let start = Instant::now();
    // A loaded machine delivers late: wait for a first frame well past the
    // window before saying nothing arrived.
    let until = start + window + block + Duration::from_secs(5);
    let mut frames = Vec::new();
    loop {
        thread::sleep(Duration::from_millis(5));
        frames.extend(lock(context).client.frames(sub));
        let now = Instant::now();
        if (now >= start + window && !frames.is_empty()) || now >= until {
            break;
        }
    }
    unsubscribe(context, sub);
    if frames.is_empty() {
        let what = stream["port"]
            .as_str()
            .or(stream["source"].as_str())
            .unwrap_or("the stream");
        return Err(refused(format!(
            "no data arrived from {what} within {:.1} s (is the project playing? `project_info` says)",
            (until - start).as_secs_f64()
        )));
    }
    Ok((reply, frames))
}

fn db(level: f64) -> Value {
    if level > 0.0 {
        json!(20.0 * level.log10())
    } else {
        Value::Null
    }
}

fn levels(points: &[f32]) -> (f64, f64) {
    let peak = points.iter().fold(0.0f64, |p, x| p.max(f64::from(x.abs())));
    let rms = if points.is_empty() {
        0.0
    } else {
        (points.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / points.len() as f64).sqrt()
    };
    (peak, rms)
}

fn meter(context: &Context, arguments: &Value) -> Outcome {
    if let Some(ports) = arguments.get("ports").and_then(Value::as_array) {
        let rate = 4800.0;
        let span = window(arguments, 500.0, rate)?;
        let mut levels_of = Vec::new();
        for port in ports.iter().filter_map(Value::as_str) {
            let (_, frames) = collect(
                context,
                json!({"stream": "scope", "port": port, "rate": rate}),
                span,
                Duration::from_secs_f64(256.0 / rate),
            )?;
            let points: Vec<f32> = frames.iter().flat_map(Frame::f32s).collect();
            let (peak, rms) = levels(&points);
            levels_of.push(json!({"port": port, "peak": peak, "rms": rms, "peakDb": db(peak), "rmsDb": db(rms), "points": points.len()}));
        }
        let summary = levels_of
            .iter()
            .map(|l| {
                format!(
                    "{} peak {:.4} rms {:.4}",
                    l["port"].as_str().unwrap_or(""),
                    l["peak"].as_f64().unwrap_or(0.0),
                    l["rms"].as_f64().unwrap_or(0.0)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Ok((json!({"ports": levels_of, "decimated": true}), summary));
    }
    let source = arguments
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("output");
    let span = window(arguments, 500.0, f64::INFINITY)?;
    let (_, frames) = collect(
        context,
        json!({"stream": "meters", "source": source, "rate": 30}),
        span,
        Duration::from_millis(100),
    )?;
    // Per channel: the highest peak, and the RMS over every frame covered.
    let mut channels: Vec<(f64, f64, f64)> = Vec::new();
    for frame in &frames {
        let covered = f64::from(frame.extra.max(1));
        for (c, pair) in frame.f32s().chunks(2).enumerate() {
            if channels.len() <= c {
                channels.push((0.0, 0.0, 0.0));
            }
            if let (Some(channel), [peak, rms]) = (channels.get_mut(c), pair) {
                channel.0 = channel.0.max(f64::from(*peak));
                channel.1 += f64::from(*rms).powi(2) * covered;
                channel.2 += covered;
            }
        }
    }
    let channels: Vec<Value> = channels
        .iter()
        .enumerate()
        .map(|(c, (peak, sum, covered))| {
            let rms = if *covered > 0.0 {
                (sum / covered).sqrt()
            } else {
                0.0
            };
            json!({"channel": c, "peak": peak, "rms": rms, "peakDb": db(*peak), "rmsDb": db(rms)})
        })
        .collect();
    let summary = if channels.is_empty() {
        format!("no {source} levels arrived (is it playing?)")
    } else {
        channels
            .iter()
            .map(|c| {
                format!(
                    "ch {} peak {:.4} rms {:.4}",
                    c["channel"],
                    c["peak"].as_f64().unwrap_or(0.0),
                    c["rms"].as_f64().unwrap_or(0.0)
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    Ok((
        json!({"source": source, "frames": frames.len(), "channels": channels}),
        summary,
    ))
}

fn scope(context: &Context, arguments: &Value) -> Outcome {
    let port = text_arg(arguments, "port");
    let rate = arguments
        .get("rate")
        .and_then(Value::as_f64)
        .unwrap_or(4800.0);
    let span = window(arguments, 200.0, rate)?;
    let wanted = arguments
        .get("points")
        .and_then(Value::as_u64)
        .unwrap_or(256) as usize;
    let (reply, frames) = collect(
        context,
        json!({"stream": "scope", "port": port, "rate": rate}),
        span,
        Duration::from_secs_f64(256.0 / rate),
    )?;
    let first = frames.first().map(|f| f.first);
    let points: Vec<f32> = frames.iter().flat_map(Frame::f32s).collect();
    let (peak, rms) = levels(&points);
    let step = points.len().div_ceil(wanted.max(1)).max(1);
    let shown: Vec<f32> = points.iter().step_by(step).copied().collect();
    let summary = format!(
        "{port}: {} points ({} shown), peak {peak:.4}, rms {rms:.4}",
        points.len(),
        shown.len()
    );
    Ok((
        json!({
            "port": port,
            "decimation": reply["decimation"],
            "firstFrame": first,
            "every": step,
            "points": shown,
            "peak": peak,
            "rms": rms,
        }),
        summary,
    ))
}

fn read_stream(context: &Context, arguments: &Value) -> Outcome {
    let rate = arguments
        .get("rate")
        .and_then(Value::as_f64)
        .unwrap_or(512.0);
    let span = window(arguments, 1000.0, rate)?;
    let describe = ask(context, json!({"type": "describe"}))?;
    let instances = describe["instances"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ports: Vec<String> = match arguments.get("ports").and_then(Value::as_array) {
        Some(ports) => ports
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => instances
            .iter()
            .filter_map(|i| {
                let output = match i["module"].as_str()? {
                    "std.step_seq8" => "step",
                    "std.note_seq" => "position",
                    _ => return None,
                };
                Some(format!("{}.{output}", i["address"].as_str()?))
            })
            .collect(),
    };
    if ports.is_empty() {
        return Err(refused(
            "the project has no sequencer (std.step_seq8, std.note_seq): name `ports` to read",
        ));
    }
    let mut read = Vec::new();
    for port in &ports {
        let (_, frames) = collect(
            context,
            json!({"stream": "scope", "port": port, "rate": rate}),
            span,
            Duration::from_secs_f64(256.0 / rate),
        )?;
        let points: Vec<f32> = frames.iter().flat_map(Frame::f32s).collect();
        if points.is_empty() {
            return Err(refused(format!("no data arrived from {port}")));
        }
        let last_frame = frames
            .last()
            .map(|f| f.first + u64::from(f.count.saturating_sub(1)) * u64::from(f.extra.max(1)));
        let min = points.iter().copied().fold(f32::INFINITY, f32::min);
        let max = points.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut entry = json!({
            "port": port,
            "points": points.len(),
            "first": points.first(),
            "last": points.last(),
            "min": points.first().map(|_| min),
            "max": points.first().map(|_| max),
            "lastFrame": last_frame,
        });
        if port.ends_with(".step") {
            entry["step"] = json!(points.last().map(|s| s.round() as i64));
        }
        if port.ends_with(".position") {
            entry["position"] = json!(points.last());
        }
        read.push(entry);
    }
    // Each transport's bpm: the live base of the control `tempo` sets
    // (`control`; null when no top-level control reaches it), else the
    // plan's base (`planBpm`: the files' value, which a live change does not
    // move until it is committed). A control a connection drives
    // (`modulated`) is not one `tempo` sets: it is named as `wired`, with
    // `control` and `bpm` null, since what plays is what the connection
    // makes of it.
    let targets = tempo_targets(&describe);
    let reach = targets.reach;
    let wired = targets.skipped;
    let bpm: Vec<Value> = instances
        .iter()
        .filter(|i| TEMPO_MODULES.iter().any(|m| i["module"] == json!(m)))
        .map(|i| {
            let plan = i["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["name"] == json!("bpm"))
                .map_or(Value::Null, |p| p["base"].clone());
            let control = bpm_port(i).and_then(|p| reach.get(&p).cloned());
            if let Some((address, _)) = control.as_ref().filter(|c| wired.contains(&c.0)) {
                return json!({
                    "instance": i["address"],
                    "control": null,
                    "wired": address,
                    "bpm": null,
                    "planBpm": plan,
                });
            }
            json!({
                "instance": i["address"],
                "control": control.as_ref().map(|c| c.0.clone()),
                "wired": null,
                "bpm": control.map_or_else(|| plan.clone(), |c| c.1),
                "planBpm": plan,
            })
        })
        .collect();
    let pong = ask(context, json!({"type": "ping"}))?;
    let summary = read
        .iter()
        .map(|r| format!("{} = {}", r["port"].as_str().unwrap_or(""), r["last"]))
        .collect::<Vec<_>>()
        .join("; ");
    Ok((
        json!({"frame": pong["frame"], "streams": read, "transport": bpm}),
        summary,
    ))
}

/// The next free numbered folder `<out>/<prefix>-<n>`.
fn numbered(out: &Path, prefix: &str) -> PathBuf {
    (1..)
        .map(|n| out.join(format!("{prefix}-{n}")))
        .find(|dir| !dir.exists())
        .unwrap_or_else(|| out.join(prefix))
}

fn render(context: &Context, arguments: &Value) -> Outcome {
    let seconds = arguments
        .get("seconds")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    let out = numbered(&context.out, "render");
    let args = RenderArgs {
        project: context.dir.clone(),
        seconds: Some(seconds),
        frames: None,
        out: out.clone(),
        seed: arguments.get("seed").and_then(Value::as_u64),
        sample_rate: None,
        block: 256,
        threads: 0,
    };
    let rendered =
        render_project(&args, context.registry, context.setup).map_err(|f| refused(f.message()))?;
    let output = rendered.output;
    let wav = out.join("audio.wav");
    let summary = format!(
        "rendered {} frames to {} (offline, the project as written: no output ceiling applies, and nothing is played); audio_fnv1a64 {}",
        output["frames"],
        wav.display(),
        output["manifest"]["audio_fnv1a64"].as_str().unwrap_or("")
    );
    Ok((
        json!({
            "wav": wav.display().to_string(),
            "out": out.display().to_string(),
            "frames": output["frames"],
            "sampleRate": output["sample_rate"],
            "channels": output["channels"],
            "hashes": {
                "audio_fnv1a64": output["manifest"]["audio_fnv1a64"],
                "log_fnv1a64": output["manifest"]["log_fnv1a64"],
            },
            "manifest": output["manifest"],
            "metrics": output["metrics"],
            "warnings": output["warnings"],
            "outputCeiling": null,
            "played": false,
        }),
        summary,
    ))
}

// ---------------------------------------------------------------------------
// Checks and library.

fn ui_check(context: &Context, _: &Value) -> Outcome {
    let dir = &context.dir;
    let project = Project::load(dir).map_err(|e| refused(e.to_string()))?;
    project.plan(context.registry).map_err(|diagnostics| {
        refused(
            located(dir, diagnostics)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    let checked = check_project(dir, &project, context.registry).map_err(refused)?;
    let diagnostics: Vec<String> = located(
        dir,
        checked
            .iter()
            .flat_map(|c| c.findings.iter().map(|f| f.diagnostic.clone()))
            .collect(),
    )
    .iter()
    .map(ToString::to_string)
    .collect();
    let failed = checked.iter().any(|c| c.failed());
    let panels: Vec<Value> = checked
        .iter()
        .map(|c| json!({"file": c.file, "controls": c.controls, "bindings": c.bindings}))
        .collect();
    let summary = if failed {
        format!("the panels have problems:\n{}", diagnostics.join("\n"))
    } else {
        format!("{} panels: ok", panels.len())
    };
    Ok((
        json!({
            "ok": !failed,
            "panels": panels,
            "generated": !project_ui(dir).join(MAIN_PANEL).is_file(),
            "diagnostics": diagnostics,
        }),
        summary,
    ))
}

fn check(context: &Context, _: &Value) -> Outcome {
    let dir = &context.dir;
    let lines =
        |d: Vec<_>| -> Vec<String> { located(dir, d).iter().map(ToString::to_string).collect() };
    let loaded = match read_sources(dir) {
        Ok(files) => Project::from_sources_in(files, dir).map_err(lines),
        Err(e) => Err(vec![e.to_string()]),
    };
    let result = loaded.and_then(|project| {
        project
            .plan(context.registry)
            .map(|planned| lines(planned.warnings))
            .map_err(lines)
    });
    Ok(match result {
        Ok(warnings) => (
            json!({"ok": true, "diagnostics": [], "warnings": warnings}),
            format!("the project loads and plans ({} warnings)", warnings.len()),
        ),
        Err(diagnostics) => (
            json!({"ok": false, "diagnostics": diagnostics, "warnings": []}),
            format!("the project does not load:\n{}", diagnostics.join("\n")),
        ),
    })
}

fn library(context: &Context, _: &Value) -> Outcome {
    let entries = signal_project::library_modules(context.registry)
        .map_err(|d| refused(format!("the built-in library does not load: {d:?}")))?;
    let summary = format!(
        "built-in library {}: {} composites",
        Library::VERSION,
        entries.len()
    );
    Ok((
        json!({
            "version": Library::VERSION,
            "composites": entries,
            "changelog": include_str!("../../../signal-lib/CHANGELOG.md"),
        }),
        summary,
    ))
}

// ---------------------------------------------------------------------------
// Sessions.

fn session_start(context: &Context, arguments: &Value) -> Outcome {
    let describe = ask(context, json!({"type": "describe"}))?;
    let unsaved = describe["unsaved"]["count"].as_u64().unwrap_or(0);
    if unsaved > 0 && arguments.get("discard") != Some(&json!(true)) {
        return Err(refused(format!(
            "{unsaved} parameters are unsaved: a session starts from the files at frame 0, so commit or revert them first, or pass `discard: true`"
        )));
    }
    // Discarded values go back to the files': checked as a revert is.
    guard_restart(context)?;
    let mut link = lock(context);
    let Some(control) = &link.control else {
        return Err(refused(
            "sessions need `signal mcp <project>` (not --attach)",
        ));
    };
    link.client = control.session_start().map_err(refused)?;
    Ok((
        json!({"recording": true, "frame": 0}),
        "recording a session from frame 0".to_owned(),
    ))
}

fn session_stop(context: &Context, _: &Value) -> Outcome {
    let folder = numbered(&context.out, "session");
    let mut link = lock(context);
    let Some(control) = &link.control else {
        return Err(refused(
            "sessions need `signal mcp <project>` (not --attach)",
        ));
    };
    let (client, written) = control.session_stop(&folder).map_err(refused)?;
    link.client = client;
    let summary = format!(
        "recorded {} frames into {}",
        written["frames"],
        written["session"].as_str().unwrap_or("")
    );
    Ok((written, summary))
}

/// The last session folder written under `out`.
fn last_session(out: &Path) -> Option<PathBuf> {
    (1..)
        .map(|n| out.join(format!("session-{n}")))
        .take_while(|dir| dir.exists())
        .last()
}

fn replay(context: &Context, arguments: &Value) -> Outcome {
    let folder = match arguments.get("session").and_then(Value::as_str) {
        Some(session) => PathBuf::from(session),
        None => last_session(&context.out)
            .ok_or_else(|| refused("no session recorded yet: session_start, then session_stop"))?,
    };
    // A folder session_stop wrote holds `session/` and `captured.json`; any
    // other is a session directory itself.
    let (session, captured) = if folder.join("session").is_dir() {
        let captured = fs::read_to_string(folder.join("captured.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        (folder.join("session"), captured)
    } else {
        (folder.clone(), None)
    };
    let out = numbered(&context.out, "replay");
    let args = ReplayArgs {
        session: session.clone(),
        out: Some(out.clone()),
        frames: None,
        threads: 0,
        ideal: false,
        trust_provenance: false,
    };
    let replayed =
        replay_session(&args, context.registry, context.setup).map_err(|f| refused(f.message()))?;
    let replay_hash = replayed.output["manifest"]["audio_fnv1a64"].clone();
    let recorded = captured
        .as_ref()
        .map_or(Value::Null, |c| c["capturedFnv1a64"].clone());
    // The capture is what the device got, held within the run's output
    // ceiling: the replay's audio is compared held the same way.
    let ceiling = captured.as_ref().and_then(|c| c["ceiling"].as_f64());
    let hash = match ceiling {
        Some(ceiling) => held_hash(&out.join("audio.wav"), ceiling)?,
        None => replay_hash.clone(),
    };
    let identical = !recorded.is_null() && recorded == hash;
    let summary = format!(
        "replayed {} frames: audio_fnv1a64 {}{}",
        replayed.output["frames"],
        hash.as_str().unwrap_or(""),
        if recorded.is_null() {
            String::new()
        } else if identical {
            ", identical to the recorded run".to_owned()
        } else {
            format!(", DIFFERENT from the recorded run's {recorded}")
        }
    );
    Ok((
        json!({
            "session": session.display().to_string(),
            "out": out.display().to_string(),
            "audio_fnv1a64": replay_hash,
            "heldFnv1a64": if ceiling.is_some() { hash } else { Value::Null },
            "ceiling": ceiling,
            "capturedFnv1a64": recorded,
            "identical": identical,
            "frames": replayed.output["frames"],
            "warnings": replayed.warnings,
        }),
        summary,
    ))
}

/// FNV-1a 64 of the replay's `audio.wav` with every sample held within
/// ±`ceiling` as the run's output adapter held it ([`signal_output::hold`]).
fn held_hash(wav: &Path, ceiling: f64) -> Result<Value, Refused> {
    let mut reader =
        hound::WavReader::open(wav).map_err(|e| refused(format!("{}: {e}", wav.display())))?;
    // The same f32 level the runner holds (`Runner::set_ceiling`).
    let level = Some(signal_output::ceiling_level(ceiling));
    let mut hasher = signal_core::Fnv1a64::new();
    let mut held = 0;
    for sample in reader.samples::<f32>() {
        let sample = sample.map_err(|e| refused(format!("{}: {e}", wav.display())))?;
        let sample = signal_output::hold(sample, level, &mut held);
        hasher.update(&sample.to_le_bytes());
    }
    Ok(json!(format!("{:016x}", hasher.finish())))
}

// ---------------------------------------------------------------------------
// Resources: the project's text files and the docs an agent needs.

const DOCS: &[(&str, &str)] = &[
    (
        "agent-interface.md",
        include_str!("../../../../docs/agent-interface.md"),
    ),
    (
        "module-contract.md",
        include_str!("../../../../docs/module-contract.md"),
    ),
    (
        "project-format.md",
        include_str!("../../../../docs/project-format.md"),
    ),
    (
        "ui-protocol.md",
        include_str!("../../../../docs/ui-protocol.md"),
    ),
];

/// Whether `path` is a link (not followed).
fn linked(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The project's own text files, as the read and write tools reach them:
/// `project.toml`, `composites/*.toml`, `patterns/*.toml`, `ui/*.html` and
/// `ui/schematic.toml`, by path relative to the folder, in path order. A
/// file that is a link, or in a folder that is one, is never listed (as
/// `read_file` refuses it).
fn project_text_files(dir: &Path) -> Vec<String> {
    let own_file = |path: &Path| {
        !linked(path) && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
    };
    let mut paths = Vec::new();
    if own_file(&dir.join("project.toml")) {
        paths.push("project.toml".to_owned());
    }
    for (folder, extension) in [("composites", "toml"), ("patterns", "toml"), ("ui", "html")] {
        let full = dir.join(folder);
        if linked(&full) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&full) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let path = entry.path();
                path.extension().is_some_and(|e| e == extension) && own_file(&path)
            })
            .filter_map(|entry| entry.file_name().to_str().map(|n| format!("{folder}/{n}")))
            .filter(|path| !path.split('/').any(|part| part.starts_with('.')))
            .collect();
        if folder == "ui" && own_file(&full.join("schematic.toml")) {
            names.push("ui/schematic.toml".to_owned());
        }
        paths.extend(names);
    }
    paths.sort();
    paths
}

fn mime(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html"
    } else {
        "application/toml"
    }
}

/// `resources/list`.
pub(crate) fn resources(context: &Context) -> Vec<Value> {
    let mut list: Vec<Value> = project_text_files(&context.dir)
        .iter()
        .map(|path| {
            json!({
                "uri": format!("signal://project/{path}"),
                "name": path,
                "mimeType": mime(path),
            })
        })
        .collect();
    list.push(json!({"uri": "signal://modules", "name": "modules", "description": "The module catalogue (as `signal modules --json`)", "mimeType": "application/json"}));
    list.extend(DOCS.iter().map(|(name, _)| {
        json!({"uri": format!("signal://docs/{name}"), "name": name, "mimeType": "text/markdown"})
    }));
    list
}

/// `resources/read`.
pub(crate) fn read_resource(context: &Context, uri: &str) -> Result<Value, String> {
    if let Some(name) = uri.strip_prefix("signal://docs/") {
        let (_, text) = DOCS
            .iter()
            .find(|(doc, _)| *doc == name)
            .ok_or_else(|| format!("Resource not found: {uri}"))?;
        return Ok(json!({"uri": uri, "mimeType": "text/markdown", "text": text}));
    }
    if uri == "signal://modules" {
        let reply = lock(context)
            .client
            .request(json!({"type": "modules"}))
            .map_err(|e| e["message"].as_str().unwrap_or("").to_owned())?;
        return Ok(
            json!({"uri": uri, "mimeType": "application/json", "text": bare(reply).to_string()}),
        );
    }
    // Only one of the project's own text files, never another path, a link
    // or a file in a linked folder.
    let path = uri
        .strip_prefix("signal://project/")
        .ok_or_else(|| format!("Resource not found: {uri}"))?;
    if !project_text_files(&context.dir).iter().any(|p| p == path) {
        return Err(format!("Resource not found: {uri}"));
    }
    let text = fs::read_to_string(context.dir.join(path))
        .map_err(|_| format!("Resource not found: {uri}"))?;
    Ok(json!({"uri": uri, "mimeType": mime(path), "text": text}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// review L4: a result over the limit keeps its small fields, replaces
    /// the large ones, says so, and is not repeated as text.
    #[test]
    fn large_results_are_capped() {
        let small = json!({"a": 1, "b": "two"});
        let (kept, text) = capped("graph", small.clone());
        assert_eq!(kept, small);
        assert_eq!(text, Some(small.to_string()));
        let big: Vec<Value> = (0..40_000)
            .map(|i| json!({"address": format!("/voice{i}"), "x": i}))
            .collect();
        let (kept, text) = capped("graph", json!({"project": "p", "top": big}));
        assert!(text.is_none(), "a large result is not repeated as text");
        assert!(kept.to_string().len() <= MAX_RESULT);
        assert_eq!(kept["truncated"], true);
        assert_eq!(kept["project"], "p");
        assert_eq!(kept["top"]["omitted"], true);
        assert_eq!(kept["top"]["items"], 40_000);
        assert!(kept["guidance"].as_str().unwrap().contains("level"));
        // Between the two limits: whole, but not repeated as text.
        let middle: Vec<Value> = (0..2_000).map(|i| json!(i)).collect();
        let (kept, text) = capped("stats", json!({"m": middle}));
        assert!(kept.get("truncated").is_none());
        assert!(text.is_some() == (kept.to_string().len() <= MAX_RESULT_TEXT));
    }

    /// review M5: a graph of another folder is not this project's.
    #[test]
    fn the_project_is_the_one_played() {
        let dir = Path::new("/projects/a");
        let id = sha256_hex(dir.as_os_str().as_encoded_bytes());
        assert!(plays(dir, &json!({"projectId": id})));
        assert!(!plays(Path::new("/projects/b"), &json!({"projectId": id})));
        assert!(!plays(dir, &json!({})));
    }
}
