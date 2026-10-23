# ADR 0035: A project as content

- **Status:** Accepted (2026-10-23). The owner's words of 2026-10-23 (translated): "I'd rather imagine it so that a whole project can be brought in as a composite …" and "If mixing is needed, a mixer module or a mixer composite will do it." Read with the owner's earlier words: "there may suddenly be two transports if we want to mix two projects into one" (2026-10-22, ADR 0033), and "whether a new instance or a newly joining project starts, there must be no jump/glitch" (2026-10-23). The owner's answers of 2026-10-23 to the draft's questions:
  - a glitch-free join is enough (a bit-identical join is not required, §6 and Alternatives);
  - an included project's panel edits are written into the host (`set`, §10);
  - the audio interface is the one clock (ADR 0036 §1, accepted the same day; §8);
  - everything is bound to signal sources: following a transport is wiring, not policy (§5, ADR 0037 §6);
  - a project can also be loaded into a slot (ADR 0037).
  The rest of the design is the operator's, delegated.
- **Builds on:** ADR 0006 and `semantics.md` §12 (swaps, survivors, the fade of added instances), 0008 (seeded streams), 0019 (workers), 0022 (output adapters, one audio sink), 0025 (`.signal` files), 0027 §4 (library composites, `signal vendor`), 0033 (`Inputs::is_connected`, exclusive inputs, `lib.transport`) and 0036 §1 (the device's clock is the run's).
- **Relates to** ADR 0037 (slots: an interface whose content may be a project) and the separate decision that a project is open in only one Signal.app instance (§12).
- **Replaces** the earlier draft "Several projects in one instance" (several engines, a mix runner, `set.toml`), which is now an alternative (below). **Supersedes nothing accepted.**
- **Date:** 2026-10-23

## Context
The owner wants to play two projects together, for example two studios with two transports. The model asked for is not a second engine beside the first, but the first project **containing** the second, as it contains any composite.

What exists today:
- **The graph:** a project is already a `CompositeDef` (`signal-graph/src/def.rs`), flattened with hierarchical addresses.
- **The sink:** the plan has one audio sink, chosen by the audio adapter, which applies the sink's `gain` (ADR 0022). A swap must keep the run's sinks (`SinksChanged`).
- **Random streams** are `RngStream::derive(master_seed, address)`, so moving a subtree under a new address changes its randomness.
- **Signal sums and event merges** use canonical source order, by byte-wise source address (`semantics.md` §3, §4).
- **Swaps:**
  - a live edit is a plan swap at a frame;
  - survivors (same address, same type) keep their state;
  - added instances' `audio` outputs fade in over F = 5 ms;
  - removal is suspend-then-swap, an exact fade to +0.0 (`semantics.md` §12).
- **Inputs and exports:** a primitive can ask `Inputs::is_connected(port)` (ADR 0033 §2). Composites have `[exports]` (`project-format.md` §3).
- **Rates:** live play runs at the output device's rate; a project's `sample_rate` is the rate of its offline renders and replays (ADR 0036 §1).

The questions:
- what an included project's sink becomes;
- how the host drives it;
- whether its sound stays what it is alone;
- how it joins and leaves without a glitch;
- how files, panels, the schematic, packing and editing behave.

## Decision

### 1. An included project is an instance
A project can be content in two ways:
- a **plain instance** (`drums = { project = "../studio-transport" }`): its exports are its ports;
- a **slot's `load`** (ADR 0037 §2): its ports are the slot's interface's, matched by name.

Both resolve to the same plan (ADR 0037 §5). This ADR describes what a project is as content; ADR 0037 describes the slot around it. An instance in `[instances]` (of `project.toml` or a composite) may name a **project** instead of a module:

    [instances]
    drums = { project = "../studio-transport" }
    bass  = { project = "parts/bass.signal", seed = 3, bpm = 128,
              set = { "transport.run" = 1, "synth.cutoff" = 400 } }

- **`project`** is a project folder (holding `project.toml`) or a `.signal` file. The path is relative to the folder of the file that names it. An instance has exactly one of `module`, `project` and `slot`.
- **`seed`** (optional): the seed scope of the included project (§4). The default is the included project's own `[project] seed`.
- **Exported parameters** of the included project (§2) are set like a composite's: `bpm = 128`. The outermost value wins.
- **`set`** (optional): overrides of the included project's **top-level** parameters, keyed `"<inner instance>.<parameter>"`, with the same rules as that instance's entry in the included `project.toml`. This is where a host writes what its panels change in an included project (§10). It never writes the included project's files.
- `seed` and `set` apply to both forms, the plain instance and the slot's `load`.
- **Reserved keys:** `project`, `seed`, `set`, `slot`, `load` and `map` join `gpu` and `pattern`. No composite or project may export a parameter of these names.
- **What it is:** the included project is read and checked by every rule of `project-format.md`, in its own folder:
  - its composites resolve in its own `composites/`, its interfaces in its own `interfaces/`, its patterns in its own `patterns/`;
  - `lib.*` resolves in the same library;
  - diagnostics are located in its files (`../studio-transport/composites/kick909.toml:7:3: error: …`).
