# The UI protocol (version 1)

The interface through which a web page describes, observes and edits a running project (ADR 0023 §1). Signal.app's WebView, a browser and later remote devices all use it. The server is the crate `signal-ui`; `signal ui <project>` runs it (see "Running it").

## Servers

Two servers, both bound to **127.0.0.1** by default:

| Server | Port | Serves |
|---|---|---|
| HTTP (`tiny_http`) | `--port N` (a free one without it, or with 0) | Static files only (below) |
| WebSocket (`tungstenite`) | a free port | The protocol |

They are separate because `tiny_http`'s upgraded connection is one boxed stream with no timeouts that cannot be split, so one thread could not both wait for a client's messages and write the frames queued for it.

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
| `/project/<path>` | the project's `ui/<path>`: its panels, and `ui/assets/` for their SVG and PNG files. `/project/main.html` of a project without `ui/main.html` is the generated panel, the repository's `ui/default-panel.html` (`ui-controls.md` "Panels") |

Only `GET` and `HEAD` (else 405). A path with `..`, `%`, `\`, NUL, an empty segment or a segment starting with `.` is not served, nor one that resolves (links followed) outside its folder; there are no directory listings (404). Content types: `.html` `text/html; charset=utf-8`, `.js` and `.mjs` `text/javascript; charset=utf-8` (ES modules), `.css` `text/css`, `.json` `application/json`, `.svg` `image/svg+xml`, `.png` `image/png`, `.jpg` `image/jpeg`, `.gif`, `.webp`, `.woff2`, `.txt`; anything else `application/octet-stream`. Every response carries `X-Content-Type-Options: nosniff`, `Cache-Control: no-cache` and `Referrer-Policy: no-referrer`.

## Messages

Control messages are JSON text frames, each an object with a `type`. A request may carry an `id` (any JSON value); its reply carries the same `id`. Data streams are binary frames (below). Messages without an `id` are pushed by the server.

### `hello` (the first message)

```json
{"type": "hello", "protocol": 1, "token": "<session token>"}
```

The first message on a connection must be `hello`, within 5 s, with the session token and protocol 1. Then the server answers

```json
{"type": "welcome", "protocol": 1, "client": 3, "project": "kick", "sampleRate": 96000, "frame": 123456, "planVersion": 0}
```

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
 "scopes": {"slots": 8, "points": 256, "maxRate": 48000}}
```

(Values here are illustrative.) `instances` is the flattened plan: every primitive instance with its ports and parameters (kind, unit, range, default, combine, and the parameter's initial base in the plan). `controls` are what the parameter panel edits: the parameters of the project's top-level instances, a composite's exported ones included, at the address `/<instance>.<parameter>`, with the plan ports a value goes to (an export can fan out to several), the range they share, the value without a key in the file (`default`), the base (`base`, the file's value or the default) and whether the file sets it. `planVersion` is the running engine's plan version; `meters.output` lists the sink's channels, the device channel each plays on, and its `label` (`out 1 → 3`, or `out 2 (not played)`).

The server pushes `describe` (without `id`) when a plan swap commits, after a reload that changed values, after a `commit`, and after `setup.apply` played again (then with `"restarted": true`).

### `subscribe`, `unsubscribe`

`{"type": "subscribe", "stream": …, "id": 2}` → `{"type": "subscribed", "id": 2, "sub": 5, "stream": …}`; the stream's frames carry `sub`. `{"type": "unsubscribe", "sub": 5}` → `{"type": "unsubscribed", "sub": 5}`. A client's subscriptions end with its connection.

| `stream` | Fields | Frames |
|---|---|---|
| `parameters` | `addresses`: controls or plan parameter ports; `rate`: frames per second, at least 1 (default 30; more than 60 is taken as 60; below 1, not a number or not finite: `bad-rate`) | kind 3, every `1/rate` s: each address's base and effective value, as the runner last reported them (the parameter observation queue: up to 64 parameters per callback, round robin) |
| `meters` | `source`: `"output"` (default, the audio sink's channels) or `"input"` (the Setup's input, metered only); `rate` as above | kind 1, every `1/rate` s once levels arrived: per channel, the peak and the RMS over the callbacks since the last frame |
| `scope` | `port`: a signal output of the running plan (`/a.out`), or a signal input (`/out.l`: the sum of its sources, each scaled, as the instance receives it); `rate`: points per second, at least 1 (default and at most the sample rate; below 1, not a number or not finite: `bad-rate`) | kind 2: blocks of 256 consecutive points, one every `decimation` frames, `decimation = max(1, round(sampleRate / rate))`; the reply also gives `decimation` and the `slot` |

