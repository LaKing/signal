# The agent interface: `signal mcp`

`signal mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio (ADR 0029). With it, an LLM client such as Claude Code or Claude Desktop can do what a person at the panel does: inspect a running project, turn its knobs, patch its graph, edit its patterns and panels, read its meters, scopes and sequencers, render it offline and record a session. It is a *client* of the UI protocol (`ui-protocol.md`). Every tool that observes or changes the running project sends UI-protocol messages through `signal-ui`'s server code, the same code the browser, Signal.app and the tests use. Every change is a change of the project's text files, which the person can see, diff and undo (AGENTS.md rule 4). The server keeps no project state of its own.

## Running it

```sh
signal mcp projects/kick                         # plays the project, null driver (silent), real-time pace
signal mcp projects/kick --audio                 # plays it on the default output device
signal mcp projects/kick --max-gain 0.5          # output ceiling and gain limit 0.5 (default 0.25)
signal mcp projects/kick --read-only             # inspect, observe and render tools only
signal mcp --attach 'http://127.0.0.1:8800/#ws=8801&token=…' projects/kick   # share a running `signal ui`
```

Options:

- `--out <dir>`: where renders and sessions go. The default is a folder of its own under the system's temporary folder. It is never inside the project.
- `--device`, `--buffer`, `--threads`: as for `signal ui`.
- `--fast`: runs the null driver as fast as it goes, for tests.
- `signal mcp --help` lists every option.

The server ends when its stdin closes (the MCP shutdown). stdout carries protocol frames only, one JSON-RPC message per line; logs go to stderr.

**Claude Code:**

```sh
claude mcp add signal -- <path>/signal mcp <project>
```

**Claude Desktop** (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "signal": {
      "command": "/Applications/Signal.app/Contents/Helpers/signal",
      "args": ["mcp", "/Users/me/Music/my-project"]
    }
  }
}
```

Add `"--audio"` to `args` to hear it. Signal.app bundles the same binary as `Contents/Helpers/signal`. A `.signal` file is unpacked first (`signal unpack`): `signal mcp` plays a project folder.

## How it reaches the project

- **Without `--attach`** it plays the project itself, exactly as `signal ui` does: the same `signal_ui::Host`, watch, sessions and setup, on the null driver at real-time pace unless `--audio`. It talks to that host **in process**: `Host::new_local` (the `signal-ui` feature `in-process`) binds no port, and each in-process connection is served by the same WebSocket connection code as a TCP client, with the WebSocket handshake, `hello` with the session token, the bounded queue and every write path (`ui-protocol.md`). So `signal mcp` opens no network listener of its own. To watch it in a browser at the same time, run `signal ui` and attach to it instead.
- **With `--attach <url>`** it connects to a running `signal ui` or Signal.app over that server's loopback WebSocket with its token:
  - `url` is the page URL `signal ui` prints (`http://127.0.0.1:<http>/…#ws=<port>&token=<token>`), or `ws://127.0.0.1:<port>/#token=<token>`.
  - Only `127.0.0.1`, `localhost` and `[::1]` are accepted.
  - The `<project>` argument must be the folder that server plays: its canonical path's SHA-256 must equal `graph.projectId`. `render`, `check`, `ui_check` and the gain check read that folder.
  - The session tools are not offered when attached (see "Sessions").

## Protocol

The server is *dual-era* in the specification's terms ("Versioning and Compatibility", modelcontextprotocol.io). It serves:

- **2026-07-28**, the current revision. There is no handshake: every request names its version in `_meta` (`"io.modelcontextprotocol/protocolVersion": "2026-07-28"`), and `server/discover` answers the supported versions, capabilities and server identity. A request naming another version gets error `-32022` (`UnsupportedProtocolVersionError`) with `data.supported` and `data.requested`. Results carry `"resultType": "complete"`.
- **2025-11-25** and **2025-06-18**, the handshake-based revisions. `initialize` with `protocolVersion` negotiates one of them for the process; any other requested version is answered with 2025-11-25.

Methods:

- `initialize`, `server/discover`, `ping`.
- `tools/list`, `tools/call`.
- `resources/list`, `resources/read`.

Notifications (`notifications/initialized`, `notifications/cancelled`) get no reply. A request is answered before the next one is read, so a cancellation always comes too late and is ignored.

JSON-RPC errors:

| Code | When |
|---|---|
| -32700 | The line is not JSON |
| -32600 | The message is no JSON-RPC 2.0 request; a request before `initialize` that names no version in `_meta` |
| -32601 | Unknown method |
| -32602 | Unknown tool (including one `--read-only` hides); `tools/call` without a `name`, or with `arguments` that are no object |
| -32022 | Unsupported protocol version |
| -32002 | Unknown resource |

Arguments that do not fit a tool's schema are a *tool* error (`isError: true`, "invalid arguments: …"), as the specification asks for input validation, so the model can correct them. A missing `arguments` is taken as `{}`.

**Results.** A tool's result has `structuredContent` (the JSON object the tables below describe) and `content`: a short text summary, then the same JSON as text when it is at most 32 KiB (a larger result is only in `structuredContent`; see "Limits"). A refusal is `isError: true` with the reason as text. When the UI protocol refused, `structuredContent.error` holds its error: `code`, `message`, and for a `conflict` the current `base` and `text`, for `does-not-load` the `diagnostics`.

## Tools

Addresses are as in the UI protocol: `/instance` or `/composite/inner`, and ports `/instance.port`. `base` is a file's SHA-256 (hex), as `graph.files`, `read_file` and every write give it.

### Inspect

| Tool | Arguments | Result |
|---|---|---|
| `project_info` | — | `project`, `dir`, `sampleRate`, `playing`, `frame`, `planVersion`, `mode` (`in-process` / `attached`), `driver` (`null` / `coreaudio`), `maxGain`, `outputCeiling` (the level every device sample is held within; null attached), `readOnly`, `writerLock` (whether the folder's file system gives the project's writer lock now; `false`: Signal's writes go on unlocked, each still compared just before its rename, ADR 0040; null when the folder cannot be opened), `out`, `files` (each file's base), `instances`, `controls` (`describe.controls`: range, default, live `base`, `inFile`), `sinks`, `unsaved`, `gain`: each `std.audio_out` gain `{port, file, live, modulated, aboveCeiling}` |
| `graph` | `level?` (`""`, a composite) | `graph` as written (instances, ports, parameters, connections with converters, composites, positions, `files`); with `level`, that level only |
| `modules` | `category?`, `query?` | `primitives` and `composites` (the project's, then `lib.*`) as the schematic's palette lists them, filtered |
| `describe_instance` | `address` | the plan instances at or inside it, its controls, its `graph` entry |
| `read_file` | `file` | `project.toml`, `composites/<name>.toml` (`file.read`), `patterns/<name>.toml` (`pattern.read`, with `kind`), `ui/<name>.html` (`panel.read`, with the `<signal-panel>` `html`), `ui/schematic.toml`: `{exists, base, text}` |
| `snapshot_list` | — | `snapshot.list`: `base` (the SHA-256 of `snapshots.toml`, null without one), `snapshots`, each `{name, count, values}`, and the undo snapshot apart: `undo` (whether there is one), `undoCount`, `undoValues` (ADR 0041) |
| `stats` | — | `stats` (frame, clients, drops, overflows, clipped, `ceiling` and `ceilinged`: the output ceiling and the device samples it held, writes) |

### Play and adjust

| Tool | Arguments | Does |
|---|---|---|
| `set_parameter` | `address`, `value` | `set`, answered once applied: `{frame, requested, late, outcome}` |
| `ramp_parameter` | `address`, `to`, `ms?` or `frames?` | `ramp` (default 5 ms) |
| `trigger` | `address` (an event input) | `trigger` |
| `transport` | `action`: `start` / `stop` / `tempo`; `bpm`, `ms?` | `transport.play` (plays again from frame 0 from the files; refused, as `revert` is, when that would raise a lowered gain above `--max-gain`) / `transport.stop`; `tempo` sets or ramps the tempo (below) |
| `commit` | `address`, `value?` (default: the live value; `null`: remove the key) | `commit`: writes the control's value into `project.toml` |
| `revert` | `address` | `revert`: each port gets the file's value again |
| `snapshot_save` | `name`, `overwrite?` (false) | `snapshot.save`: every control's live base under `name` in `snapshots.toml` (1 to 64 characters, trimmed); a name in use is refused (`exists`) unless `overwrite` |
| `snapshot_restore` | `name` | `snapshot.restore` with the `base` the tool listed (it writes the undo snapshot first): the snapshot's values written into `project.toml` as one change and sent live, as `commit` does; `{restored, written, sent, skipped, clamped, note}`. Checked by the gain guard as a `commit` of each value ("Safety") |
| `snapshot_undo` | — | `snapshot.undo`: the values the last restore (or undo) replaced, restored as a restore is (a second undo redoes); gain-guarded as `snapshot_restore`. The guard judges every value the snapshot holds for a control, even one left out of snapshots (which the restore skips), so a `snapshot.exclude` by another client between the check and the restore can only narrow what is restored. In consequence (the operator's decision: safer) a snapshot that holds a value above the ceiling blocks `snapshot_restore` and `snapshot_undo` even when that control is left out of snapshots; put it back in and lower the value, or delete the snapshot. `snapshot_list.undo` says whether there is one |
| `snapshot_exclude` | `address`, `exclude?` (true) | `snapshot.exclude`: the control left out of snapshots, or put back (false); `project_info.controls[].inSnapshots` shows it |
| `snapshot_move` | `name`, `to` (0-based) | `snapshot.move`: the snapshot to place `to` in the list's order (past the end: the last place); replies with the list in its new order (ADR 0041 §9) |
| `snapshot_delete` | `name` | `snapshot.delete` |

**The tempo** (`transport` `tempo`) follows the rule Space's `transport.toggle` follows for `run` (ADR 0039 §6, `ui/lib/transport-toggle.js`), for `bpm`: its targets are the top-level controls (`describe.controls`: a top-level instance's parameter, or a composite's export such as `lib.transport`'s `/transport.bpm` on studio-transport) whose ports reach the `bpm` of a `std.transport` or a `std.playhead` (`lib.transport` is one), skipping a control a connection drives (`modulated`: a value set by hand would not be what plays). An included project's transport (ADR 0035) is reached only when `describe.controls` lists a top-level control for its `bpm`; two-studios' includes declare no `[exports]` and have none, so there the tempo is refused, naming both (tested). **Several** (two transports in one project) are all set, or ramped, to the same `bpm` **on the same frame**, so they stay in step (a playhead integrates its `bpm`, so changes a block apart would leave a lasting phase offset): every request carries the same `at`: the current frame plus 50 ms for each target, plus once more 50 ms or the controller's latency (`pong.latency`, two device buffers) when that is longer, so a large device buffer does not make the later targets late. The reply: `addresses` (the controls set), `at`, `acks` (each with its `frame` and `requested`), `late` (any applied after `at`, so out of step: also said in the summary), `skipped` (the wired ones) and `unreached` (bpm ports no top-level control reaches, such as a parameter inside a composite that does not export it). Each is a live `set` (or `ramp`) as `set_parameter` sends it, under the same gain guard; `commit` writes it into `project.toml`. Refused when nothing is left: no transport ("the project has no transport"), or "no tempo to set: " naming the wired ones ("a connection drives …") and the unreached ones ("no top-level control reaches …").

### Edit the graph

| Tool | Arguments | Does |
|---|---|---|
| `patch` | `ops`, `level?` (`""`), `base?` (default: the file's current one) | `patch` with the schematic's ops: `instance.add` {name, module, params?, x?, y?}, `instance.remove` {name}, `connection.add` {from, to, scale?}, `connection.remove` {from, to}, `connection.scale` {from, to, scale}; the reply's `inverse` undoes it |
| `schematic_move` | `instance`, `x`, `y`, or `moves`; `level?` | `schematic.move` (positions only) |
| `vendor` | `composite` (`lib.<name>`), `as?` | `vendor`: copies it, and the library composites it uses, into `composites/` (`signal vendor`) |

### Edit text

| Tool | Arguments | Does |
|---|---|---|
| `write_pattern` | `name`, `text`, `base` (null: a new pattern) | `pattern.write` |
| `write_panel` | `html`, `base`, `file?` (`main.html`) | `panel.write`: the `<signal-panel>` content, within the panel whitelist |
| `write_file` | `file`, `text`, `base` | `project.toml` and `composites/<name>.toml`: `file.write`; `patterns/<name>.toml`: `pattern.write`. Panels and positions have their own tools |

Every write is checked the way a reload would load it, and is written atomically, never through a link, and only when the file is still the version `base` names, checked and written under the project's writer lock so that another Signal or `signal mcp` process on the same project cannot write in between (ADR 0040). Otherwise the refusal is a `conflict` carrying the current `base` and `text`. The watch then applies the file live.

Example:

```json
{"name": "write_file", "arguments": {"file": "project.toml", "text": "schema_version = 1\n…", "base": "3f2a…"}}
→ {"structuredContent": {"file": "project.toml", "base": "9c41…", "created": false}, "isError": false, …}
```

### Observe

A scope stream delivers blocks of 256 consecutive points, so a window that reads scope streams is at least 256 / rate seconds. A shorter one is refused with the minimum.

| Tool | Arguments | Result |
|---|---|---|
| `meter` | `source?` (`output`, `input`), `window_ms?` (500) | the sink's channels: `peak`, `rms` (linear, and `peakDb`, `rmsDb`) over the window, from the `meters` stream |
| `meter` | `ports` (≤ 8), `window_ms?` (500, ≥ 54) | each signal output's peak and RMS from a scope stream at 4800 points/s: a decimated capture, so its peak can read below the true sample peak |
| `scope` | `port`, `rate?` (4800), `window_ms?` (200), `points?` (256, ≤ 1024) | the window as numbers, every `every`-th point, with `firstFrame`, `decimation`, `peak`, `rms` |
| `read_stream` | `ports?`, `rate?` (512), `window_ms?` (1000) | the sequencers' playheads: every `std.step_seq8` `step` (0–255, as `step`) and `std.note_seq` `position` (beats), or the outputs named. Each with `first`, `last`, `min`, `max`, `lastFrame`; also the `frame` and `transport`: each `std.transport` and `std.playhead` as `{instance, control, wired, bpm, planBpm}`: `control` the top-level control `tempo` sets for it (null when none reaches it), `bpm` that control's live value (with no control, the plan's), `wired` a control that reaches it but that a connection drives (`modulated`, which `tempo` skips; then `control` and `bpm` are null: what plays is what the connection makes of it, not a value read here), `planBpm` the plan's base, the files' value, which a live change does not move until it is committed |
| `render` | `seconds?` (2, ≤ 600), `seed?` | an offline render of the files as they are now (`signal render`; unsaved live values are not in it), under `--out`: `wav`, `hashes` (`audio_fnv1a64`, `log_fnv1a64`), `manifest`, `metrics`, `warnings`, `played: false`. It never plays, and it is the project as written: the output ceiling is not applied (`outputCeiling: null`), so its samples can be louder than anything the device gets |

The 8 scope slots of a run are shared with every client: a full set refuses `no-scope-slot`, and the tool says so. An observation that receives no data (nothing plays, or the stream delivered nothing within the window plus 5 s) is a refusal, "no data arrived", never an empty result. The meters measure what the device gets, after the output ceiling.

### Checks and library

| Tool | Result |
|---|---|
| `ui_check` | `signal ui check`: `{ok, panels, generated, diagnostics}` |
| `check` | the files loaded and planned: `{ok, diagnostics, warnings}` |
| `library` | the built-in library's composites (category, description, hash, ports, parameters), its `version` and `changelog` |

### Sessions

The session tools are offered only without `--attach`: a recording covers one run from frame 0, and only the process that owns the host can restart it.

| Tool | Does |
|---|---|
| `session_start` (`discard?`) | Plays the project again from frame 0, recording a session (`session-format.md`). It is refused while parameters are unsaved, unless `discard`, and, as `revert` is, when playing from the files would raise a lowered gain above `--max-gain` ("Safety") |
| `session_stop` | Ends the recorded run and writes `<out>/session-<n>/session/` (the session directory) and `audio.wav` beside it: the samples the null driver played, which the session format does not hold, as the device got them (held within the output ceiling). Returns their FNV-1a 64 as `capturedFnv1a64`, and the `ceiling` (also in `captured.json`). With `--audio` the samples were the device's, so `wav` and `capturedFnv1a64` are null. The project then plays again, not recording |
| `replay` (`session?`, default: the last one) | `signal replay` of it into `<out>/replay-<n>/`. The replay is the project's audio (`audio_fnv1a64`, unheld); for a session `session_stop` wrote with a ceiling, its samples are held the same way (`heldFnv1a64`). `identical` is true when the replay succeeds and the held hash (without a ceiling, `audio_fnv1a64`) equals `capturedFnv1a64` |

## Resources

`resources/list` gives:

- the project's own text files, as `signal://project/<path>`: `project.toml`, `composites/*.toml`, `patterns/*.toml`, `ui/*.html` and `ui/schematic.toml`. A file that is a link, or in a `composites/`, `patterns/` or `ui/` folder that is one, is never listed nor read, as `read_file` refuses it;
- the module catalogue, `signal://modules`;
- the docs an agent needs, as `signal://docs/<name>`: this file, `module-contract.md`, `project-format.md` and `ui-protocol.md`.

`resources/read` returns their text. A project URI must name one of the files listed.

## Safety

- **Local only.** It talks over stdio, or with `--attach` over the loopback WebSocket with its token. It opens no listener.
- **No sound unless `--audio`.** Without it the null driver runs at real-time pace and nothing reaches a device.
- **The output ceiling (the guarantee).** Without `--attach`, every sample `signal mcp`'s own host hands to the device is held within ±`--max-gain` (default 0.25, from 0 to 4; above 1 it is the device's own range, ±1). The audio adapter does it in the real-time path, after the sink's gain and the conversion to `f32`, whatever the project, a patch, a connection's scale or a modulation does (`signal_output::Runner::set_ceiling`, output policy per ADR 0022; no other command sets one). Held samples are counted (`stats.ceilinged`), and the meters, `meter` and a session's captured audio measure the held output. What is guaranteed is the device output's level, not the project's: the project still computes what it computes, the offline `render` is the project as written (unheld), and `scope`/`read_stream` read the plan's signals before the sink.
- **The parameter guard (an early refusal).** On top of the ceiling, no tool may cause a `std.audio_out` gain above `--max-gain`: such a change is refused with a clear error before anything is sent, so the agent learns why instead of hearing a held, clipped output. The value the protocol sends is also clamped to the gain's declared range.

  A sink's effective gain is `clamp(base · m, 0, 4)`, where `m` is the sum of what is connected into its gain, or the base alone when nothing is (`semantics.md` "The effective value"). Nothing bounds `m` but that clamp, so a connected ("modulated") gain has no known effective value. For every gain port, a change is allowed only if:

  - **R1, static after:** the new live base is at most `--max-gain`, or the port was static before too and the new live base is not higher than the old.
  - **R2, modulated after:** `--max-gain` is 4, or the port was modulated before, with the same connections, scales and fan-in (every instance with a path to it, their parameters, connections and pattern data), and the new base is not higher.

  The live base before a change is the one the runner last reported (a `parameters` stream). The base after:

  - for `set`, `ramp` (its target), `commit`, `revert`, `snapshot_restore` and `snapshot_undo` (each value of the snapshot, or of the undo snapshot, of a control in snapshots, clamped to its control's range), the value it sends;
  - for an edit of the files (`patch`, `write_file`, `write_pattern`, `vendor`, checked in memory on the text at `base`), the file's new value when it changes or the port is new, otherwise the live base. A reload sends a parameter only when its file value changes (`ui-protocol.md` "Unsaved"). An absent `gain` key is the declared default, 1.0;
  - for playing again from the files (`transport` start, `session_start` with `discard`), the file's value of every unsaved parameter, as for `revert`: a lowered live gain is not raised that way. Commit the lowered value first.

  More rules:

  - **Fail closed.** When the project's files do not load and plan as they are now (a person or another tool mid-edit, while the last good version plays), every tool that changes something is refused (`does-not-load`, with the diagnostics), since no change can be checked (the files are loaded from the project folder, so the projects they include are found as `signal play` finds them); so is an edit that would leave them not loading, a `patch` this server cannot simulate, a `vendor` it cannot prepare, and a `patch` whose `base` is not the file's current one (`conflict`).
  - Removing an instance that feeds a gain leaves its modulation held, and counts as modulated: remove the connection first.
  - A `trigger`, or a `set` of anything in a modulated gain's fan-in, changes that gain's modulation and falls under R2.
  - The UI protocol's `note` (ADR 0039: a live note held on a sequencer's `audition`, the piano roll's keys) is **not a tool**: it plays a note that sounds until its note off, and a held note's gain cannot be judged by these rules beforehand (it is a new source of modulation in every fan-in the sequencer reaches). An agent that wants a note heard edits a pattern (`write_pattern`), which is checked; a `trigger` into `audition` (which only ends a held note) is allowed as any `trigger` is.
  - `commit` also refuses to write an over-ceiling value that the file did not already hold, whatever the live value; so does `snapshot_restore`, for each gain the snapshot holds.
  - A call made while a reload is pending (the files differ from what plays) is refused: try again.
  - `write_panel` cannot reach a parameter and is not checked.
  - When attached, the person can change values between the check and the change. The check judges what the agent's own change does.

  The project as the person wrote it is played as it is, even with a gain above the ceiling (`project_info.gain` says `aboveCeiling`); without `--attach` its output is still held.
- **Attached** (`--attach`), the running server's output is its own: `signal mcp` sets no output ceiling there, and only the parameter guard applies. Every guard, `write_panel`, `schematic_move` and `transport` first check that the server still plays the folder named (`graph.projectId`; Signal.app can open another project meanwhile) and refuse otherwise.
- **Changes outside `signal mcp`.** The guard covers what this server's tools do. An agent that also edits the project's files with tools of its own (a file editor) changes them as a person would: the watch applies those edits live and no guard sees them. The output ceiling of an in-process `signal mcp` still holds the device output.
- **The person's files** change only through the UI protocol's write paths: `commit`, `patch`, `schematic.*`, `panel.write`, `pattern.write`, `file.write`, `vendor` and `snapshot.*` (ADR 0041: `snapshots.toml`, and a restore's `project.toml` as `commit` writes it). Each checks the base, writes atomically, never writes through a link and validates before writing. `file.write` and `vendor` were added for this interface by the operator's decision of 2026-10-18: `file.write` uses `patch`'s helpers, and `vendor` uses `signal vendor`'s writer with the host's checks. Renders, sessions and replays are written only under `--out`.
- **`--read-only`** offers only the inspect, observe and render tools: `project_info`, `graph`, `modules`, `describe_instance`, `read_file`, `snapshot_list`, `stats`, `meter`, `scope`, `read_stream`, `render`.
- **The engine's rules are unchanged**: real-time rules, the magnitude bound, determinism. The agent reaches the audio thread only through the messages a panel sends.

## Limits

- A UI-protocol client message is at most 64 KiB, so a write of a larger text is refused before it is sent.
- A tool's `structuredContent` is at most 256 KiB of JSON. A larger result (the `graph` of a project with thousands of voices, say) keeps its smallest fields whole and replaces each larger one with `{"omitted": true, "bytes", "items"}`, and says `truncated: true`, its full size (`bytes`), the `limit` and `guidance` (`graph`: ask for one `level`; `modules`: `category` or `query`; `describe_instance`: a narrower address). The JSON is repeated as text in `content` only up to 32 KiB; above that the text is the summary alone.
- No reconnect: if the connection to the UI server closes (attached, the server stopped or restarted, or evicted this client), every tool answers "the connection to the server is closed". Restart `signal mcp`.
- After an engine failure (`semantics.md` §14; only an internal error stops the engine, a module's panic is contained), the run plays nothing until `transport` start plays the project again; the control loop then plays the new run. This recovery has no automated test: no module of a production registry can make the engine itself fail.
- The version rule: the tool set is versioned with the UI protocol (version 1). Removing or changing a tool follows its rules (`ui-protocol.md` "Version rules").

## Tests

- `crates/signal-cli/tests/mcp.rs`: conformance in both eras, one test per tool (the seven snapshot tools in `snapshots`), read-only mode, `--help`, the AC11 session on a copy of `projects/studio`, and `attach_to_a_running_signal_ui` (loopback).
- `crates/signal-cli/tests/mcp_gain.rs`: the gain guard, each path and each rule; failing closed when the files do not load; restarts; the device output held at the ceiling after a ×20 patch.
- `crates/signal-cli/src/mcp/` unit tests: the rules, attach URLs, the result cap, the project identity.
- `crates/signal-output/tests/ceiling.rs`: the output ceiling at the adapter (null driver), its meters and counts, renders never held; `crates/signal-audio/tests/no_allocation.rs`: the callback with a ceiling allocates nothing.
- `crates/signal-cli/src/drive.rs`: `--seconds` after a client's stop.
- `crates/signal-ui/tests/file_write.rs`: `file.read`, `file.write`, `vendor`.
- `crates/signal-ui/tests/app_mode.rs`: `signal_ui_stops_and_plays`.

The `signal-ui` tests run on both transports. The TCP ones and the attach test need a loopback port.