- **Definition names:** its definitions are handed to `build_plan` under qualified names (`<project id>/<name>`, where `/` cannot occur in a bare name). They never collide with the host's composites or with `lib.` ones. Two instances of the same project share the definitions.
- **What it is for the graph:** an ordinary composite instance at `/drums`. Its insides are `/drums/transport/playhead`, `/drums/kick/env` and so on.
- **The core knows no projects.** Everything project-specific here (paths, `seed` defaults, `set`, the sink rule in §3) is `signal-project`'s. The graph gains one generic notion, the **scope root** (§4) (hard rule 1; `domain_scope.rs` checks that `signal-graph` and `signal-engine` name no `project` or `include`).

### 2. A project's interface: `[exports]` in `project.toml`
`project.toml` may carry `[exports]` with the syntax and meaning of a composite's (`project-format.md` §3: inputs fan out, one source per output, parameters fan out, groups):

    [exports]
    inputs     = { position = "transport.follow", jump = "transport.follow_jump" }
    outputs    = { l = "out.l_out", r = "out.r_out", drums = "bus.out" }
    parameters = { bpm = "transport.bpm", run = "transport.run", volume = "out.gain" }

- **Rendered alone,** a project's exports mean nothing. They are checked (each names a port that exists) and listed by `describe`, and change no render: hashes are unchanged except the file's own.
- **No `[exports] outputs`:** an included project then exports its audio sink's channels, named after the sink's inputs (`l`, `r`), from the tap that replaces the sink (§3). Any project can therefore be included without editing it.
- **An unconnected exported input** is an unconnected input inside: `is_connected` is false and the input reads 0, as for any composite. The included project falls back to its own value (§5).
- **In a slot,** a project's exports, or its default sink exports `l`, `r`, are what the slot's interface is matched against (ADR 0037 §4). The `lib.deck` template matches every project's default outputs with no `[exports]`.

### 3. Sinks: the host's sink is the only device sink; no mixing by the host
- **The rule** (in `signal-project`): in an included project, every instance whose type is a sink is planned as that sink's **tap**. The tap is the type the sink's library registers for it (`Registry::sink_taps`, a name-to-name table, domain-free like `exclusive_inputs`). It keeps:
  - the same address, inputs, parameters and connections;
  - no sink kind;
  - signal outputs carrying exactly what the adapter would have taken from the sink.
- **A sink type with no registered tap** makes the project refused as an include, with the reason: "studio-av has a std.video_out, which has no tap and cannot play inside another project".
- **`std.audio_tap`** (`signal-std`, category `io`) is the tap of `std.audio_out`:
  - inputs `l`, `r` (audio); parameter `gain` as `std.audio_out`'s (`[0, 4]`, multiply);
  - outputs `l_out`, `r_out` (audio), `l_out = gain·l`, `r_out = gain·r`. This is the adapter's expression on the same parameter row, so its value equals what the included project's `audio.wav` records before conversion to f32.
- **The included project's level, limiter and gain stay part of its graph:** its `tanh_shaper`, its `dc_block`, its output `gain` (now the tap's).
- **No device clamp inside.** The clamp to [−1, 1] happens once, at the host's sink, as for any signal in the host. An included project's render, `audio.wav`, is not clamped either (`semantics.md` §8).
- **The host's own `std.audio_out` is the plan's one audio sink,** chosen by the audio adapter exactly as now (ADR 0022). Adding or removing an include never changes the run's sinks, so a live include is never refused for `SinksChanged`.
- **No implicit mix.** The host wires the include's outputs where it wants them: into a mixer (§9), an effect, or straight into its `audio_out`. An output it does not wire is still computed (every instance runs, ADR 0016) and heard nowhere. The schematic warns: no sink reachable.
- **In a slot,** the taps' outputs become the slot's outputs through the match (ADR 0037 §4). A tap not matched is computed and unheard; the schematic marks it.
- **Scopes, meters and canvases are adapters that read outputs by address** (ADR 0022; `scope.rs`, `vectors.rs`). They work inside an include at its re-rooted addresses (§10). A panel meter the included project bound to its sink's input `/out.l` binds, in the host, to the tap's `/drums/out.l_out`.
- **Future sink kinds** (DMX, video, network) each register a tap. Where several of a kind could share a device instead, that is that adapter's own decision, later.