**Scope streams** come from the scope adapter in `signal-output` (beside the audio adapter of ADR 0022): a decimated capture. A point is the port's value at an absolute frame that is a multiple of the decimation, so a stream does not depend on how the frames were split into callbacks or threads. **A scope stream is the output as its readers see it, fades included** (a signal input, the sum of its sources as they are read): it fades in when its instance is added, out when it is suspended or removed, and through the 64-frame fault fade after a fault (`semantics.md` §12, §14). It costs the audio thread a copy per point and allocates nothing there (measured: `crates/signal-output/tests/scope_no_allocation.rs`). **An open block is dropped, not sent, at a discontinuity**: when a swap rebinds the slot, when the slot is started again (a new `subscribe`, or `setup.apply` playing again), and when the points stop being consecutive (frames not run). So up to 255 points are lost there, and every block that arrives is 256 consecutive points. There are 8 slots per run (`no-scope-slot` when they are all in use). After a swap that removes the port the server pushes `{"type": "scope.unbound", "sub": 5}`, and `scope.bound` when a later swap brings it back. Errors: `not-a-signal-output` (neither a signal output nor a signal input), `bad-rate`, `busy` (too many scope commands outstanding; try again).

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

Writes a control's value into the project's `project.toml` (`value: null` removes the key: each port back to its own default), with `toml_edit` as the M2.5 panel does: only that key changes, the file is written atomically through a hidden temporary file. The value is applied live too, unless it is exactly the last value sent to every port; the running project then takes the same edit, so when the file watch reloads the written file **the diff is empty and nothing is applied twice** (the server pushes `{"type": "reload", "sets": 0, "swap": null, "message": "no change to apply"}`). Reply:

```json
{"type": "committed", "id": 6, "address": "/out.gain", "value": 0.25, "file": "project.toml", "writes": 1, "sent": true, "note": null}
```

Only controls are committed (`not-editable` otherwise): a parameter inside a composite is set in the composite's file, and writing it there would change every instance of the composite.

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
| `app.get` | — | `{"type": "app", "project": {"dir", "sampleRate", "diagnostics"} or null, "playing", "reload": {"message", "error"} or null, "notes", "message", "writes", "setupPath", "setupProblems"}`: what the window shows beside the panel and meters (`sampleRate` is null and `diagnostics` lists why when the folder does not load; `message` is the last refusal or notice). Pushed (without `id`) whenever it changes |
| `transport.play` | Plays the open project from frame 0 with the Setup | `{"type": "transport", "playing": true}`; errors `no-project` ("open a project folder first"), `does-not-load` ("the project does not load"), `device` (the output or engine could not start) |
| `transport.stop` | Stops; edits waiting for reports are answered `abandoned`; subscriptions are kept (scopes start again with the next run); the devices are listed again | `{"type": "transport", "playing": false}` |
| `project.open` | `path`: a project folder (made absolute). Stops, opens it (stopped), records it as the Setup's last project and saves the Setup, and serves its `ui/` under `/project/` | `{"type": "project", "dir", "loads", "diagnostics"}` |

`describe` is pushed after each of them. The **status line** is pushed as `{"type": "status", "playing", "line", "status": {"frame", "elapsed", "xruns", "loadAverage", "loadWorst", "clipped", "buffer", "threads", "workers", "scheduling"}}` four times a second while playing and once after a stop (the last run's): `line` is the text the window shows, `xruns` counts the output's and the input's. When the window closes, the host stops, records the window's size in the Setup and saves it.

### `stats`, `ping`

`stats` → `{"type": "stats", "frame", "clients": [{"client", "dropped"}], "evicted", "meterOverflows", "scopeOverflows", "commandOverflows", "clipped", "writes"}`. `ping` → `{"type": "pong", "frame"}`.

### Errors

`{"type": "error", "id": …, "code": "…", "message": "…"}`. Codes: `unauthorized`, `protocol`, `already-authenticated`, `bad-message`, `unknown-type`, `unknown-address`, `unknown-sub`, `unknown-device`, `unknown-panel`, `bad-address`, `bad-value`, `bad-rate`, `bad-device`, `not-a-signal-output`, `no-scope-slot`, `busy`, `refused`, `not-playing`, `not-editable`, `write-failed`, `playing`, `recording`, and in Signal.app's messages `not-allowed`, `no-project`, `does-not-load`, `device`.

## Binary frames

Little-endian, a 24-byte header, then the payload (`crates/signal-ui/src/frames.rs`, `decodeFrame` in `ui/lib/protocol.js`):

| Offset | Size | Field |
|---|---|---|
| 0 | u8 | kind: 1 meters, 2 scope, 3 parameters |
| 1 | u8 | protocol version (1) |
| 2 | u16 | reserved, 0 |
| 4 | u32 | subscription (`sub`) |
| 8 | u64 | first frame: meters, the first callback's; scope, point 0's; parameters, the runner's next frame when sent |
| 16 | u32 | count |
| 20 | u32 | meters: frames covered; scope: decimation; parameters: 0 |

| Kind | Payload, `count` times |
|---|---|
| 1 meters | `f32` peak, `f32` RMS (linear; full scale 1) per channel |
| 2 scope | `f32` point |
| 3 parameters | `f64` base, `f64` effective per address, in the subscription's order; NaN until the runner reported it |

## Delivery: slow and gone clients

The control thread never waits for a client. Each client has a thread that writes what the control thread queued for it and reads its messages; the queue between them is bounded (256 messages for `signal ui`). The control thread only ever tries to queue: when a client's queue is full, a **binary frame is dropped and counted** (`stats`: the client's `dropped`), and a **control reply that does not fit evicts the client** (`evicted`), since losing a reply silently would leave it waiting. Eviction sets the client's flag and drops its queue; its thread sees either and closes the connection on its own. The client's thread writes one message at a time and flushes it; while its socket takes nothing (each write waits at most 100 ms), it takes no more from the queue, so the WebSocket holds at most one message beyond what the socket took (and never more than 4 MiB): the rest waits in the bounded queue, where the drops and the eviction above apply. A client whose socket takes nothing for 2 s is disconnected by its thread. A single outgoing message larger than the 4 MiB write limit cannot be buffered, and the client it was for is disconnected (no current message comes near that size). Verified with a socket whose writes keep timing out while frames keep arriving (`crates/signal-ui/src/server.rs` tests) and with a client whose writer is held (`crates/signal-ui/tests/backpressure.rs`).

