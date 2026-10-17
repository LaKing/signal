# The UI protocol (version 1)

The interface through which a web page describes, observes and edits a running project (ADR 0023 §1). Signal.app's WebView, a browser and later remote devices all use it. The server is the crate `signal-ui`; `signal ui <project>` runs it (see "Running it").

## Servers

Two servers, both bound to **127.0.0.1** by default:

| Server | Port | Serves |
|---|---|---|
| HTTP (a small `std::net` server, `crates/signal-ui/src/http.rs`) | `--port N` (a free one without it, or with 0) | Static files only (below) |
| WebSocket (`tungstenite`) | a free port | The protocol |

They are separate: each WebSocket client has a thread that both waits for its messages and writes the frames queued for it, while the HTTP server serves one request per connection from a fixed pool (below). (Until M2.8-8 the HTTP server was `tiny_http`, whose upgraded connection could not be split either.)

**No client can block the HTTP server (M2.8-8).** An accept thread hands each connection to a queue of 32 (more are closed at once), and a fixed pool of **4 workers** serves them, one request per connection (`Connection: close`, with `Content-Length`). A connection holds one worker at most: it has **2 s** to send its request head (at most 8 KiB; otherwise 400), and a response is streamed in 64 KiB chunks, never read whole into memory; a client that takes nothing for **3 s** is dropped and its worker freed. Every wait is a poll of 100 ms that looks at the stop flag, so dropping the server (`signal ui` ending, Signal.app's window closing or quitting) returns within a few polls whatever its clients do. A client that keeps reading, however slowly, keeps its worker. Tests: `crates/signal-ui/src/http.rs` (deadlines, stop, streaming, without a socket) and `crates/signal-ui/tests/http.rs` (a client that stops reading a 32 MiB asset; another request answered within 1 s; dropping the server within 2 s; the deadline; over TCP and in process).

**The URL to open** (printed by `signal ui`, opened by `--open`):

```
http://127.0.0.1:<http port>/#ws=<websocket port>&token=<session token>
```

The fragment is never sent over HTTP, so the token is in no request, Referer or server log. A page reads both values from `location.hash` (`parseFragment` in `ui/lib/protocol.js`). `signal ui` also prints the project's main panel with the same fragment, `http://127.0.0.1:<http port>/project/main.html#ws=…&token=…`, and `--open` opens that one.

### Static files

| Path | File |
|---|---|
| `/` | the repository's `ui/index.html` (the raw protocol test page) |
| `/ui/<path>` | the repository's `ui/<path>` (the UI library; `signal ui --ui-dir` names another folder) |
| `/schematic` | the repository's `ui/schematic.html`: the schematic, `<signal-schematic>` alone filling the page (M2.9-1; also at `/ui/schematic.html`). `signal ui` prints its URL with the same fragment; it is its own page rather than a tab of the generated panel, so it fills the window |
| `/project/<path>` | the project's `ui/<path>`: its panels, and `ui/assets/` for their SVG and PNG files. `/project/main.html` of a project without `ui/main.html` is the generated panel, the repository's `ui/default-panel.html` (`ui-controls.md` "Panels") |

Only `GET` and `HEAD` (else 405); a response to `HEAD`, a refusal (403, 404) included, is the head alone, with the length the body would have. A path with `..`, `%`, `\`, NUL, an empty segment or a segment starting with `.` is not served, nor one that resolves (links followed) outside its folder; there are no directory listings (404). Content types: `.html` `text/html; charset=utf-8`, `.js` and `.mjs` `text/javascript; charset=utf-8` (ES modules), `.css` `text/css`, `.json` `application/json`, `.svg` `image/svg+xml`, `.png` `image/png`, `.jpg` `image/jpeg`, `.gif`, `.webp`, `.woff2`, `.txt`; anything else `application/octet-stream`. Every response carries `X-Content-Type-Options: nosniff`, `Cache-Control: no-cache` and `Referrer-Policy: no-referrer`.

## Messages

Control messages are JSON text frames, each an object with a `type`. A request may carry an `id` (any JSON value); its reply carries the same `id`. Data streams are binary frames (below). Messages without an `id` are pushed by the server.

### `hello` (the first message)

```json
{"type": "hello", "protocol": 1, "token": "<session token>"}
```

The first message on a connection must be `hello`, with the session token and protocol 1, within 5 s of the connection's acceptance: one deadline for the WebSocket handshake and `hello` together, however slowly their bytes arrive (see Clients below). Then the server answers

```json
{"type": "welcome", "protocol": 1, "client": 3, "project": "kick", "sampleRate": 96000, "frame": 123456, "planVersion": 0}
```

(Signal.app's adds `"power": "on"` or `"off"`, its power switch: "Signal.app" below.)

Otherwise it sends `{"type": "error", "code": "unauthorized", …}` (a wrong or missing token, another first message, not JSON) and closes the connection with code 1008 (policy), or `{"type": "error", "code": "protocol", …}` (another version) and closes it with 1002. Nothing else is answered on that connection, and the control thread never hears of it. A later `hello` is an error (`already-authenticated`).

### `describe`

`{"type": "describe", "id": 1}` → the running project:

```json
{"type": "describe", "id": 1, "planVersion": 0, "sampleRate": 48000, "project": "tone", "playing": true,
 "instances": [{"address": "/a", "module": "std.sine_osc", "sink": null,
   "ports": [{"name": "out", "address": "/a.out", "direction": "out", "shape": "signal", "rate": 1, "kind": "audio", "unit": null, "range": null}],
   "parameters": [{"name": "freq", "address": "/a.freq", "kind": "frequency", "rate": 1, "unit": "Hz", "range": {"min": 0.0, "max": 24000.0}, "default": 440.0, "combine": "add", "base": 440.0}]}],
 "controls": [{"address": "/out.gain", "instance": "out", "parameter": "gain", "module": "std.audio_out", "ports": ["/out.gain"],
   "unit": null, "min": 0.0, "max": 4.0, "default": 1.0, "base": 0.5, "inFile": true, "modulated": false, "combine": "multiply"}],
 "sinks": [{"index": 0, "address": "/out", "kind": "audio", "inputs": 2}],
 "meters": {"output": [{"channel": 0, "device": 0}, {"channel": 1, "device": 1}], "input": []},
 "scopes": {"slots": 8, "free": 8, "sinceFreedMs": null, "points": 256, "maxRate": 48000}}
```

(Values here are illustrative.) `instances` is the flattened plan: every primitive instance with its ports and parameters (kind, unit, range, default, combine, and the parameter's initial base in the plan). `controls` are what the parameter panel edits: the parameters of the project's top-level instances, a composite's exported ones included, at the address `/<instance>.<parameter>`, with the plan ports a value goes to (an export can fan out to several), the range they share, the value without a key in the file (`default`), the base (`base`: the live value, the last one the Controller took for every port, even if a runner refused it afterwards; the file's value or the default when nothing else was sent) and whether the file sets it. `unsaved` lists the parameters whose live value the file does not hold ("Unsaved", below). `planVersion` is the running engine's plan version; `meters.output` lists the sink's channels, the device channel each plays on, and its `label` (`out 1 → 3`, or `out 2 (not played)`).

The server pushes `describe` (without `id`) when a plan swap commits, after a reload that changed values, after a `commit` (or `retry`) is written and after a `revert`, and after `setup.apply` played again (then with `"restarted": true`).

### `graph`

`{"type": "graph", "id": 1}` → the project's structure **as written, not flattened** (ADR 0026 §1), for the schematic (`ui-controls.md` "Schematic"):

```json
{"type": "graph", "id": 1, "project": "kick", "planVersion": 0,
 "top": {
   "instances": [{"name": "drums", "address": "/drums", "module": "drums", "composite": true,
     "ports": [{"name": "trigger", "address": "/drums.trigger", "direction": "in", "shape": "events", "rate": 1, "kind": "trigger", "unit": null, "range": null}, …],
     "parameters": [{"name": "kick_decay", "address": "/drums.kick_decay", "kind": "control", "rate": 1, "unit": "ms", "range": {"min": 0.1, "max": 60000.0}, "default": 400.0, "combine": "add"}]}, …],
   "connections": [{"from": "/lfo.out", "to": "/drums.kick_decay", "scale": 120.0, "converters": []}, …]},
 "composites": {
   "drums": {"inputs": [{"name": "trigger", "to": ["kick.trigger"], "direction": "in", "shape": "events", …}],
             "outputs": [{"name": "out", "from": "kick.out", "direction": "out", "shape": "signal", …}],
             "parameters": [{"name": "kick_decay", "to": ["kick.decay"]}],
             "instances": [{"name": "kick", "address": "kick", "module": "kick", "composite": true, …}],
             "connections": []},
   "kick": {…}},
 "layout": {"file": "ui/schematic.toml", "positions": {"": {"drums": {"x": 120, "y": -40}}, "kick": {"osc": {"x": 0, "y": 30}}}, "error": null}}
```

- `top`: the project's instances, each with its `module`, whether it is a `composite`, its ports (as in `describe`: direction, shape, kind, rate divisor; a composite's are its exported inputs and outputs, with the shape of what they reach) and its parameters (a composite's: its exported ones); and its connections (`from`, `to` as written, with the leading `/`), each with `converters`.
- `converters`: the converters the compiler inserted on that connection (ADR 0005, `semantics.md` §16), each `{"name", "module", "conversion", "fromRate", "toRate"}`: `name` the converter instance's name (`out_d800`), `module` its type (`core.decimate.d1.d800.signal.control`). It is empty when there is none. **A connection between two primitives has at most one entry; a connection to an exported input or parameter that fans out to several ports may have several**, one per distinct conversion. Converters are never listed as instances.
- `composites`: every composite the project uses (reached from the top level, nested ones included), by name, in name order: its exported `inputs` (`to`: the inner ports each reaches), `outputs` (`from`), `parameters` (`to`), and its inside as the top level's, with addresses relative to an instance of it (`kick.out` is `/drums/kick.out` in `/drums`). Its connections' converters are those of its first instance; a composite's rates are the same in every instance.
- **Order:** instances and connections in source order (the file's), composites by name; the same project always gives the same reply.
- `layout`: the positions of `ui/schematic.toml` (`project-format.md` §11) by level (`""` the top level, else the composite's name), read with every `graph`; `error` says why the file was ignored (it is never a reason not to answer).
- `planVersion` is the running plan's (null while Signal.app is stopped).
- `files` (M2.9-3): `{"": "<sha-256 of project.toml>", "<composite>": "<sha-256 of composites/<composite>.toml>", …}`, every file the project has, read with every `graph`: the `base` a `patch` names.
- `projectId` (M2.9-3): which project folder this is, opaque: the SHA-256 of the folder's canonical path (the path itself is not sent). It tells apart projects whose folders and names are the same; the schematic keeps its undo history for one `projectId` only (`ui-controls.md` "Editing the patch").

Playing, `graph` describes the running project (the last version that loaded, as `describe`); stopped (Signal.app), the folder as it is now. A folder that does not load is `{"type": "error", "code": "does-not-load", "message": <its diagnostics, one per line>}`; Signal.app without a project open answers `no-project`.

**Cached** (M2.9-1b): the structure is built once per running plan version, and stopped once per state of the folder (each file's modification time and length, `ui/schematic.toml` aside), which opening the project and playing it record from the load they already do; the positions are read with every `graph`. So a `graph`, a `schematic.move` (one write, no load), `app.power`, `transport.stop` or `project.open` loads nothing the folder did not change for. The load is counted by a test hook (`Host::graph_counts`).

**Pushed** (without `id`) after every plan swap (once it commits, beside `describe`), and whenever the `graph` it would give differs from the last pushed (compared by a hash of its text, serialised once for every client): after a reload of the folder (an edit of `ui/schematic.toml` included), after `transport.*`, `project.open`, `app.power` and `setup.apply` playing again, after each `schematic.move` and `schematic.reset`, after a `commit` or `retry` (their write changes `files`), and after a `patch` (stopped at once, playing once its reload swaps). Stopped, Signal.app watches the folder every 100 ms for the same push; switched off it watches nothing, and switching on pushes what changed meanwhile. When the folder stops loading, the push is `{"type": "graph.error", "folder": <the project folder's name>, "code": "does-not-load", "message": <its diagnostics, one per line>}` (`no-project` without a project), once, and the schematic keeps its last drawing, marked stale, if it is of the same folder (`graph` carries `folder` too; another folder's error clears the drawing); the next `graph` that loads is pushed again. Tests: `crates/signal-ui/tests/schematic.rs` (kick, pulse, physical-av's converter, the push after a structural reload, positions, the structure built once per plan version) and `crates/signal-ui/tests/app_mode.rs` (stopped, does not load, switched off, the cache, `graph.error`, a `schematic.move` on 4096 voices), both transports; `crates/signal-project/tests/structure.rs`. A 4096-voice project's `graph` is about 6 MB, more than a client's 4 MiB write buffer; it is sent as one message larger than the limit ("Delivery: slow and gone clients").

### `schematic.move`, `schematic.reset`

```json
{"type": "schematic.move", "id": 8, "level": "", "instance": "drums", "x": 120.4, "y": -39.6}
{"type": "schematic.move", "id": 11, "level": "", "moves": [{"instance": "drums", "x": 120, "y": -40}, {"instance": "lfo", "x": 300, "y": 0}]}
{"type": "schematic.reset", "id": 9, "level": "kick"}
{"type": "schematic.reset", "id": 10}
```

`schematic.move` writes instance `instance` of `level` (`""` the top level, else a composite the project uses) at `x`, `y` (finite, at most 1e9 in size, rounded to integers) into `ui/schematic.toml` (`project-format.md` §11), with `toml_edit` and atomically through a hidden temporary file, as `commit` writes; the watch takes the write as seen (no `reload` push), and `graph` is pushed to every client. Reply `{"type": "schematic.moved", "id": 8, "level": "", "instance": "drums", "x": 120, "y": -40, "file": "ui/schematic.toml"}`. The batch form (2026-10-17, a group of boxes dragged together in the schematic) gives `moves`, a non-empty list of `{instance, x, y}` of the same `level`, in place of `instance`, `x` and `y` (any of those beside `moves` is `bad-message`; an instance named twice is `bad-value`): every entry is checked first, as above, and one write holds them all (`set_positions`: the file parsed and written once), so one `graph` is pushed; an entry refused refuses the message and nothing is written. A message is at most 64 KiB ("Limits"), about 1500 moves of short names: the schematic splits a larger group into as few `moves` messages as fit (`moveMessages` in `ui/lib/schematic-select.js`), each its own write and `graph` push (positions are presentation only, so a group need not be written at once), and keeps the boxes of the messages not yet answered where the drag left them meanwhile. Its reply is `{"type": "schematic.moved", "id": 11, "level": "", "moves": [{"instance": "drums", "x": 120, "y": -40}, {"instance": "lfo", "x": 300, "y": 0}], "file": "ui/schematic.toml"}`, the coordinates rounded, in the message's order. Tests: `crates/signal-ui/tests/schematic.rs` (both forms, both transports). Errors: `unknown-level`, `unknown-instance`, `bad-value` (not a finite number, or too large), `bad-message`, `does-not-load`, `write-failed` (also when `ui/` or `ui/schematic.toml` is a link: the file is written only in the project's own `ui/`, and nothing is written). `schematic.reset` removes the table of `level` (it must be a level of the project), or without a `level` the whole file; reply `{"type": "schematic.reset", "level", "file"}`, then the `graph` push. Neither changes the project: the file is presentation only.

### `modules`

`{"type": "modules", "id": 4}` → the module types an instance can be added from (M2.9-3, the schematic's add palette, `ui-controls.md` "Editing the patch"):

```json
{"type": "modules", "id": 4,
 "primitives": [{"module": "std.svf", "composite": false, "description": "…",
   "ports": [{"name": "in", "direction": "in", "shape": "signal", "rate": 1, "kind": "audio", "unit": null, "range": null}, …],
   "parameters": [{"name": "cutoff_hz", "kind": "control", "rate": 1, "unit": "Hz", "range": {…}, "default": 1000.0, "combine": "add"}, …]}, …],
 "composites": [{"module": "kick", "composite": true, "description": null, "ports": […], "parameters": […]}]}
```

`primitives` are the registry's types, `composites` the project's (every `composites/*.toml`, used or not), each in name order. A primitive carries its palette group as `"category"` (ADR 0027 §1, e.g. `"filter"`) when its type has one; the key is absent otherwise, and on composites: the schematic groups a composite under `project` itself (and, reserved, a `lib.*` one under `library`; `ui/lib/schematic-palette.js`). A primitive's `description` is its descriptor's `doc`, the rustdoc summary that states its formula (`std.add`: `"Adds its two signal inputs: out = a + b."`), which the palette and the sidebar show (ADR 0027 §6); a composite's is `null`. Ports and parameters are what `graph` gives an instance of the type (a composite's: its exports, with the shape of what they reach) **without `address`**: there is no instance. The registry holds lab types only in a `lab` build. Of the project `graph` describes (playing, the running one; stopped, the folder's, cached with its structure), and the same reply every time. Errors: `does-not-load`, `no-project`.

### `patch`

Patching from the schematic (M2.9-3, ADR 0026 §6): ops written into one level's file **as text**, applied live by the normal reload. Nothing is held anywhere but in the files.

```json
{"type": "patch", "id": 5, "level": "", "base": "<sha-256 of project.toml, from graph.files>", "ops": [
  {"op": "instance.add", "name": "svf", "module": "std.svf", "params": {"cutoff_hz": 800}, "x": 120, "y": -40},
  {"op": "connection.add", "from": "svf.lp", "to": "out.l", "scale": 0.5},
  {"op": "connection.scale", "from": "lfo.out", "to": "drums.kick_decay", "scale": 60},
  {"op": "connection.remove", "from": "transport.beat", "to": "drums.trigger"},
  {"op": "instance.remove", "name": "accent"}]}
```

`level` is `""` (the top level, `project.toml`) or a composite (`composites/<level>.toml`); one patch writes that one file, and `ui/schematic.toml` for positions. `base` is the file's SHA-256 the patch was made against. The ops are applied in order and fail together:

- `instance.add`: `name` (`[a-z][a-z0-9_]*`, unique at the level), `module` (a registry type or a project composite), `params` (optional), `x`, `y` (optional, both or neither: its position, written into `ui/schematic.toml` as `schematic.move` writes it). Written as the last key of `[instances]`, an inline table, its parameters in declared order: `svf = { module = "std.svf", cutoff_hz = 800 }`.
- `instance.remove`: `name`, with every connection to or from it at the level, its parameter values and its position.
- `connection.add`: `from`, `to` (`instance.port`, without a leading `/`), `scale` (optional). The last element of `connections`, on its own line. A connection with the same `from` and `to` as one the level has is `bad-value`.
- `connection.remove`: the first `from` → `to` in source order.
- `connection.scale`: the first `from` → `to`'s `scale`; 1 removes the key.

**The text.** The file is located with `toml_edit` and edited as whole lines; every other byte is kept: comments, order, formatting. A removed entry takes its own lines (its trailing comment included) and the comment lines directly above it (a section, `[instances.x]` or `[[wiring.connections]]`, also one blank line above); never a neighbour's. An entry sharing a line with others takes the separator next to it, never a comment there. Every form a project may be written in is edited: `[instances]` keys, `[instances.x]` sections, dotted keys (also when an instance's lines lie among other entries: each line is removed, and restored, in its place), inline `instances = {…}`; `connections` on one line or many, with or without a trailing comma, empty, as `[[wiring.connections]]` sections (where a new connection is a section, as TOML cannot mix the two), `[wiring]` without `connections` and no `[wiring]` at all (created after the instances: `[wiring]` and `connections = [` with one element per line). A file whose instances are all `[instances.x]` sections gets an `[instances]` table after them for the new inline entry. A connection added to a single-line array starts a new line.

**The server**, before writing anything:

1. takes the level's file from the files the project has (`unknown-level` otherwise; a file or `composites/` reached through a link is refused: `signal ui` writes only the project it serves);
2. refuses a file that changed since `base` with `conflict`, carrying the current `base` (and `level`, `file`);
3. applies the ops (`unknown-instance`, `unknown-module`, `bad-value`);
4. refuses removing an instance that a composite's `[exports]` or a panel binding in `ui/*.html` names (through every instance of the composite; the edited project's when the current one does not load) with `in-use` and `references: [{file, line?, text}]`;
5. loads the edited project in memory with the same validation and compile as a reload, the whole project for a composite's edit, and while playing the same checks a live reload makes (the audio sink kept): `does-not-load` with `diagnostics` (one per line, `file:line:column`);
6. computes the requests that undo and redo the patch (below), with room for every parameter its instances could be committed with meanwhile: a patch whose undo or redo would be larger than a client message (64 KiB) is `bad-value` ("not written: undoing this patch needs a message of N bytes, over the 64 KiB limit"), so every written patch can be undone;
7. refuses a patch that moves a position when `ui/` or `ui/schematic.toml` is a link (`write-failed`: positions are written only in the project's own `ui/`), and reads the level's file again: one that changed since step 2 is `conflict`, as there;
8. writes the level's file atomically (a hidden temporary file, synced and renamed, as `commit` writes), then `ui/schematic.toml` (taken as seen by the watch); if the second write fails, the first file's previous bytes are written back and the patch is `write-failed`.

Playing, the watch's reload then applies the file live (swap, fades, `reload` push) and `graph` is pushed once it swaps; stopped (Signal.app), `graph` is pushed at once. A `.signal` project's working folder then differs from its file: the project is edited.

Reply: `{"type": "patched", "id": 5, "level": "", "file": "project.toml", "base": "<sha-256 of the written file>", "inverse": [ops]}`. Every other client learns of it from the `graph` push.

**Undo and redo.** `inverse` undoes the patch: sent as a patch with the file's newest SHA-256 (the reply's `base`, or a later `graph.files`), its reply's `inverse` redoes it, and so on. The ops of an inverse carry two **inverse-only** fields that the server writes and the page sends back unchanged: `edits`, the exact text edits that restore the bytes (`{"edit": "insert" | "delete", "at": {"item", "offset"}, "text"}` or `{"edit": "cut", "item", "before", "after"}`, each anchored to an entry (`instance:<name>`, `instance:<name>#<n>` for line `n` of an instance written as dotted keys, `connection:<index>`), a table header (`table:<path>`), the connections array (`array`) or the file's start (`file`), so a `commit` between a patch and its undo moves nothing), and on instance ops `layout` (`{"edits", "created"}`: the same for `ui/schematic.toml`; `created` removes the file when the edits leave it empty). An undo restores the file byte for byte, comments and order included; a parameter committed meanwhile is kept. Ops that carry `edits` are not trusted: every position must fall inside the file on a character boundary (an offset that overflows or splits a character is refused), every `delete` and `cut` must name the bytes there, a `delete` stays inside one gap between entries and a `cut` touches no other entry (`bad-value` otherwise), and the change must be exactly what the op's forward rule would make, in one direction or the other, the whole file compared byte for byte; `layout` the same, for the op's instance's position at the op's level. So `edits` can write nothing but what the op means, the comment lines directly above a restored entry and that entry's `gpu` pin, and the load in step 5 checks them.

