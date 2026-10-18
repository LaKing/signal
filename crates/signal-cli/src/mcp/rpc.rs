//! JSON-RPC 2.0 and the MCP methods (`docs/agent-interface.md` "Protocol").
//!
//! The server is *dual-era* in the specification's sense
//! (modelcontextprotocol.io, "Versioning and Compatibility"): it speaks the
//! current revision, **2026-07-28** (no handshake: every request names its
//! version in `_meta`, `server/discover` answers what is supported, an
//! unsupported version is error -32022 listing the supported ones), and the
//! handshake-based revisions **2025-11-25** and **2025-06-18** (`initialize`
//! negotiates one of them for the process).

use serde_json::{Map, Value, json};

use super::Context;
use super::tools;

/// The current MCP revision.
pub(crate) const MODERN: &str = "2026-07-28";
/// The handshake-based revisions `initialize` negotiates, newest first.
pub(crate) const LEGACY: [&str; 2] = ["2025-11-25", "2025-06-18"];

const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_VERSION: i64 = -32022;

const INSTRUCTIONS: &str = "Signal plays a project of real-time signal modules. Its text files (project.toml, composites/, patterns/, ui/) are the source of truth: every tool that changes something writes those files through Signal's own write paths, with the file's SHA-256 as `base` (a `conflict` returns the current text). Start with project_info and graph; set_parameter and ramp_parameter adjust live, commit writes a value into project.toml; patch edits the graph; meter, scope and read_stream observe; render renders offline. A change that would put a std.audio_out gain above the server's --max-gain is refused.";

/// The MCP server's state: the legacy revision `initialize` negotiated, if
/// any (the modern revision has none).
pub(crate) struct Server<'c, 'a> {
    context: &'c Context<'a>,
    legacy: Option<&'static str>,
}

fn error(id: &Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message.into()});
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

impl<'c, 'a> Server<'c, 'a> {
    pub(crate) fn new(context: &'c Context<'a>) -> Server<'c, 'a> {
        Server {
            context,
            legacy: None,
        }
    }

    fn capabilities() -> Value {
        json!({"tools": {"listChanged": false}, "resources": {"listChanged": false}})
    }

    fn server_info() -> Value {
        json!({"name": "signal", "version": crate::VERSION})
    }

    /// The reply to one line from stdin, `None` for a notification.
    pub(crate) fn handle(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => {
                return Some(error(
                    &Value::Null,
                    PARSE_ERROR,
                    format!("Parse error: {e}"),
                    None,
                ));
            }
        };
        let Value::Object(message) = message else {
            // Batches are not part of MCP since 2025-06-18.
            return Some(error(
                &Value::Null,
                INVALID_REQUEST,
                "Invalid Request: a message is one JSON object",
                None,
            ));
        };
        let id = message.get("id").cloned();
        let valid_id = matches!(id, None | Some(Value::String(_) | Value::Number(_)));
        let method = message.get("method").and_then(Value::as_str);
        let reply_id = id.clone().filter(|_| valid_id).unwrap_or(Value::Null);
        if message.get("jsonrpc") != Some(&json!("2.0")) || !valid_id {
            return Some(error(
                &reply_id,
                INVALID_REQUEST,
                "Invalid Request: needs \"jsonrpc\": \"2.0\" and an id that is a string or a number",
                None,
            ));
        }
        let Some(method) = method else {
            // A response from the client (the server sends no requests): ignored.
            return None;
        };
        let params = match message.get("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(_) => {
                return id.map(|id| error(&id, INVALID_PARAMS, "Invalid params: an object", None));
            }
        };
        // Notifications (`notifications/initialized`, `…/cancelled`) need no
        // reply; requests here finish before the next is read.
        let id = id?;
        Some(self.request(&id, method, &params))
    }

    fn request(&mut self, id: &Value, method: &str, params: &Map<String, Value>) -> Value {
        let named = params
            .get("_meta")
            .and_then(|m| m.get(VERSION_KEY))
            .and_then(Value::as_str);
        let supported = || {
            let mut all = vec![MODERN];
            all.extend(LEGACY);
            all
        };
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = LEGACY
                    .iter()
                    .copied()
                    .find(|v| Some(*v) == asked)
                    .unwrap_or(LEGACY[0]);
                self.legacy = Some(version);
                return result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": Self::capabilities(),
                        "serverInfo": Self::server_info(),
                        "instructions": INSTRUCTIONS,
                    }),
                );
            }
            "ping" => return result(id, json!({})),
            _ => {}
        }
        // The era: a modern request names its version; a legacy one follows
        // `initialize`.
        let modern = match named {
            Some(MODERN) => true,
            Some(other) => {
                return error(
                    id,
                    UNSUPPORTED_VERSION,
                    "Unsupported protocol version",
                    Some(json!({"supported": supported(), "requested": other})),
                );
            }
            None if self.legacy.is_some() => false,
            None if method == "server/discover" => true,
            None => {
                return error(
                    id,
                    INVALID_REQUEST,
                    format!(
                        "Invalid Request: send `initialize` first ({}), or name the protocol version in `_meta` (\"{VERSION_KEY}\": \"{MODERN}\")",
                        LEGACY[0]
                    ),
                    Some(json!({"supported": supported()})),
                );
            }
        };
        let complete = |mut value: Value| {
            if modern {
                value["resultType"] = json!("complete");
            }
            result(id, value)
        };
        match method {
            "server/discover" => complete(json!({
                "supportedVersions": supported(),
                "capabilities": Self::capabilities(),
                "_meta": {"io.modelcontextprotocol/serverInfo": Self::server_info()},
                "instructions": INSTRUCTIONS,
            })),
            "tools/list" => complete(json!({"tools": tools::list(self.context)})),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return error(id, INVALID_PARAMS, "Invalid params: `name`, a string", None);
                };
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => Value::Object(Map::new()),
                    Some(arguments @ Value::Object(_)) => arguments.clone(),
                    Some(_) => {
                        return error(
                            id,
                            INVALID_PARAMS,
                            "Invalid params: `arguments` is an object",
                            None,
                        );
                    }
                };
                match tools::call(self.context, name, &arguments) {
                    Some(outcome) => complete(outcome),
                    None => error(id, INVALID_PARAMS, format!("Unknown tool: {name}"), None),
                }
            }
            "resources/list" => complete(json!({"resources": tools::resources(self.context)})),
            "resources/read" => {
                let Some(uri) = params.get("uri").and_then(Value::as_str) else {
                    return error(id, INVALID_PARAMS, "Invalid params: `uri`, a string", None);
                };
                match tools::read_resource(self.context, uri) {
                    Ok(contents) => complete(json!({"contents": [contents]})),
                    Err(message) => error(id, -32002, message, Some(json!({"uri": uri}))),
                }
            }
            _ => error(
                id,
                METHOD_NOT_FOUND,
                format!("Method not found: {method}"),
                None,
            ),
        }
    }
}