## Version rules

- `protocol` is 1. The server names it in `welcome`, in the `protocol` error, and in byte 1 of every binary frame.
- Adding message types, fields, error codes or frame kinds keeps version 1. Clients ignore what they do not know; the server ignores unknown fields.
- Removing or renaming anything, or changing what a field or a frame byte means, is version 2. A server that speaks several versions answers each in the one its `hello` asked for.

## Security notes

- **Local only by default.** Both servers bind 127.0.0.1. `signal ui --remote` binds every interface, for a tablet or another Mac; it is off by default and prints a warning. Remote access then relies on the token alone, and the token travels in clear text (no TLS). Signal.app binds 127.0.0.1 only (a Setup switch for remote clients is later work).
- **A token per session.** 32 bytes from the system's random source (`/dev/urandom`), hex, new for every run of the server, compared in constant time. It is required in the first message of every WebSocket connection. It is in the URL's fragment, never in an HTTP request. Anyone with the URL can control the run and write the project's `project.toml`, so the URL is a credential: `signal ui` prints it on the terminal only.
- **Origin.** A browser connecting to the WebSocket must come from the HTTP server's own origin: `http://127.0.0.1:<http port>` (or `localhost`, `[::1]`); with `--remote`, the host the request was sent to and the HTTP port. Another site's page is refused at the handshake (403), before the token. A client without `Origin` is not a browser page and goes on to the token check.
- **Host.** Without `--remote`, the HTTP server answers only the loopback names with its port, so another site cannot read the files through a name that resolves to 127.0.0.1 (DNS rebinding).
- **Files.** Only the two `ui/` folders are served (above). A client writes only through `commit` (a top-level parameter in `project.toml`) and `setup.apply` (the Setup file the server was started with).
- **`project.open` exists only in Signal.app's app mode**; `signal ui` refuses it (`not-allowed`), so a client of `signal ui` cannot point it at another folder. In the app, a client with the token may open any folder it names: the host then writes the Setup file the app was started with (`last_project`), serves that folder's `ui/` (its panels and `ui/assets/`, by the rules above) under `/project/`, and later `commit`s write that folder's `project.toml`. The token rules are unchanged, and the app binds 127.0.0.1 only.
- **Limits.** A client message is at most 64 KiB; numbers a client sends are checked before use (an unsupported rate is `bad-rate`, a value outside its range is clamped); messages from all clients wait in a bounded queue; at most 256 are handled per control step.

## Running it

```sh
signal ui projects/kick --open             # CoreAudio, the default output; opens the main panel
signal ui check projects/kick              # checks the panels' bindings (exit 1 with diagnostics)
signal ui projects/kick --driver null      # no device, real-time pace
signal ui projects/kick --port 8800        # a fixed HTTP port
```

`signal ui` takes every option of `signal play` (`--device`, `--buffer`, `--seconds`, `--threads`, `--rt-policy`, `--rt-computation`, `--rt-trace`, `--driver`, `--fast`, `--record-session`, `--record-audio`, `--record-boundary-data`) and always watches the folder, as `play --watch` does. It stops on Enter (or at `--seconds`), and with `--json` prints `{"url", "panel", "http", "ws"}` when it starts. The UI library's folder is the app bundle's `Contents/Resources/ui` when the program runs from a bundle, else the repository's `ui/` of the build (`--ui-dir` for another).

Pages and controls use the client in `ui/signal-ui.js` (reconnect included) and the web components in `ui/controls/`; `/ui/gallery.html` shows them all (`docs/ui-controls.md`).

The page at `/` is the raw protocol test page: the connection and protocol, the output meters, a scope of a chosen signal output, a slider per control (a ramp while moving, a commit on release) with its effective value, and the `describe` reply.