### 4. Seeds and addresses: a generic scope root
- **`signal-graph`:** `InstanceDef` gains `scope: Option<ScopeDef { seed: u64 }>`, set by `signal-project` on an include instance (and, §11, on a composite that declares `[composite] seed`). Flattening gives every primitive instance under a scope root a **stream key** `(seed, address relative to its innermost scope root)`: `PlanInstance::stream: Option<StreamKey>`, `None` outside any scope root.
- **`signal-engine` and `signal-oracle`:** an instance's stream is `RngStream::derive(key.seed, key.relative)` when it has a key, else `derive(config.seed, address)` as now.
  - `/drums/kick/noise` in a host draws the stream `/kick/noise` draws when studio-transport is rendered alone, under studio-transport's seed.
  - Plans without scope roots are unchanged: every golden and every plan dump stays byte for byte (the field is skipped when `None`).
- **The host's seed and `--seed`** reach the host's own instances only. An include's seed is its file's, or the include's `seed` key.
- **Two instances of the same project with the same seed** draw the same streams. Driven alike, they play alike: that is what "the same project twice" means. Give one another `seed` to vary it.
- **Canonical order is preserved inside without any new rule.** Every port address inside an include starts with the same `/drums/`. Byte-wise comparison of `/drums/x` with `/drums/y` is the comparison of `/x` with `/y`. So every sum and event merge whose sources are all inside associates exactly as it does alone. So do the converters, which sit under their source's address (`convert.rs`), and the per-frame log order (`semantics.md` §9).
- **Nothing else in a primitive's computation depends on the address or the absolute frame.** `std.frames` and latent modules count from their own `prepare`. GPU placement does depend on the address (round-robin by instance address): an included GPU instance may land on another device, which changes timing and, for a module not declared `BitExact` across devices, its output.

**What is bit-identical, stated precisely.** Assume an include present in the plan the engine was built with: the host opened, Restart from Files, `signal render` of the host. Assume the host runs at the included project's `sample_rate`, no `set` overrides, its seed its own, and none of its exported inputs or parameters connected or set by the host. Then:

| What | Identical to |
|---|---|
| Every output, event and parameter value of every instance inside, frame for frame | The included project rendered alone, for any *k*-partition and thread count (ADR 0019 §2), and whatever the host does around it |
| Its log records | Its log alone, with addresses prefixed by `/drums` |
| The tap's `l_out`, `r_out` | The included project's own `audio.wav` channels: f32 of them is its `audio.wav` byte for byte, −0 included |
| The host's own instances not fed by the include | The host without the include |
| A host that wires one include's `l_out`, `r_out` straight into its `audio_out` (scale 1, gain 1) | The included project's `audio.wav`, equal as numbers. The one possible difference is the sign of a zero: the host's sum starts from +0 (`semantics.md` §3), so a −0 sample reads +0. It is counted in the test |
| Loaded into a slot | The plain instance under the slot's name: the plans are byte-identical (ADR 0037 §5) |

**Not identical, by design:**
- an include that joined live (§6): its insides start through the 5 ms fades of added instances, and what remembers its input, such as a filter or a reverb tail, keeps that difference;
- an include driven through its exports, or overridden by `set`;
- an include run at another rate than its `sample_rate` (§8): it equals itself rendered alone with `--sample-rate <host rate>`;
- the host's sums of several sources: in a plain connection fan-in they associate by source address, which depends on the include names. Use a mixer (ordered operands, §9) to make them independent of names;
- a run after a live edit inside the include, which lands at a frame the host chooses;
- anything after an engine-level failure (§14).