Errors: `conflict`, `does-not-load`, `in-use`, `unknown-level`, `unknown-instance`, `unknown-module`, `bad-value`, `bad-message`, `write-failed`, `no-project`. Tests: `crates/signal-ui/tests/patch.rs` over both transports (every op in one patch, its undo and redo byte for byte on copies of kick, pulse, physical-av and the kick composite; the errors writing nothing; `in-use` for exports and panel bindings, also while the project does not load; positions refused through a linked `ui/` or `ui/schematic.toml`; a patch heard on the meters after its reload; `modules`; the size refusal; `graph.files` after a commit), `crates/signal-ui/tests/app_mode.rs` (stopped; a `.signal` project edited), `crates/signal-project/src/patch/tests.rs` (every op on every form, undo and redo byte for byte, undo across a commit, duplicates, comments, a neighbour's comment on a shared line, positions, an unreadable `ui/schematic.toml`, the refusals of untrusted edits).

### Schematic level bars

The schematic's level bars (`ui-controls.md` "Schematic") are `scope` streams of the signal outputs on screen (a closed composite's: the primitive output its export reaches), at 7680 points per second. They share the run's 8 scope slots with every other scope of every client (the Panel tab's included), so the schematic takes them in order, the selected instance's outputs first, then by place on screen, and lets a slot go when its port leaves the screen, when it is hidden, when the power is off and when nothing plays. A port it cannot get a slot for shows its bar marked **no slot**, the same size as a live one; while stopped every bar is **idle**.

