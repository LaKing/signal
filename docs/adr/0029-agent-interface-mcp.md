# ADR 0029: An agent interface: `signal mcp`, the Model Context Protocol over the UI protocol

- **Status:** Accepted (owner's request of 2026-10-18: asked whether Signal has an interface for live interaction with LLM models, chose "MCP or API?" → the operator's recommendation, then "ok, let there be an MCP that can edit, adjust, practically everything a human could"; design by the operator, delegated). Builds on ADR 0023 (web UI and the UI protocol), 0026 (editor write paths), 0028 (patterns), 0027 (library). Consistent with ADR 0009/0013 (text is the source of truth). Supersedes nothing.
- **Date:** 2026-10-18

## Context
Signal is designed for humans and AI agents working together (`00-vision.md` G1, G9, P2). Agents today work through text files and the CLI: they edit `project.toml`, composites, patterns and panels, a watched `signal ui`/`signal play --watch` applies the change live, and `signal render --json`, `signal modules --json`, `signal ui check` and `signal replay` give machine-readable results. What an agent cannot do without writing its own client is act on a *running* instrument the way a person at the panel does: turn a knob and watch the meter, add a wire in the schematic, edit a pattern while it plays, read the scope.

The UI protocol (`docs/ui-protocol.md`, version 1) already carries every one of those actions for the browser, Signal.app and the tests: `graph`, `modules`, `describe`, `patch`, `set`/`ramp`/`trigger`, `commit`/`unsaved`/`revert`, `panel.*`, `pattern.*`, `schematic.*`, `subscribe` (meters, scopes, vectors), `setup.*`, `stats`, `transport`. Each write path already enforces the rules (base sha and `conflict`, validation before writing, atomic writes, no links, size limits, live application, `.signal` projects marked edited). A second API (REST) would duplicate them. The Model Context Protocol (MCP) is the standard way LLM clients (Claude Code, Claude Desktop and others) discover and call tools with JSON-schema'd arguments.

## Decision
1. **`signal mcp`**: a new subcommand that runs an MCP server over stdio (JSON-RPC 2.0, the MCP tools and resources capabilities; protocol version negotiated per the spec), implemented with the workspace's existing `serde_json` and `tungstenite` (threads, no async runtime); **no new external crate** (if one turns out to be indispensable, that is an operator step under AGENTS.md rule 7).
   - `signal mcp <project>` opens and plays the project itself (as `signal ui` does, same engine, watch and session machinery); `signal mcp --attach <ws-url>` attaches to a running `signal ui` or Signal.app instead, using its token, so an agent and a person can share one running instrument.
   - It is a **client-side adapter**: every tool is implemented by sending UI-protocol messages through the same server code paths, never by a parallel write path. One rule set, one place.
2. **Tools: everything a person can do**, each with a JSON schema and a plain description:
   - *Inspect:* `project_info`, `graph` (any level, with positions), `modules` (std, library, project; categories, formulas, ports, parameter ranges), `describe_instance`, `read_file` (project text files: `project.toml`, composites, patterns, panels, `ui/schematic.toml`), `stats`.
   - *Play and adjust:* `set_parameter`, `ramp_parameter`, `trigger`, `transport` (start/stop/tempo where the project has one), `commit` / `revert` (persist or drop unsaved control changes, as the panel does).
   - *Edit the graph:* `patch` (add/remove instances, connect/disconnect, set initial values, rename; the schematic's patch ops), `schematic_move`, `vendor` (`lib.*` into the project).
   - *Edit text:* `write_pattern`, `write_panel`, `write_file` for project files the UI can write (with `base` sha; `conflict` returns the current text), each validated and applied live.
   - *Observe:* `meter` (levels of chosen outputs over a window), `scope` (a downsampled window of an output as numbers), `read_stream` (sequencer position, step), and `render` (an offline render of the project as it is, returning metrics, hashes and the WAV path, never playing it).
   - *Sessions:* `session_start` / `session_stop` (record what the agent did, ADR 0013), `replay` (check determinism).
   - *Library and checks:* `ui_check`, `check` (load/validate), `library` listing and changelog.
   Results are structured JSON plus a short text summary. Resources expose the project's text files and the docs an agent needs (`module-contract.md`, `project-format.md`, the module list).
3. **Safety and authority:**
   - Local only: stdio, or the UI protocol's loopback WebSocket with its token. No network listener of its own.
   - **Audio output is off unless the person starts the server with `--audio`** (default: the null driver at real-time pace). An agent can never raise a sink's gain above the project's declared range, and the server refuses any `set` that would put an `std.audio_out` gain above a `--max-gain` (default 0.25) unless the person passed a higher one.
   - The engine's rules hold unchanged (real-time rules, magnitude bound, determinism); the agent cannot reach the audio thread except through the same messages a panel sends.
   - Every write is a file change the person can see, diff and undo (text is the source of truth); the MCP server keeps no hidden state.
   - The person's own files are edited only through the same write paths as the UI (base sha, atomic, no links). An optional `--read-only` flag exposes inspect/observe tools only.
4. **Documentation and discovery:** `docs/agent-interface.md` (tools, schemas, examples, safety), `signal mcp --help`, and a ready-to-paste client configuration for Claude Code / Claude Desktop (`claude mcp add signal -- <path>/signal mcp <project>`). Signal.app bundles the same binary (`Contents/Helpers/signal`).
5. **Testing:** an MCP conformance test (initialize, tools/list, tools/call, errors), a test per tool through a real `signal ui` server (both transports), and an end-to-end scripted agent session on a copy of `projects/studio` (adjust a knob, edit a pattern, add a module and a wire, read meters, render) proving every change lands in text and replays bit-identically.

## Alternatives considered
- **A separate REST/HTTP API:** a second write path duplicating the UI protocol's rules; the UI protocol already is the API. MCP sits on it.
- **A built-in chat panel calling a model provider from Signal.app:** needs a provider choice, keys and network permissions; out of scope now. An MCP client (Claude Code/Desktop) gives the same interaction with the person's own account.
- **Agents writing their own WebSocket clients:** possible today, but every agent re-learns the protocol; MCP makes it discoverable.

## Consequences
- A new subcommand and module in `signal-cli` (or a small crate `signal-mcp` in the workspace if it grows), docs, tests; no change to core, graph or engine.
- The UI protocol stays the single source of behaviour; new UI features become agent features by adding a tool.
- Version rule: the MCP tool set is versioned with the UI protocol; a removed or changed tool follows its version rules (`ui-protocol.md` "Version rules").