### 5. Driving an included project from outside
- **Exported parameters** are its knobs, set (`bpm = 128`), modulated by a wire into the export (by each parameter's combine), or bound by the host's panel.
- **Exported inputs** feed it.
- **The canonical use case, two transports: the playhead follows** (ADR 0037 §6).
  - **Each keeps its own:** do nothing. Each project's `lib.transport` runs from its own frame 0, at its own tempo, started and stopped on its own panel card (§10).
  - **The inner follows the host:** the content's `lib.transport` has the inputs `follow`, `follow_jump`, `follow_running`, `follow_tempo` and the parameter `release` (`std.playhead`'s, ADR 0037 §6). The project exports them as `position`, `jump`, `running`, `tempo` (and `release`), and its sequencers stay wired to `transport.position` and `transport.jump` as today. The host wires its transport into them, or into a `lib.deck` slot's same-named ports:

        { from = "transport.position", to = "drums.position" },
        { from = "transport.jump",     to = "drums.jump" },

    The inner's playhead then copies the host's position exactly, and its sequencers fire on the host's beats, on the frames a local follower would (ADR 0033 §3).
  - **Every start, stop and rewire of following is a `jump`** from the playhead on the sequencers' `reset`, so it reaches them as a seek: nothing fires retroactively, nothing twice, and the next note is on time. For a sequencer wired directly to a position whose source changes at a swap, `Inputs::sources_changed` (ADR 0037 §6) marks the rewiring and the sequencer seeks.
  - **When following stops** (the wire removed, or the slot port unmatched), what the playhead does is a function of the wiring: its `release` parameter, 0 by default (continue from the last followed position at its own tempo), 1 (return to its own clock's position). It is set, wired or bound like any parameter (ADR 0037 §6).
- **Other exported inputs, falling back to their own value: `std.prefer`** (`signal-std`, category `core`):
  - signal inputs `a`, `b`; event inputs `a_events`, `b_events`; outputs `out`, `events` and `switched`;
  - `out = a` when `a` is connected (`Inputs::is_connected`), else `b`, an exact copy;
  - `events` forwards `a_events` when that is connected, else `b_events`;
  - `switched` carries one `Trigger` on a selection change, or on `sources_changed` of the chosen input;
  - the flags come from the plan, so the choice changes only at a swap, at its frame (ADR 0033 §2).
  - The included project wires its own value into `b` and exports `a`. Alone, `a` is unconnected and `out` is `b` bit for bit. Included and driven, it is the host's.
  - It is the generic fallback, not the transport idiom.

### 6. Joining and leaving without a glitch: a live edit like any other
**Glitch-free is the requirement** (the owner, 2026-10-23). Adding `drums = { project = … }` to a running host's file, or removing it, is a live edit (`semantics.md` §12).
- The new plan is built and prepared on the control thread. The swap is committed between blocks at the edit frame, allocating nothing on the audio thread.
- **Every other instance survives:** same address, same type, so state, random stream, ramps and fades move unchanged.
- **The newcomer fades in through the existing rule.** Every added instance's `audio` outputs fade in over F frames (5 ms; 240 at 48 kHz), exact in integers. This includes the tap, so the include's boundary always fades in, even when something inside starts with a step from a non-audio source such as a `std.const`.
- **A leaving include is suspended at *s* and removed at *s* + F** (the suspend-then-remove protocol `live.rs` already runs). Its audio outputs reach exactly +0.0 before its terms leave the host's sums, so the host's sums are bit-unchanged when they leave.
- **Slots use this protocol.** Loading, unloading and replacing a slot's content are these edits; Replace is unload-then-load, with no state kept (ADR 0037 §6).
- **No fade is hidden policy.** The 5 ms fade is the engine's documented semantics for any added or removed instance, not an include rule. Longer, musical fades are modules: the host's mixer channel `level` (§9), moved from the panel as a ramp, or driven by a wire.
- **No second fade-on-connect mechanism** is added. The added-instance fade already prevents the click, and a second fade triggered by "became connected" would double it and depend on plan history.
- **Timing:** a joining include with its own transport starts at its beat 0 on the join frame. To land in the host's time, wire the host's position (§5). A quantised join, at the next bar of a chosen position, is later.
- **Existing sound is bit-continuous across the swap.** Proven by the tests: the host's own outputs across a join, and the host's audio across the join of an include into a mixer channel at level 0.

### 7. A live reference
- **The included project's files are watched with the host's:** every project file of §1 of each include (and of theirs, recursively), and a `.signal` include's file.
- **An edit there is a live edit of the host:** re-planned whole, swapped at a frame, survivors keeping their state. Instances inside the include survive across its own edits exactly as they would in the project alone.
- **An include that stops loading** keeps the host on its last good plan, as any failed reload does. The diagnostic is shown in the host and located in the included file.
- **Missing or refused at open:** the host does not load, with the diagnostic at the include entry, as for an unknown composite.
- **A `.signal` include** is unpacked read-only into a temporary folder for the run (ADR 0025) and re-read when the file changes.
- **`signal vendor <host> --include <key>`** is the copy alternative (§11).

### 8. Rules
- **Sample rate** (agrees with ADR 0036 §1):
  - live play runs at the output device's rate, the host and every include with it;
  - `[project] sample_rate` is the rate of offline renders and replays and the preferred device rate;
  - an include runs at its host's rate (offline the host's `sample_rate` or `--sample-rate`, live the device's). Its own `sample_rate` is noted in `describe` and the inspector ("studio-transport is written for 48000 Hz and plays at 96000 Hz here"), never an error;
  - no resampling. Resampling an include, as a clock-domain adapter (roadmap M6), would be a later and explicit decision; it would not be bit-identical.
- **Cycles:** a project that includes itself, directly or through other includes or slot loads, is refused at the entry that closes the cycle, naming the chain by canonical paths (`realpath`): "two-studios → studio-transport → two-studios". The same project included twice, not in a cycle, is fine.
- **Depth:** includes and project loads nest up to **8** levels. Deeper is refused (a guard, not a semantic limit).
- **Paths:**
  - relative to the naming file's project folder;
  - an absolute path is allowed with a warning (not portable);
  - a symbolic link is followed for folders and resolved for the cycle check.
- **Library:** an include uses the host's build's library. There is one per process, and the manifest's `library_composite_hashes` covers all.

### 9. Mixing is a module: `lib.mixer`
- **No host-level mixing exists.** The host mixes with `std.mix` (four inputs, ordered operands, so independent of source addresses: ADR 0027 §1) or with the new library composite **`lib.mixer`**: four stereo channels `a`…`d`.
  - **Inputs** `a_l`, `a_r`, … per channel.
  - **Parameters** per channel: `a_level` (dB, a `std.gain`), `a_mute` (0 or 1, smoothed by a `std.lag` of 5 ms so muting never clicks), `a_balance` (−1…1), and a master `level`.
  - **Outputs** `l`, `r`.
  - Built from std primitives with ordered operands; pinned and in the `CHANGELOG.md` like every library composite.
- **A channel at level −120 dB (exact 0) or muted is bit-neutral:** the host's output is the same as with the channel unconnected, equal as numbers. The one possible difference is the sign of a zero, when every other term is −0 (counted in the test).
- **A musical fade-in of a newcomer** is that channel's level, moved from the panel (a ramp) or by a wire (a `std.line` into the level).

### 10. Signal.app, panels, schematic, parameters, MCP
- **One project, one Host, one engine, one UI server.** Nothing in ADR 0023 or 0032 changes structurally.
- **Addresses:** an include's instances are host addresses (`/drums/transport/playhead.bpm`) everywhere: `describe`, `graph`, `set`, meters, scopes and MCP. `signal mcp` sees the host's graph and its guard covers it. Its `file.write` stays scoped to the host's folder, so it cannot write an included project.
- **Panels:**
  - each include with a `ui/main.html` contributes its panel to the host's Panel tab: its cards follow the host's own, under a title card "DRUMS — studio-transport (linked)";
  - the included panel is served under `/include/<key>/` and rendered by a `<signal-panel root="/drums">` that prefixes every binding;
  - card ids are `drums:<id>`, so cards move between windows (ADR 0032) and the host's `ui/windows.toml` records them;
  - an include without a panel contributes the default panel of its exported parameters;
  - a slot's content contributes its cards under "DECK_A: lib.deck ◂ studio-transport", and an empty slot shows one card, "empty: Load…" (ADR 0037 §7).
- **What a control changes** (decided by the owner, 2026-10-23: panel edits go into the host):
  - an exported parameter writes the host's include entry (`bpm = 128`);
  - any other top-level parameter of the included project writes the entry's `set` table (`set = { "transport.run" = 1 }`) with `toml_edit`, as the parameter panel writes `project.toml` today;
  - Reset to default removes the key;
  - deeper parameters are read-only, as inside any composite (`project-format.md` §9).
- **Structure, patterns and panel layout of an include are read-only in the host:** the schematic's Edit patch inside it, the piano roll and step grid on its patterns, and Edit layout on its cards. The reason given is "linked project: open studio-transport to edit it" (File ▸ Open Included Project, §12).
- **Schematic:**
  - an include is a composite box with a link marker and its project name and path in the inspector;
  - its exports are its ports, grouped as written;
  - semantic zoom opens it like a library composite (ADR 0027 §6), read-only, laid out from its own `ui/schematic.toml`, with "Open studio-transport" and "Vendor into this project";
  - in a slot, the content opens inside the slot box, and an empty slot is drawn hollow (ADR 0037 §7);
  - the `graph` message gains `link: {path, name}` on that box and levels `include.<key>…` (an added field).
- **Parameters tab:** an include's exported parameters are editable like a composite's. The rest of its top-level parameters are editable as overrides (marked "override", with Reset).
- **Presets:** an include's exports and `set` are part of the host's file, so they are part of any future host preset. The included project's own presets stay its own.
- Slot boxes, the Load dialog, slot cards and the slot MCP tools are ADR 0037 §7.

### 11. Copying instead of referencing: `signal vendor --include` and `--slot`
`signal vendor <host> --include <key> [--as <name>]` turns a live reference into the host's own text, with ADR 0027 §4's all-or-nothing writes:
- the included project's `project.toml` becomes `composites/<name>.toml`, with `[composite] name = "<name>"`, its `[exports]` kept, and **`[composite] seed = <its seed>`**. A composite with a `seed` is a scope root (§4), so the copy draws the same streams;
- its sinks become their taps textually;
- its composites, interfaces and patterns are copied as `<name>_<x>`, with references rewritten;
- the include entry becomes `module = "<name>"`, with `set` folded into the copied entries;
- its panel is copied to `ui/<name>.html`, which the host's panel then includes with `root`.
- `signal vendor <host> --slot <key> [--as <name>]` does the same for a slot's project content and rewrites the slot's `load` to the copy (ADR 0037 §8).
- The host renders bit-identically after vendoring (tested). `[composite] seed` is generic: any composite may declare a scope root.

### 12. Locks, opening and editing (relation to the multi-instance decision)
- **Including is read-only and takes no lock.** A project may be open (and locked) in one Signal.app instance while any number of hosts include it. Its saves reach every host live (§7). This is the intended way to edit a linked project while hearing it in context.
- **The host writes only its own files** (§10), so two instances never write one file.
- **File ▸ Open Included Project** opens the include as a project, in a new instance if this one is playing the host, taking its lock as any open does.
- In-place structural editing of an include through the host (which would write its files and need its lock) is not in v1.

### 13. Packing, sessions, provenance
- **`signal pack`:** includes inside the host's folder are packed at their relative paths (§10 of `project-format.md` gains "the files of included projects inside the folder").
  - Includes outside the folder make `pack` refuse, naming them, unless `--embed`. With `--embed`, each is packed under `includes/<key>/`, and the archive's `project.toml` has those `project` (or `load`) values rewritten to `includes/<key>` (only those values, with `toml_edit`): the archive renders bit-identically to the folder.
  - Signal.app's Save As… embeds, saying so.
  - A `.signal` include is embedded unpacked.
- **Manifest:**
  - `project_file_hashes` gains each include's files under `include:<key>/<path>`;
  - a new `included_projects` field (path, name, seed, its `project_file_hashes` digest) is absent when there are none, so every existing manifest and golden is unchanged.
- **Sessions:** each plan version's copy holds the include files under `includes/<key>/`, and replay plans from them (`session-format.md` §3).

### 14. One engine: failures, CPU, edit cost
- **Failure isolation is the trade-off of one engine:**
  - a panicking or faulting instance inside an include is contained per instance, as anywhere;
  - an engine-level failure (`semantics.md` §14: an engine bug or invariant) stops the host and everything in it;
  - a hang anywhere stalls the callback (detected, not contained; hard rule 2);
  - an include that fails to load never stops a running host (§7).
- **CPU:** one plan; the scheduler's tasks span host and includes, so the worker pool parallelises across them (ADR 0019); the worker budget per plan (M2.10-5) covers the whole. An unwired include still costs its full CPU.
- **Edit cost:** any edit, in the host or an include, re-plans the whole host and prepares only added instances.
  - *Assumption, to measure in phase 2:* planning two studios is well under 100 ms on the control thread.
  - If not, incremental planning per scope root is the remedy, a later decision.

## Alternatives considered
- **Several engines and a mix runner, with `set.toml`** (the previous draft):
  - for: per-project failure isolation, different rates later, a project's computation untouched by construction;
  - against (rejected by the owner): a second runtime model, a mix that is policy outside the graph, buses as a new transport, several Hosts, servers and menus;
  - kept as a possible later layer if hard isolation between unrelated projects is ever needed (two shows on one machine).
- **Includes without a scope root** (inner randomness re-derived from the host's seed and the full address): simpler, but an included project would no longer sound as it does alone wherever it uses randomness, and renaming an include would change its sound.
- **A project-relative seed rule in the core** ("instances under a project"): the same mechanics with a domain word in the core (hard rule 1). The scope root is the generic form, also used by vendored copies (§11).
- **Keeping the included `std.audio_out` as a second sink and letting the adapter mix sinks:** host-level mixing, which the owner ruled out. It would also change the one-sink rule and `SinksChanged`.
- **Giving `std.audio_out` thru outputs:** changes a std descriptor every project uses. The tap is substituted only where it is needed.
- **A graph-level default source on exports** ("this inner connection holds unless the export is connected"): exact too, but a new flattening rule. `std.prefer` uses the existing `is_connected` and shows on the schematic.
- **`std.prefer` before the sequencers as the transport idiom** (an earlier draft): `follow` stays wired to `prefer.out` while the position under it switches, so a sequencer never sees a seek, and a forward switch of less than a beat fires every skipped step at once. Replaced by the playhead follow idiom (§5, ADR 0037 §6).
- **A fade-on-connect mixer channel:** see §6. The engine's added-instance fade already guarantees no click.
- **A bit-identical live join** (inside a newly added scope root nothing fades except the tap): the joined include would equal its standalone render shifted by the join frame. It needs a new swap rule, and an export not through a tap would then step. **Rejected by the owner** (2026-10-23): a glitch-free join is enough.
- **Writing the included project's files from the host:** convenient, but two writers and a lock across projects. Overrides in the host (`set`) keep each file single-writer.
- **Broadcasts, or automatic role-based connections, for transport following:** replaced by slots and the playhead follow idiom (ADR 0037, Alternatives).

## Consequences
- **`signal-core`:** `Registry::sink_taps` (register, query). `Inputs::sources_changed` is ADR 0037's.
- **`signal-graph`:**
  - `InstanceDef::scope`; flattening computes `PlanInstance::stream: Option<StreamKey { seed, relative }>`, serialized only when `Some`;
  - qualified definition names accepted as opaque strings (already the case for `lib.`).
- **`signal-engine`:** `engine.rs` derives the stream from the key; `domain_scope.rs` gains `project` and `include`.
- **`signal-oracle`:** the same derivation (`oracle.rs`), so oracle parity holds.
- **`signal-std`:** `std.audio_tap` (registered as `std.audio_out`'s tap) and `std.prefer` with its `switched` output, with contract tests; `std.playhead`'s follow inputs and `release` (ADR 0037 §6).
- **`signal-lib`:** `lib.mixer`, pinned, in the changelog's current section; `lib.transport` exports the playhead's follow inputs and `release` (ADR 0037 §6).
- **`signal-project`:**
  - `model.rs`: `project`, `seed`, `set` keys, `[exports]` in `project.toml`, `[composite] seed`;
  - `lib.rs`: resolution, qualified definitions, tap substitution, default exports, the rate note, cycle and depth check, and the slot resolution (ADR 0037 §5);
  - `locate.rs`: diagnostics in included files; `schema.rs`;
  - `live.rs`: the watched file set includes includes;
  - `diff.rs`; `archive.rs` (`--embed`, `.signal` includes); `session.rs`; `provenance.rs`;
  - `vendor.rs`: `--include` and `--slot`.
- **`signal-ui`:**
  - `host.rs`: the watch, read-only refusals for include files;
  - `panel.rs` and `bindings.rs`: rooted bindings, meters on taps;
  - `panel_write.rs`: overrides into `set`;
  - the `graph` message's `link`; `describe` lists include panels.
- **`ui/`:** `<signal-panel root>`, the include title card, the schematic's link marker and read-only levels, and the Parameters tab's override rows.
- **`signal-app`:** File ▸ Open Included Project.
- **`signal-cli`:** `pack --embed`, `vendor --include`, `vendor --slot`.
- **Docs:**
  - `project-format.md` §2–§4, §7, §10 (§2's `sample_rate` wording already matches ADR 0036);
  - `semantics.md` §11 (streams per scope root);
  - `module-contract.md` (sink taps);
  - `ui-controls.md`, `ui-protocol.md`, `session-format.md`;
  - `00-vision.md` §3a: no new structural term for this ADR ("an included project is a composite instance whose type is another project"); Slot and Interface are ADR 0037's;
  - roadmap M3.2.
- **Goldens:** every existing render and manifest is unchanged. New (the operator's choice, keeping both): `projects/two-studios`, plain includes of `../studio` and `../studio-transport` through `lib.mixer`, in phase 1 before slots exist; and `projects/two-decks`, the same through two `lib.deck` slots (ADR 0037).

## Tests
1. **Identity:** `projects/two-studios` rendered offline. The tap outputs of each include (captured through the scope adapter) converted to f32 equal that project's own `audio.wav` byte for byte. Checked for *k* = 1, 64, 256 and threads 1, 2, 4, 8.
2. **Straight-through:** a host whose only content is one include wired into `audio_out` equals the included project's `audio.wav` as numbers, with sign-of-zero differences counted (expected 0 on studio and studio-transport).
3. **Seeds:** a project using `std.noise` and `std.random` is identical included and alone. With another `seed` it differs; two same-seed includes are identical; `--seed` on the host leaves includes unchanged.
4. **Canonical order:** a fixture whose sums would associate differently under another prefix (sources `/a…`, `/z…` and a converter) is bit-identical included under `/m`.
5. **Existing sound is bit-continuous across a join and a leave** (null driver, the real-time runner, seeded callback sizes, threads 1 and N):
   - the host's own outputs (every instance not fed by the include, through the scope adapter) are bit-identical to a run without the edit;
   - with the include wired into a `lib.mixer` channel at level −120 dB, the host's device samples are bit-identical to a run without the edit (sign-of-zero differences counted, 0);
   - the newcomer's tap output follows the F-frame fade exactly;
   - its removal reaches +0.0 at *s* + F − 1;
   - no allocation on the audio thread during the swap (counting allocator).
6. **Driving:** studio-transport with the follow exports renders alone bit-identically to itself without them. Followed from a host transport through playhead follow, every drum hit lands on the frame a local follower's would (`follow.rs` style). A switch (start, stop, rewire to another transport) at every phase of a beat gives no merged or double hits and nothing retroactive, for `release` 0 and 1.
7. **Live reference:** an edit of the included `patterns/drums.toml` swaps the host, with every survivor's state kept. A broken included file keeps the last good plan, with the diagnostic at its line and column.
8. **Refusals:** a direct and an indirect cycle, depth 9, a sink without a tap, and `pack` with an outside include without `--embed`.
9. **Rate:** a 48 kHz include in a 96 kHz host equals it rendered alone with `--sample-rate 96000`, and `describe` notes its rate.
10. **Slot equivalence:** the same project as a plain include and in a `lib.deck` slot gives byte-identical plans.
11. **Packing:** `pack --embed` and unpack render bit-identically to the folder; `vendor --include` and `vendor --slot` render bit-identically to the reference.
12. **UI:**
    - `signal ui check` passes on a host with included panels;
    - an override written from an include card lands in `set` and survives a reload unchanged;
    - include structure edits are refused with the reason.
13. **Device:** 10 minutes of two-studios on the device, with an include added and removed every 30 s: 0 xruns.
14. **The owner's listening verdict.**

## Phases
| Phase | Delivers | Exit criteria |
|---|---|---|
| 1. Offline core | Scope root (graph, engine, oracle), `[exports]` in `project.toml`, `project` instances (folders), tap substitution with `std.audio_tap`, rate note, cycle and depth checks, diagnostics in included files, manifest fields, `projects/two-studios` | Tests 1–4, 8 (all but the pack refusal) and 9 pass; every existing golden unchanged; the determinism matrix green |
| 2. Live | Watching includes, live join and leave, playhead follow, `sources_changed`, `std.prefer` with `switched` (with ADR 0037 phase 2), edit-cost measurement | Tests 5–7 and 10 pass; planning cost recorded in `docs/benchmarks.md` |
| 3. Mixing and UI | `lib.mixer`, rooted include panels and cards across windows, `set` overrides, schematic link marker and read-only levels, Parameters overrides, Open Included Project | Test 12; the operator's panel and schematic checks in Signal.app |
| 4. Files | `.signal` includes, `pack --embed`, sessions and replay with includes, `vendor --include` and `--slot` with `[composite] seed` | Test 11; a session with a live join replays bit-exactly |
| 5. Verdict | Device run | Test 13; the owner plays two studios together (each on its own transport, then one following the other, a third joining and leaving) and records a verdict, with ADR 0037 phase 4 |

## Notes

**2026-10-23, phase 1 (offline) implemented** (operator; decisions delegated). Where this ADR left a choice open, or was not consistent, phase 1 decided as follows; the text above stands otherwise.
- **`projects/two-studios` mixes with `std.mix`** (two of them, ordered operands, 0.5 each), since `lib.mixer` is phase 3's; the Goldens paragraph's "through `lib.mixer`" applies from then, as a new golden. It includes `../studio` and a **copy of `projects/studio-transport` as committed in M3.1**, in the host's own folder (`projects/two-studios/studio-transport/`): an include's files are part of its host's manifest (§13), so including the live studio-transport would move two-studios' golden whenever that project is edited.
- **Paths** are relative to the naming file's **project** folder (§8), for a composite's file too; §1's "the folder of the file that names it" means that folder.
- **Qualified names** are `<canonical folder>/<name>`, the root `<canonical folder>/project.toml`: a canonical path is the project's identity (§8) and always holds a `/`. Diagnostics print names as written, never qualified.
- **`Registry::sink_taps`** is `Registry::set_sink_tap(sink, tap)` and `Registry::sink_tap(sink)`. A tap's output for the sink's signal input `x` is named `x_out` (`std.audio_tap`: `l_out`, `r_out`), which is what the default exports `l`, `r` read; `module-contract.md` "Sink taps".
- **A primitive instance cannot be a scope root** (`GraphError::ScopeOnPrimitive`); a converter draws nothing and gets no stream key.
- **`set`** is planned as a copy of the include's root with the overrides on its instances, one copy per entry that has them; the plan is the same as if the included file said so.
- **The rate note** is `signal render --json`'s `notes` (and a `note:` line on stderr), and each include's `note` in `describe`'s new `content` field (`content.includes`, with `content.slots`); never an error.
- **The manifest**: `project_file_hashes` keys `include:<key>/<path>`, `<key>` the including instance's name (`<composite>.<instance>` inside a project composite), an include's own includes nested the same way; `included_projects` entries carry `key`, `path` (as written), `name`, `seed` and `files_fnv1a64` (the FNV-1a 64 of the `<path> <hash>` lines of its files in path order).
- **Until phase 2,** a project read from files in memory (Signal.app's in-memory edits, `Project::from_sources`) refuses an include ("cannot include another; load it from its folder"); loading from a folder, rendering and playing work. **Until phase 4,** a `.signal` include is refused with the reason.
- **`domain_scope.rs`** checks `project`, `projects`, `include`, `includes` and `included` (and ADR 0037's spellings) in code lines of `signal-core`, `signal-graph` and `signal-engine`.
- **2026-10-23, after the review of phase 1:**
  - cycle chains name the canonical paths, as §8 says; the copy a `set` makes is named by a hash of where its entry is written, so no folder of the machine shows in a diagnostic;
  - a live edit that changes an include's `seed` (the stream key of a surviving instance) is refused with "restart the project", like a changed `[project] seed`; making it a remove-and-add would need the engine's survivor rule to compare stream keys, a later decision if wanted;
  - until phase 4, `signal pack` refuses a project with includes, and a session recorded from one carries `included_projects` and is refused by replay;
  - `[exports] outputs` may name a sink's tap output (`out.l_out`, the §2 example): alone it is checked against the tap;
  - every reserved key (`module`, `project`, `seed`, `set`, `slot`, `load`, `map`, with `gpu` and `pattern`) is refused as an exported parameter's name;
  - **for phase 2:** the MCP gain guard (`signal mcp`'s `--max-gain`, ADR 0029) must also cover a `std.audio_tap`'s `gain` inside an include, which a host can reach through `set` or a wire, since an include's level now reaches the host's sink through it.