**Who gets a slot first** (M2.9-1b): panel scopes, then schematic bars. The schematic asks only for the slots the host says are free: `describe.scopes` gives `free` and `sinceFreedMs` (the milliseconds since the free count last rose, null before it ever did), and the host pushes `{"type": "scopes", "slots": 8, "free": 3, "sinceFreedMs": 0}` whenever the free count changes; the latest push wins over `describe`, and the schematic counts its own requests since then against it. It takes a freed slot only 2.5 s after the free count rose, whoever freed it, at its own start too, while a `<signal-scope>` refused a slot asks again every 2 s: a panel scope waiting for a slot gets it first. A slot the schematic gives up and takes again in the same redraw (one port leaves the screen as another comes on) is swapped at once; a port that comes on screen later can wait up to 2.5 s for its bar. The schematic never gives up a slot it holds for a panel scope: one opened after the schematic took every free slot waits until the schematic lets one go (a port leaves the screen, the tab is hidden, the power is off). A refused port (`no-scope-slot` or `busy`) is not asked again within 2 s, and no timer runs while no slot is free: the next `scopes` push says when one is. Tests: `ui/lib/schematic-levels.test.js` (the order, the delays, a lost race), `ui/lib/trigger.test.js` (the scope's retry), `crates/signal-ui/tests/protocol.rs` (the free count, both transports). A slot samples its port at its decimated frames, so a divided-rate output (its value held between its ticks, `semantics.md` §1) has a level too. This limit was accepted for M2.9-1 (operator decision, 2026-10-16); a stream of levels for every port would be a new stream kind.

### `subscribe`, `unsubscribe`

`{"type": "subscribe", "stream": …, "id": 2}` → `{"type": "subscribed", "id": 2, "sub": 5, "stream": …}`; the stream's frames carry `sub`. `{"type": "unsubscribe", "sub": 5}` → `{"type": "unsubscribed", "sub": 5}`. A client's subscriptions end with its connection.

| `stream` | Fields | Frames |
|---|---|---|
| `parameters` | `addresses`: controls or plan parameter ports; `rate`: frames per second, at least 1 (default 30; more than 60 is taken as 60; below 1, not a number or not finite: `bad-rate`) | kind 3, every `1/rate` s: each address's base and effective value, as the runner last reported them (the parameter observation queue: up to 64 parameters per callback, round robin) |
| `meters` | `source`: `"output"` (default, the audio sink's channels) or `"input"` (the Setup's input, metered only); `rate` as above | kind 1, every `1/rate` s once levels arrived: per channel, the peak and the RMS over the callbacks since the last frame |
| `vector` | `port`: a vector output of the running plan (at most 512 elements); `rate`: snapshots per second (default 60; at least 1, else `bad-rate`; at most the port's tick rate, the sample rate over its rate divisor) | kind 4, one per snapshot: the port's elements at one frame, at every frame that is a multiple of `decimation`, a whole number of the port's ticks (`decimation = d · max(1, round(sampleRate / (d · rate)))`); the reply also gives `rows`, `decimation` and the `slot` |
| `scope` | `port`: a signal output of the running plan (`/a.out`), or a signal input (`/out.l`: the sum of its sources, each scaled, as the instance receives it); `rate`: points per second, at least 1 (default and at most the sample rate; below 1, not a number or not finite: `bad-rate`) | kind 2: blocks of 256 consecutive points, one every `decimation` frames, `decimation = max(1, round(sampleRate / rate))`; the reply also gives `decimation` and the `slot` |

**Vector streams** (M2.8-7) come from the vector adapter in `signal-output` (`crates/signal-output/src/vectors.rs`): the audio thread copies the port's elements at each snapshot frame, absolute and on the port's ticks, so a stream depends neither on the callbacks nor on when it was started, into a preallocated queue (a full queue drops the snapshot and counts it, `stats`' `vectorOverflows`). It allocates nothing there (`crates/signal-output/tests/vector_no_allocation.rs`). There are 8 slots per run (`no-vector-slot`); `describe` says so (`"vectors": {"slots": 8, "maxRows": 512}`). After a swap that removes the port the server pushes `{"type": "vector.unbound", "sub": 5, "reason": "absent"}`, and `vector.bound` when it is back. A stream is also unbound, never sampled, while the running plan gives its port a rate divisor its decimation is not a multiple of (`"reason": "decimation"`, M2.8-9: the decimation is checked again in every plan the run swaps to), or before its slot has heard of the running plan (`"unverified"`) (`crates/signal-output/tests/vector_swaps.rs`). A slot reads its port only while the port has exactly the elements the stream was started with, so it never carries another port's rows: when a swap gives the port another number of elements or another rate divisor (its instance retyped), the server pushes `{"type": "vector.reshaped", "sub": 5, "rows": 12, "decimation": 800}` and the stream carries the new width from then on (`crates/signal-ui/tests/protocol.rs`, `a_vector_stream_follows_a_swap_that_changes_its_width`; the adapter and the replay's taps: `crates/signal-cli/tests/physical_av.rs`). Errors: `not-a-vector-output`, `bad-rate`, `busy`. A replay can take the same snapshots offline (`ReplayOptions::vectors`), which is how a recorded session is shown to reproduce them (`crates/signal-cli/tests/physical_av.rs`). In `describe`, a vector port's `shape` is `{"vector": N}` and its `rate` the divisor.

**Scope streams** come from the scope adapter in `signal-output` (beside the audio adapter of ADR 0022): a decimated capture. A point is the port's value at an absolute frame that is a multiple of the decimation, so a stream does not depend on how the frames were split into callbacks or threads. **A scope stream is the output as its readers see it, fades included** (a signal input, the sum of its sources as they are read): it fades in when its instance is added, out when it is suspended or removed, and through the 64-frame fault fade after a fault (`semantics.md` §12, §14). It costs the audio thread a copy per point and allocates nothing there (measured: `crates/signal-output/tests/scope_no_allocation.rs`). **An open block is dropped, not sent, at a discontinuity**: when a swap rebinds the slot, when the slot is started again (a new `subscribe`, or `setup.apply` playing again), and when the points stop being consecutive (frames not run). So up to 255 points are lost there, and every block that arrives is 256 consecutive points. There are 8 slots per run (`no-scope-slot` when they are all in use; how many are free is in `describe.scopes.free` and the pushed `scopes`, "Schematic level bars"). After a swap that removes the port the server pushes `{"type": "scope.unbound", "sub": 5}`, and `scope.bound` when a later swap brings it back. Errors: `not-a-signal-output` (neither a signal output nor a signal input), `bad-rate`, `busy` (too many scope commands outstanding; try again).

### `set`, `ramp`, `trigger`

Live edits through the run's `Controller`, the path the parameter panel takes:

```json
{"type": "set", "id": 3, "address": "/out.gain", "value": 0.25}
{"type": "ramp", "id": 4, "address": "/drums.kick_decay", "to": 300, "ms": 5}
{"type": "trigger", "id": 5, "address": "/accent.trigger"}
```

`address` is a control (to every one of its ports) or a plan parameter port; `trigger` takes an event input of the plan. Values are clamped to the range; a ramp takes `frames`, or `ms`, or the panel's 5 ms. `at` (a frame) stamps the edit; without it the edit gets the default stamp (the current frame plus the latency). Once the runner has reported every command of the edit:

```json
{"type": "ack", "id": 3, "frame": 12544, "requested": 12544, "late": false, "outcome": "scheduled"}
```

`frame` is the **effective frame** (the latest of its ports'), `requested` the stamp's; `late` when it applied after it was requested. `outcome` is `refused` when the runner refused a command, and `abandoned` when the run stopped first (`setup.apply`). A command the Controller cannot take now is answered with `{"type": "error", "code": "refused", …}`. Errors: `unknown-address`, `bad-value`, `not-playing`.

### `commit`

```json
{"type": "commit", "id": 6, "address": "/out.gain", "value": 0.25}
```

Writes a control's value into the project's `project.toml` (`value: null` removes the key: each port back to its own default), with `toml_edit` as the M2.5 panel does: only that key changes, the file is written atomically through a hidden temporary file. The value is applied live too, unless it is exactly the last value sent to every port; the running project then takes the same edit, so **nothing is applied twice**: when the live send was taken, the text this write read is exactly the text the running project was loaded or last reloaded from (or last committed), and the file still reads as written right after the write, the watch takes the written file as seen and does not reload it (no `reload` push). Otherwise (the send was refused, or another writer's edit is in the file) the watch reloads it and applies what differs from the running project. The comparison is of content, not of modification times. Reply:

```json
{"type": "committed", "id": 6, "address": "/out.gain", "value": 0.25, "file": "project.toml", "writes": 1, "sent": true, "note": null}
```

Only controls are committed (`not-editable` otherwise): a parameter inside a composite is set in the composite's file, and writing it there would change every instance of the composite.

A write that fails (a read-only folder, a full disk) answers `{"type": "error", "code": "write-failed", "message": …}`, and the value the commit sent live **stays live and unsaved** (below): it is never quietly taken as saved.

### Unsaved: `unsaved`, `retry`, `revert`

The live state and the project's text may differ (ADR 0009, 0013), but never silently: **the host knows every parameter whose live value the file does not hold, or may not hold, and tells every client** — in `describe` (`unsaved`) and as a push whenever it changes, so a second client or a reloaded page sees the same. Clients hold no state of their own about it.

```json
{"type": "unsaved", "count": 1, "lastError": "out.gain was not written to …/project.toml: Permission denied (os error 13)",
 "parameters": [{"address": "/out.gain", "file": 0.5, "inFile": true, "control": true, "applied": true,
   "error": "out.gain was not written to …/project.toml: Permission denied (os error 13)"}]}
```

A parameter is listed while:

- **a control's latest live value is not the file's** (`file`: the file's value, or the default without a key): an edit not committed yet (a drag in progress, a `set` or `ramp`), or one whose write failed (`error`: the write's error). Each new `set`, `ramp` or `commit` replaces the live value: `retry` writes the newest one, never an older failed one.
- **its latest live value did not reach every port** (`applied: false`): the Controller refused it (`refused`), a runner refused it after (the edit's ack is `refused`), or a commit was written while its live send was refused (until the reload of the written file sends every port the file's value). It stays listed whatever its value until a send of it is taken: a refused `set` back to the file's value does not make it saved, since the ports still hold what they held. `describe`'s `base` is then the last value the Controller took for every port: one it refused is never the base, but one a runner refused after the Controller took it is (it is listed `applied: false`), and stays so through later sends the Controller refuses. A plan parameter port that is no control has no live base in `describe`: `instances` gives the plan's base whatever was sent. (Through this protocol a runner does not refuse a `set`: the Controller keeps at most as many commands in flight as the engine's event queue holds.)
- **a failed reset** (`commit` of `null`) leaves each port at its default while the file has the key.
- **a plan parameter port that is no control** was edited live (`control: false`): its value is in a composite's file, which the protocol never writes; it can be reverted, not retried.

`count` is the number listed; `lastError` the most recent error still listed. A parameter whose live and file values agree again (moved back to the file's value, with that send taken) is no longer listed, and its error goes with it. A reload whose file changes a listed parameter's value sends the file's value and so clears it; a reload of other keys keeps it. A new run of the same project (`transport.play`, `setup.apply` playing again) plays the file and the host plays every listed control's live value again in it, so it stays live and unsaved; `transport.stop` keeps them, and opening another project (`project.open`) lets them go and says how many in `app`'s `message`.

| Message | Does | Reply |
|---|---|---|
| `{"type": "retry", "address": "/out.gain"}` | Commits the control's live value again (the latest one; after a failed reset, `null`), sending it live first unless every port already took it. A send the Controller refuses writes nothing (`refused`) | `committed`, as for `commit`; errors `write-failed` (still unsaved, with the new error), `refused`, `not-editable` (a plan port that is no control), `not-unsaved` |
| `{"type": "revert", "address": "/out.gain"}` | Sends each port the file's value (each port its own default without a key; a plan port the plan's base), and the parameter is saved. Stopped, it only clears | `{"type": "reverted", "address", "value": <the file's value or null>, "sent": <whether it played>}`; errors `refused` (still unsaved, its ports in doubt), `not-unsaved` |

Tests: `crates/signal-ui/tests/unsaved.rs` (a read-only project folder: `write-failed`, listed in `describe` and pushed to every client, retry once writable, revert, a newer edit replacing the failed value, a refused edit back to the file's value staying unsaved, a reload), `crates/signal-ui/src/panel.rs` (`the_base_is_the_last_value_the_controller_took`: a value a runner refused stays the base), `crates/signal-ui/tests/app_mode.rs` (a failed write while stopped is still unsaved when playing).

### `panel.check`

```json
{"type": "panel.check", "id": 7, "file": "main.html"}
```

The binding check of a panel in the project's `ui/` folder (`file`: an `.html` file there, by the static files' rules) against the running plan: the check `signal ui check` makes (`crates/signal-ui/src/bindings.rs`), so a panel shows at load what the command prints. `<signal-panel>` asks it once described, and again when the plan version changes. Reply:

```json
{"type": "panel.check", "id": 7, "file": "ui/main.html", "controls": 6, "bindings": 6, "ok": false,
 "diagnostics": [{"severity": "error", "file": "ui/main.html", "line": 36, "column": 28, "control": 0,
   "message": "unknown port /drums.kick_decya: /drums has kick_decay, kick_drive"}]}
```

`control` is the control's index among the file's `signal-*` elements other than `<signal-panel>`, in source order (null for the file as a whole); `ok` is false when any diagnostic is an error. Errors: `unknown-panel` (no such file, not `.html`, outside `ui/`), `not-playing` (the project does not plan).

### `panel.read`, `panel.write`, pushed `panel.written`

The panel editor's text (M2.9-2, ADR 0026 §5, `docs/ui-controls.md` "Editing a panel"). A write replaces exactly the content of the file's one `<signal-panel>` element and nothing else; the file stays text.

```json
{"type": "panel.read", "id": 8, "file": "main.html"}
```

→ `{"type": "panel.read", "id": 8, "file": "main.html", "exists": true, "base": "<sha-256 of the file, hex>", "text": "<the whole file>", "html": "<the content of its <signal-panel>>", "editable": true, "reason": null, "planned": true}`. `file` follows `panel.check`'s rules (an `.html` file in the project's `ui/`); a missing `main.html` (the generated default panel) reads `exists: false` with `base`, `text` and `html` null. `editable` is false, with `reason`, for a file without exactly one `<signal-panel>` element (or without its end tag outside comments and scripts). `planned` says whether the project plans, which a write needs (below). Errors: `unknown-panel`.

```json
{"type": "panel.write", "id": 9, "file": "main.html", "html": "\n  <div class=\"signal-cards\">\n    …\n  </div>\n", "base": "<sha-256 the editor started from>"}
```

`html` is the new content of the `<signal-panel>` element, as the editor's serialiser writes it (`ui/lib/panel-tree.js`); `base` is the SHA-256 of the file it started from (`panel.read`, or the last `panel.written`), or null for a `main.html` that does not exist yet. The server, never trusting the page:

1. takes `file` by `panel.check`'s rules, or a missing `main.html` (`unknown-panel` otherwise: no other folder, name or project; `signal ui` writes only the project it serves). The file must resolve inside the project: a linked `ui/` folder or a linked file is `unknown-panel`, as for `panel.read`;
2. checks `html` against the whitelist again (`ui/lib/panel-whitelist.json`, the same file the page uses): the controls with their attributes, `div.signal-cards`, `section.signal-card`, `h1`, `h2`, `p`, `div.row`, `div.signal-group`, lowercase class tokens and text, in that nesting; a comment, a script, a `style` element or attribute, an event handler, any other element or attribute is `bad-html`;
3. refuses a file that changed since `base` with `conflict`, carrying the file's current `text`, `base` and `html`, so the page can read it again;
4. finds the file's one `<signal-panel>` element (its end tag found as the start tags are, outside comments and the content of `script`, `style`, `textarea` and `title`): `not-editable`, naming why, without exactly one;
5. replaces its content only: every byte outside it (the head, styles, scripts, comments) is kept. A missing `main.html` is created with the standard head (the project's name as title, `/ui/controls/theme.css`, a dark page) and `import '/ui/controls/index.js'`;
6. checks the bindings of the new file as `panel.check` does (`crates/signal-ui/src/bindings.rs`): what it refuses is `bad-binding`, with its `diagnostics` in `panel.check`'s form; without a plan, `not-playing`;
7. writes atomically (a hidden temporary file beside it, synced and renamed, as `commit` writes `project.toml`), and the watch takes it as seen (no `reload`). A `.signal` project's working folder then differs from its file: the project is edited (`app`, "Signal.app").

Reply: `{"type": "panel.written", "id": 9, "file": "main.html", "base": "<sha-256 of the written file>", "created": false}` (`created`: the write made `main.html`). Every client is pushed `{"type": "panel.written", "file", "base", "html"}`; `<signal-panel>` shows the written panel in place (the generated panel, whose file now exists, loads the page again). Nothing is written when any step refuses; `write-failed` is a write the folder refused. This adds the project's `ui/*.html` panel files to what a client may write (Security notes, "Files").

Tests: `crates/signal-ui/tests/panel_write.rs` over both transports (a write and its push, refused elements and attributes, a conflict, a refused binding, the text outside kept byte for byte with a `</signal-panel>` in a comment and a script, not editable, the generated panel's first write, only the served project's panels), `crates/signal-ui/tests/app_mode.rs` (`a_panel_write_marks_a_signal_project_edited`), `crates/signal-ui/src/panel_write.rs` (the SHA-256 vectors, the whitelist, the end tag).

### `pattern.read`, `pattern.write`, pushed `pattern.written`

Pattern files (M2.11-1, ADR 0028 §7, `project-format.md` §12): the note and step data a piano roll or step grid edits, read and written whole as text, with the rules of `panel.write` and `patch`.

```json
{"type": "pattern.read", "id": 10, "name": "melody"}
```

→ `{"type": "pattern.read", "id": 10, "name": "melody", "exists": true, "base": "<sha-256 of the file, hex>", "text": "<the whole file>", "kind": "notes"}`. `name` is a pattern name, `[a-z][a-z0-9_]*`, never a path: the file is `patterns/<name>.toml` of the project served. `kind` is `"notes"` or `"steps"`, or null for a file that does not read as a pattern. A missing file reads `exists: false` with `base`, `text` and `kind` null. Errors: `bad-value` (not a name), `unknown-pattern` (the file or `patterns/` is a link, or cannot be read), `no-project`.

```json
{"type": "pattern.write", "id": 11, "name": "melody", "text": "schema_version = 1\nkind = \"notes\"\n…", "base": "<sha-256 the editor started from>"}
```

`text` is the whole new file; `base` is the SHA-256 of the file the editor started from (`pattern.read`, or the last `pattern.written`), or null for a file that does not exist yet. The server, never trusting the page:

1. takes `name` as a pattern name (`bad-value` otherwise) and refuses a `patterns/` folder or file that is a link (`write-failed`: patterns are written only in the project's own `patterns/`, as `schematic.move` writes positions);
2. refuses a file that changed since `base` (or exists when `base` is null) with `conflict`, carrying its current `base` and `text`;
3. loads and plans the project with the new text, as a reload would, and while playing checks that the running project can take it: the pattern's own errors (syntax, unknown keys, values out of range, overlapping notes) and those of the instances naming it (a wrong kind, a key the type cannot play) are `does-not-load`, with `diagnostics` (one `file:line:column: error: …` per line, also in the message) naming each problem;
4. reads the file again: one that changed since step 2 is `conflict`, as there;
5. writes atomically (a hidden temporary file, synced and renamed, as `commit` writes), creating `patterns/` if needed. The watch is **not** told the write was seen: playing, its reload applies the file live, a swap at an edit frame in which every instance survives with its state (a playing loop keeps its position; `semantics.md` §12), pushed as `reload` and recorded in a session like any swap. A `.signal` project's working folder then differs from its file: the project is edited (`app`, "Signal.app").

Reply: `{"type": "pattern.written", "id": 11, "name": "melody", "file": "patterns/melody.toml", "base": "<sha-256 of the written file>", "created": false}` (`created`: the write made the file). Every client is pushed `{"type": "pattern.written", "name", "base", "text"}`. Nothing is written when any step refuses; `write-failed` is also a write the folder refused. `graph.files` does not list pattern files: a pattern's base is `pattern.read`'s.

**Size.** A client message is at most 64 KiB ("Limits"): a `pattern.write` larger than that closes the client's connection, like any message, and writes nothing. So a pattern editable over the protocol is one whose text, JSON-escaped, fits 64 KiB with the message around it: about 63 KiB of TOML, some 1200 notes written one per line, or any steps pattern up to 64 lanes of 256 steps (`project-format.md` §12). A larger file is edited as text.

**The JS client.** `ui/signal-ui.js`'s `Connection` wraps the two: `readPattern(name)` resolves `{ text, sha, base, exists, kind }` (`sha` and `base` are both the reply's `base`), `writePattern(name, text, base)` resolves `{ sha, base, created }` (the written file's), and a refusal rejects with an `Error` whose `code` and `message` are the server's and whose `reply` is the error message (a `conflict` carries the current `base` and `text`). The pattern controls use exactly these two methods when a connection offers them; `ui/fake-connection.js` offers them over in-memory patterns (`ui/signal-ui.test.js`, `ui/fake-connection.test.js`).

Tests: `crates/signal-ui/tests/pattern.rs` over both transports (a read, a write and its push and live swap, a conflict, refused text naming the problem, a wrong kind, names and links, a first write, a message over the limit closing the connection with nothing written), `crates/signal-ui/tests/app_mode.rs` (`a_pattern_write_marks_a_signal_project_edited`).

### Pushed: `reload`, `reload.error`

Every 100 ms the server checks the project folder (hidden files ignored, as `signal play --watch`) and applies a change live. It pushes `{"type": "reload", "sets": 2, "swap": 1, "removed": 0, "message": "applied a swap to version 1 (2 sets, 0 removed)"}`, or `{"type": "reload.error", "message": …}` when the folder does not load or cannot replace the running project (the last good version keeps playing).

### `setup.*`

The interface setup dialog's functions (M2.5), without a window. The Setup file is the machine's (`docs/project-format.md` §9): `signal --setup <file>`, else `$SIGNAL_SETUP`, else Signal.app's.

| Message | Does | Reply |
|---|---|---|
| `setup.get` | — | `{"type": "setup", "path", "setup": {"output", "input", "buffer", "cpuWorkers", "outputChannels", "rtPolicy"}, "problems", "playing", "run": {"device", "rate", "buffer", "threads", "map", "inputChannels"}, "rtPolicies"}`: `rtPolicy` is the workers' wake policy, one of `rtPolicies` |
| `setup.devices` | `refresh: true` lists the devices again; refused while playing (`playing`: "stop playback to refresh: listing devices pauses metering and status"), as in the dialog. `choices`: `{"output", "input"}`, each `{"name", "id", "signature"}`, devices a page chose, found again by identity | `{"type": "setup.devices", "devices": [{…, "index", "label", "signature", "capabilities"}], "error", "output", "input", "choices", "compute"}`: every device (name, id, direction, default, configurations), its label (same-named devices told apart), its signature (channels and rates, which tell same-named devices apart without an id), its capabilities as the dialog shows them; where the Setup's output and input are, and where the `choices` are now (`{"found": index}` or `{"unresolved": reason}`, as the dialog finds a chosen device after a refresh); the compute devices |
| `setup.check` | `output`: a device index (or null: the default output), `buffer` | `{"type": "setup.check", "ok", "message"}`: whether that output plays the project's rate with that buffer |
| `setup.apply` | `output`, `input`: absent or `"keep"` (keep the Setup's), `null` (none: the default output, no input), `{"index": n}` or `{"name", "id", "signature"}`; `buffer` (1 to 8192), `cpuWorkers` (`"auto"`, null or 1 to 64), `outputChannels` (`"1, 2"`, `[1, 2]` or null), `rtPolicy` (a wake policy's name: `park50`, `budget-spin`, `semaphore`, `fanout`; `bad-value` otherwise) | `{"type": "applied", "saved", "restarted", "frame", "setup", "run", "notes", "error"}` |

`setup.apply` is the dialog's Apply: the settings go into the Setup, which is saved; **while playing, the run stops and plays again from frame 0** with the Setup's output device, channel map (a map the device cannot take falls back to the identity, with a note), buffer, CPU workers, and its input, metered. Subscriptions carry over (scopes are started again on the new run), edits waiting for their reports are answered `abandoned`, and `describe` is pushed with `"restarted": true` after the reply. The options of `signal ui` that the dialog does not have stay as they were (`--rt-policy`, `--rt-computation`, `--rt-trace`, `--driver`, `--fast`). Two cases differ from the app:

- A run that records a session or its audio (`--record-session`, `--record-audio`) is not restarted: the Setup is saved and the reply is `{"type": "error", "code": "recording", …}`, because a recording covers one run from frame 0.
- Under `--driver null` the device list is still the machine's CoreAudio list, so the Setup's devices resolve as the app resolves them; the run plays on the null driver as if on that device. Tests give the host a fixed list.

### Signal.app (app mode)

Signal.app's control thread runs the host in **app mode** (`Host::new_app`, M2.8-4): its window's WebView loads `http://127.0.0.1:<http port>/ui/app/index.html#ws=…&token=…` from it. App mode starts **stopped**, on the project the app was given or else the Setup's last project, and plays every run with the Setup (its output, channel map, buffer, CPU workers, wake policy and input), with the rules of the app's state (`crates/signal-ui/src/app.rs`, shared with `signal-app`'s `AppState`). `describe` answers while stopped (with `"playing": false`), and `commit` writes while stopped. Its own messages, which `signal ui` refuses with `not-allowed`:

| Message | Does | Reply |
|---|---|---|
| `app.get` | — | `{"type": "app", "project": {"dir", "file", "name", "edited", "sampleRate", "diagnostics"} or null, "playing", "power": "on" or "off", "reload": {"message", "error"} or null, "notes", "message", "writes", "setupPath", "setupProblems"}`: what the window shows beside the panel and meters (`sampleRate` is null and `diagnostics` lists why when the folder does not load; `message` is the last refusal or notice). For a `.signal` project (M2.8-12) `dir` is its working folder, `file` the `.signal` file, and `edited` whether the working folder differs from the file (its files looked at every 250 ms, and its content compared whenever they change and at least every 2 s, or every twenty times as long as one comparison takes for a large project, so an edit that keeps the files' sizes and times shows within that bound; a change of the files shows as edited at once, until a comparison of the folder as changed says otherwise; the comparison runs on a thread of its own, so the control thread never waits for it, and a result whose folder changed since it started is let go; every decision that may discard or replace the folder (a prompt, Open, close, quit) compares at once, without compressing, and Save packs what the folder holds then; `file` is null and `edited` false for a folder); `name` is the file's stem or the folder's name. Pushed (without `id`) whenever it changes |
| `app.power` | `on`: `true` or `false`, the window's power switch (M2.8-11; application state like play and stop, never written to the project or the Setup; the app starts switched on). Off: stops the run as `transport.stop` does (the device is released, the engine's worker threads are joined, GPU work, which only a run does, ends; the open project, its panel and its unsaved values stay) and lets go of **every** subscription; the control thread then waits blocked for the next message instead of stepping every millisecond. On: plays again if it played when switched off | `{"type": "power", "on", "playing"}` |
| `transport.play` | Plays the open project from frame 0 with the Setup | `{"type": "transport", "playing": true}`; errors `no-project` ("open a project folder first"), `does-not-load` ("the project does not load"), `device` (the output or engine could not start), `powered-off` (switched off) |
| `transport.stop` | Stops; edits waiting for reports are answered `abandoned`; subscriptions are kept (scopes start again with the next run); the devices are listed again | `{"type": "transport", "playing": false}` |
| `project.open` | `path`: a project folder, its `project.toml`, or a `.signal` file (made absolute); `unsaved`: `"save"` or `"discard"`, optional. A `.signal` file is unpacked first into a working folder of its own under `~/Library/Application Support/Signal/Open/` (`project-format.md` §10), so a refused one changes nothing. Then it stops, opens the project (stopped), records it (the `.signal` file, never its working folder) as the Setup's last project and saves the Setup, and serves its `ui/` under `/project/`; the previous `.signal` project's working folder is removed. While a `.signal` project is edited it is refused with `edited` ("Save changes to <name>?") unless `unsaved` says what to do with the edits: `"save"` writes them to the file first, `"discard"` lets them go | `{"type": "project", "dir", "file", "loads", "diagnostics"}`; errors `edited`, `archive` (the refused file and the reason), `not-saved` |
| `project.save` | Save of a `.signal` project (M2.8-12): its working folder packed and written over the file atomically (a temporary file beside it, then renamed), which clears `edited`. A folder project has nothing to write here: the page's Save writes its unsaved parameters again with `retry` first, then sends this | `{"type": "saved", "file"}` (`file` null for a folder); error `not-saved` |

`describe` is pushed after each of them. **While switched off** the app's `welcome` carries `"power": "off"` (`"on"` otherwise; `signal ui`'s has no `power`), so a client that connects then (the Panel tab's frame after another project is opened, say) knows it before it could subscribe, and every `subscribe` is refused with `powered-off`: no stream frame is sent to any client. The client in `ui/signal-ui.js` sends no subscription while off and sends them all again when an `app` message says `"power": "on"`. A project opened while off opens stopped, and switching on then plays nothing. The **status line** is pushed as `{"type": "status", "playing", "line", "status": {"frame", "elapsed", "xruns", "loadAverage", "loadWorst", "clipped", "buffer", "threads", "workers", "scheduling"}}` four times a second while playing and once after a stop (the last run's): `line` is the text the window shows, `xruns` counts the output's and the input's. When the window closes, the host stops, records the window's size in the Setup and saves it; a `.signal` project's working folder is removed, unless it is edited and its edits were not let go (the window's prompt, below), when it is kept with them.

**Project files in the window (M2.8-12).** Open…, Save As… and the "Save changes?" prompt are the window's (`crates/signal-app/src/main.rs`, decided in `window.rs` without a window), not messages: Open… (NSOpenPanel), a drop and a `.signal` file opened from the Finder (tao's `Event::Opened`) open through the host as `project.open` does; Save As… (NSSavePanel, named `<project name>.signal`) writes the project to the chosen file, which it is from then on (a folder project's folder is left as it is: the project is opened again from the new file, stopped). While a `.signal` project is edited, the close button, File ▸ Quit, ⌘Q and opening another project ask "Save changes to <name>?" with Save, Don't Save and Cancel (an NSAlert). The Dock's Quit and logout ask with Save and Don't Save only: macOS has decided to terminate by then, because tao 0.37.1 never answers `applicationShouldTerminate:` and answering it needs `unsafe` code (operator decision operator-request:009, ADR 0025); unanswered, the working folder is kept with the edits. In a browser the page has no Save As and no chooser; its Setup field opens a `.signal` path too, and an `edited` refusal is asked about in the page with the same three buttons.

### `stats`, `ping`

`stats` → `{"type": "stats", "frame", "clients": [{"client", "dropped"}], "evicted", "meterOverflows", "scopeOverflows", "vectorOverflows", "commandOverflows", "clipped", "writes"}`. `ping` → `{"type": "pong", "frame"}`.

### Errors

`{"type": "error", "id": …, "code": "…", "message": "…"}`. Codes: `unauthorized`, `protocol`, `already-authenticated`, `bad-message`, `unknown-type`, `unknown-address`, `unknown-sub`, `unknown-device`, `unknown-panel`, `bad-address`, `bad-value`, `bad-rate`, `bad-device`, `not-a-signal-output`, `no-scope-slot`, `not-a-vector-output`, `no-vector-slot`, `busy`, `refused`, `not-playing`, `too-many-clients`, `not-editable`, `write-failed`, `not-unsaved`, `playing`, `recording`, `unknown-level`, `unknown-instance`, `does-not-load` (`graph`, `schematic.*`, `modules`, `patch`, `pattern.write`), `conflict` (`panel.write`, `patch`, `pattern.write`), `unknown-pattern` (`pattern.read`), `in-use`, `unknown-module` (`patch`), `no-project` (`patch`, `modules`), and in Signal.app's messages `not-allowed`, `no-project`, `does-not-load`, `device`, `powered-off` (`transport.play` and `subscribe` while switched off).

## Binary frames

Little-endian, a 24-byte header, then the payload (`crates/signal-ui/src/frames.rs`, `decodeFrame` in `ui/lib/protocol.js`):

| Offset | Size | Field |
|---|---|---|
| 0 | u8 | kind: 1 meters, 2 scope, 3 parameters, 4 vector |
| 1 | u8 | protocol version (1) |
| 2 | u16 | reserved, 0 |
| 4 | u32 | subscription (`sub`) |
| 8 | u64 | first frame: meters, the first callback's; scope, point 0's; vector, the snapshot's; parameters, the runner's next frame when sent |
| 16 | u32 | count |
| 20 | u32 | meters: frames covered; scope and vector: decimation; parameters: 0 |

| Kind | Payload, `count` times |
|---|---|
| 1 meters | `f32` peak, `f32` RMS (linear; full scale 1) per channel |
| 2 scope | `f32` point |
| 3 parameters | `f64` base, `f64` effective per address, in the subscription's order; NaN until the runner reported it |
| 4 vector | `f32` per element of the port, in order |

## Delivery: slow and gone clients

The control thread never waits for a client. Each client has a thread that writes what the control thread queued for it and reads its messages; the queue between them is bounded (256 messages for `signal ui`). The control thread only ever tries to queue: when a client's queue is full, a **binary frame is dropped and counted** (`stats`: the client's `dropped`), and a **control reply that does not fit evicts the client** (`evicted`), since losing a reply silently would leave it waiting. Eviction sets the client's flag and drops its queue; its thread sees either and closes the connection on its own. The client's thread writes one message at a time and flushes it; while its socket takes nothing (each write waits at most 100 ms), it takes no more from the queue, so the WebSocket holds at most one message beyond what the socket took (and never more than 4 MiB, but for one message larger than that): the rest waits in the bounded queue, where the drops and the eviction above apply. A client whose socket takes nothing for 2 s is disconnected by its thread. A single outgoing message larger than the 4 MiB write limit (a 4096-voice project's `graph` is about 6 MB, M2.9-1b) is taken when nothing else is unwritten, and the limit applies again once the socket took it, so a backlog stays bounded (`crates/signal-ui/tests/app_mode.rs`, the 4096-voice `graph`, both transports). Verified with a socket whose writes keep timing out while frames keep arriving (`crates/signal-ui/src/server.rs` tests) and with a client whose writer is held (`crates/signal-ui/tests/backpressure.rs`).

## Version rules

- `protocol` is 1. The server names it in `welcome`, in the `protocol` error, and in byte 1 of every binary frame.
- Adding message types, fields, error codes or frame kinds keeps version 1. Clients ignore what they do not know; the server ignores unknown fields.
- Removing or renaming anything, or changing what a field or a frame byte means, is version 2. A server that speaks several versions answers each in the one its `hello` asked for.

## Security notes

- **Local only by default.** Both servers bind 127.0.0.1. `signal ui --remote` binds every interface, for a tablet or another Mac; it is off by default and prints a warning. Remote access then relies on the token alone, and the token travels in clear text (no TLS). Signal.app binds 127.0.0.1 only (a Setup switch for remote clients is later work).
- **A token per session.** 32 bytes from the system's random source (`/dev/urandom`), hex, new for every run of the server, compared in constant time. It is required in the first message of every WebSocket connection. It is in the URL's fragment, never in an HTTP request. Anyone with the URL can control the run and write the project's `project.toml`, so the URL is a credential: `signal ui` prints it on the terminal only.
- **Origin.** A browser connecting to the WebSocket must come from the HTTP server's own origin: `http://127.0.0.1:<http port>` (or `localhost`, `[::1]`); with `--remote`, the host the request was sent to and the HTTP port. Another site's page is refused at the handshake (403), before the token. A client without `Origin` is not a browser page and goes on to the token check.
- **Host.** Without `--remote`, the HTTP server answers only the loopback names with its port, so another site cannot read the files through a name that resolves to 127.0.0.1 (DNS rebinding).
- **Files.** Only the two `ui/` folders are served (above). A client writes only through `commit` (a top-level parameter in `project.toml`), `schematic.move` and `schematic.reset` (the project's `ui/schematic.toml`, presentation only, never through a linked `ui/` or file), `panel.write` (a panel file in the project's `ui/`, its `<signal-panel>` content only, within the editor's whitelist), `pattern.write` (M2.11-1: `patterns/<name>.toml` of the served project, by name, never through a linked `patterns/` or file, after the same load and compile as a reload), `patch` (M2.9-3: the served project's `project.toml` and `composites/*.toml`, the files the project itself read, never one reached through a link; and its `ui/schematic.toml`, never through a linked `ui/` or file; after the same load and compile as a reload, and inverse-only `edits` limited to what the op means, the comment lines above a restored entry and its `gpu` pin) and `setup.apply` (the Setup file the server was started with).
- **`project.open` opens another project only in Signal.app's app mode**; `signal ui` refuses it (`not-allowed`) for every path but the project it serves, so a client of `signal ui` cannot point it at another folder. For the project it serves (its folder, its `project.toml`, or the `.signal` file it was started with, compared after resolving links) `signal ui` plays it again from frame 0 from its files, a `.signal` file unpacked again into a new temporary folder (M2.8-12); while recording that is refused with `recording`, as `setup.apply` does not restart a recording run. In the app, a client with the token may open any folder it names: the host then writes the Setup file the app was started with (`last_project`), serves that folder's `ui/` (its panels and `ui/assets/`, by the rules above) under `/project/`, and later `commit`s write that folder's `project.toml`. The token rules are unchanged, and the app binds 127.0.0.1 only.
- **Clients.** At most 32 WebSocket connections are served at once, a connection that has not sent `hello` yet included. Such a connection holds its place for at most 5 s from its acceptance: an absolute deadline over its WebSocket handshake and its `hello` with the token, not a timeout per read, so a client that trickles its handshake a byte at a time loses its place as one that sends nothing does (it is told `unauthorized` if its handshake was complete, else closed without a reason), and a stop ends it at once. Connections before `hello` have no budget of their own apart from the 32: anyone who can connect can open as many as it likes, so a smaller budget would refuse a newcomer just as the full one does; the deadline bounds how long any of them holds a place. One past that is refused: once it has sent its handshake it is told `{"type": "error", "code": "too-many-clients"}` and closed with code 1013 (try again later) and the reason `too-many-clients`, all within 1 s; one that sends no handshake in that time is closed without a reason (a close reason needs a WebSocket). Two threads refuse, with 16 connections waiting for them; while those are taken, accepting waits rather than drop a connection. None of these threads is waited for at shutdown (`crates/signal-ui/tests/backpressure.rs`).
- **Limits.** A client message is at most 64 KiB (a larger one closes the client's connection; a `patch` whose undo would need more is refused before it is written); numbers a client sends are checked before use (an unsupported rate is `bad-rate`, a value outside its range is clamped); messages from all clients wait in a bounded queue; at most 256 are handled per control step.

## Running it

```sh
signal ui projects/kick --open             # CoreAudio, the default output; opens the main panel
signal ui check projects/kick              # checks the panels' bindings (exit 1 with diagnostics)
signal ui projects/kick --driver null      # no device, real-time pace
signal ui projects/kick --port 8800        # a fixed HTTP port
```

`signal ui` takes every option of `signal play` (`--device`, `--buffer`, `--seconds`, `--threads`, `--rt-policy`, `--rt-computation`, `--rt-trace`, `--driver`, `--fast`, `--record-session`, `--record-audio`, `--record-boundary-data`) and always watches the folder, as `play --watch` does. It stops on Enter (or at `--seconds`), and with `--json` prints `{"url", "panel", "http", "ws"}` when it starts. The UI library's folder is the app bundle's `Contents/Resources/ui` when the program runs from a bundle, else the repository's `ui/` of the build (`--ui-dir` for another).

`signal ui` also prints the schematic's page, `http://127.0.0.1:<http port>/schematic#ws=…&token=…` (`--json`: `schematic`).

Pages and controls use the client in `ui/signal-ui.js` (reconnect included) and the web components in `ui/controls/`; `/ui/gallery.html` shows them all (`docs/ui-controls.md`).

The page at `/` is the raw protocol test page: the connection and protocol, the output meters, a scope of a chosen signal output, a slider per control (a ramp while moving, a commit on release) with its effective value, the unsaved indicator ("N unsaved parameters — last error", Retry all, Revert all; "Unsaved" above), and the `describe` reply.
