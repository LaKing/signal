# ADR 0041: Snapshots

- **Status:** Proposed (operator, 2026-10-24, from the owner's request; it becomes Accepted when the operator merges it). The owner's words of 2026-10-24: "A projektben legyen egy "Snapshots" feature. Minden paramétert mentsen el egy elnevezhető kulcs mögött. Ezeket lehessen menteni, visszaállítani, törölni." (translated: "The project should have a "Snapshots" feature. It should save every parameter under a nameable key. These should be possible to save, restore and delete.") The design is the operator's, delegated.
- **Builds on:** ADR 0009 and 0013 (text is the truth; the GUI edits files), ADR 0025 (project files, `.signal`), ADR 0029 (`signal mcp`, its gain guard), ADR 0035 (included projects and where their edits are written), ADR 0040 (several writers: `signal_project::transaction`). `docs/00-vision.md` §3a already names "Preset (or Snapshot): a stored set of parameter values for a module or project"; this is the project-wide one. It supersedes nothing.
- **Date:** 2026-10-24

## Context
A person who has found a sound wants to keep it under a name, try something else, and come back. Today the only way is to copy `project.toml` by hand. The values a person changes live are the **controls**: each top-level instance's own parameters (for a composite, or an included project, its exported ones). Those are what an ordinary parameter edit (`commit`, the panel's release) writes, into the instance's entry in `project.toml`. A parameter inside a composite is the composite's text, the same for each of its instances, and no edit writes it (`project-format.md` §9, "The parameter panel").

The app has no undo machinery for parameter edits: a commit writes the file and the reload applies it. A schematic `patch` returns its inverse, but that covers structure only.

## Decision

### 1. One file, `snapshots.toml`, in the project folder
Snapshots are text in the project (rule 4). The file is optional; a missing file means no snapshots.

```toml
# Snapshots: parameter values under a name (docs/project-format.md §16).
# Signal writes this file whole; comments are not kept.
schema_version = 1

[snapshots."Verse A"]
"/drums.kick_decay" = 300
"/out.gain" = 0.25

[snapshots.chorus]
"/drums.kick_decay" = 450
"/out.gain" = 0.4
```

- **Names** are any text, trimmed, 1 to 64 characters (Unicode scalar values, so accents and other scripts count as one each), without control characters, format characters (category Cf: zero-width spaces, the byte-order mark, direction marks) or line and paragraph separators, and not only blank characters: what a name holds is what it shows. They are TOML keys, quoted when they need it, so spaces, accents and quotes are fine. Names are exact: `Chorus` and `chorus` are two snapshots. **Names are not normalised** (a composed `é` and `e` with a combining accent are two names): no crate of the workspace's dependencies normalises Unicode (`icu_normalizer` is in `Cargo.lock` only as a transitive dependency; using it would be an operator's dependency step), and refusing the invisible characters removes the differences one cannot see apart from these.
- **Never through a link.** A `snapshots.toml` that is a symbolic link is neither read nor written: its target's text would reach every reader, quoted in errors too.
- **Values** are keyed by the parameter's full address, `/<instance>.<parameter>`: the address the UI protocol and `signal mcp` use for a control.
- **Canonical text.** Signal writes the file whole: the header, `schema_version`, then the snapshots in byte order of their names, each one's values in byte order of their addresses, each value in the shortest form that reads back to the same `f64` (an integer when it is one, as a parameter edit writes it). The same snapshots always give the same bytes, whatever order they were saved in, and saving the same values again writes nothing. A person may edit the file; it is read by the rules of §16 (unknown keys, bad names, non-finite values are errors), and a file that does not read is refused and never overwritten.
- **No time in it.** There is no `saved` field: a save time would be wall-clock time in a project file, and a deterministic counter would add nothing a name does not.
- **Not played.** `signal render`, `signal play` and the plan never read it; no render, golden or hash changes. A `.signal` file packs it (`project-format.md` §10), since it is the project's.
- **The last snapshot deleted** leaves the file with `schema_version` alone, not removed: `transaction` replaces files and has no compared removal, and an empty file says the same as no file.

### 2. Save: every control's value as it plays
`snapshot.save` records every control of the project (while nothing plays, the panel is first read again from the folder, so an edit by another program is what is saved), each with the value the engine plays for it now: its **live base** (`describe`'s `base`: the last value the Controller took for every port, so an unsaved live edit is saved as it plays; each port's default after a reset), defaults included. Restoring therefore also undoes a change made later to a parameter the file did not set. A modulated control is saved as its base (what the text says, or the live edit on top), never the modulated signal. A parameter inside a composite is in no snapshot (§Context).

Saving under a name in use is refused (`exists`) unless the message says `overwrite`; the UI asks "Replace?" first. `signal snapshot save` (the files alone) saves the files' values. A project that loads but does not plan has no controls: save and restore refuse it (`does-not-load`) rather than save an empty snapshot over a good one.

### 3. Restore: a parameter edit, as one change
`snapshot.restore` writes the snapshot's values where `commit` writes them and sends them live as `commit` sends them, through the same code (`Host::send_controls`, `Host::controls_written`, `panel::write_project_chosen`, `panel::after_write_all`):

- **One change.** Every value that differs from the file is written into `project.toml` in one read, edit and rename under the writer lock, compared just before the rename (ADR 0040). Which values differ is decided on the text read under the lock, not on a cached panel. A file changed meanwhile is a `conflict` and nothing is written.
- **Live.** Each value is ramped to over the panel's 5 ms, as a committed value is, so there is no glitch beyond what a parameter change makes. The running project takes the same edit, so the watch's reload applies nothing twice. A control whose file already holds the value but whose live value differs (an unsaved edit) is only sent back to it. `describe` is pushed once, after every value is written and sent, so every client shows what plays. Afterwards nothing the snapshot names is unsaved.
- **Gone entries** (no such instance, no such parameter, a nested address) are skipped and listed in the reply (`skipped`, with the reason). **Out-of-range values** are clamped to the control's range, as `commit` clamps, and listed (`clamped`). Controls the snapshot does not name are left alone.
- **Failures** are a commit's: a write that fails still sends every value, and each one the file did not hold stays live and unsaved with its error (Retry and Revert apply); the error names what was not written, skipped and clamped. A refused live send stays unsaved until the reload sends the file's value.
- **Included projects (ADR 0035).** A snapshot holds the host's controls, and a restore writes the host's `project.toml` only, exactly as a parameter edit does: an included project's exported parameters are controls of the host, written into its include entry; the included project's own files are never written, and its own snapshots stay its own. As for any parameter write in a project that includes others (`WriteError::Includes`), the running project does not take the write in memory; the watch's reload applies it.
- **Undo.** The app has no undo machinery for parameter edits; a restore has its own one-level undo, the undo snapshot (§7, the owner's decision, which supersedes the first draft's "no undo": restore a snapshot saved before, or the files' history).

### 4. Delete
`snapshot.delete` removes one snapshot; the UI confirms first.

### 5. Writers (ADR 0040)
Every write of `snapshots.toml` goes through `signal_project::snapshot::edit`: under the project's writer lock it reads the file, checks the message's optional `base` (the SHA-256 the list was read with; `null`: no file yet), edits and writes the canonical text with `transaction::replace_unchanged`. A file changed by another writer meanwhile is a `conflict` carrying the current `base`, never a lost edit; a link is refused. The watch takes the written file as seen (it plays nothing, so nothing reloads), and every client is pushed the list.

### 6. Surfaces
- **UI protocol:** `snapshot.list`, `snapshot.save`, `snapshot.restore`, `snapshot.delete`, and the pushed `snapshots` (`ui-protocol.md` "Snapshots").
- **Signal.app:** a Snapshots card in the Parameters tab, before the parameter list: a name field with Save (Enter saves), a row per snapshot with Restore and Delete; Replace? and Delete? are asked in the page (Cancel focused, Escape cancels); the outcome, with what was skipped or clamped, in a line of fixed height (`ui/app/app.js` `SignalAppSnapshots`, `ui/app/lib/snapshots.js`).
- **`signal mcp`:** `snapshot_list` (inspect, kept under `--read-only`), `snapshot_save`, `snapshot_restore`, `snapshot_delete` (`agent-interface.md`). `snapshot_restore` is checked by the gain guard as a `commit` of each value.
- **CLI:** `signal snapshot list|save|restore|delete <project> [name]` works on the files alone; a restore writes `project.toml` as one change, which a running Signal applies through its watch.

### 7. The undo snapshot (the owner's decision of 2026-10-24)
The owner's words: "legyen egy "undo snapshot"" ("let there be an undo snapshot"), on a restore having no confirmation and no undo.

- **Before every restore** the values playing now (the live bases, as a save records them) of the controls this restore can change (the snapshot's entries that exist and go into snapshots, nothing else, so an undo never reverts a later edit of another control) are written as the undo snapshot, `[undo]` in `snapshots.toml`, in the write that reads the snapshot. If that set is empty (every entry skipped), the undo snapshot is left as it was. If the write fails or is a `conflict`, the restore does not happen and the reply says why.
- **One lock for both writes.** The undo snapshot and `project.toml` are written under one writer lock (ADR 0040; `snapshot::edit_locked`, `panel::write_project_chosen_locked`). If the `project.toml` write then fails, `snapshots.toml` is put back to what it held before (a compared write, `snapshot::put_back`), so the undo snapshot to go back to is never lost; the error says so (`undoKept`). The operator's decision of review 2 (2026-10-24) set this scope.
- **Apart from the named snapshots:** a table of its own, not under `[snapshots]`, so no name is reserved and no person's name can be taken for it. Lists say whether it exists (`undo: true/false`) apart from the named ones.
- **Undo** (`snapshot.undo`, the card's "Undo restore", `snapshot_undo`, `signal snapshot undo`) restores it exactly as a restore restores a snapshot, after the values playing now take its place: pressing it again redoes. One level only: each restore replaces it.
- Restore keeps no confirmation (the owner's scope).
- Tests: `undo_restores_and_a_second_undo_redoes`, `a_failed_undo_write_blocks_the_restore` (`crates/signal-ui/tests/snapshot.rs`), `snapshots` in `mcp.rs`, the undo case in `mcp_gain.rs`, `snapshot_saves_restores_and_deletes` in `cli.rs`.

### 8. Which controls go into snapshots (the owner's decision of 2026-10-24)
The owner's words: "lehessen állítani hogy a snapshotokba bekerüljön vagy nem a tulajdonságaiban" ("it should be settable in its properties whether it goes into the snapshots or not"), asked about a transport's `run`.

- **Text, where the control is declared:** an optional list in the instance's entry in `project.toml`, `snapshot_exclude = ["run"]` (`project-format.md` §4), next to the control's value. Absent (every existing project) means every control goes in, so no project changes. Each name must be a parameter of the instance; in a composite's file the key is an error, and no composite may export a parameter named `snapshot_exclude` (reserved).
- **Not a value:** nothing reads it to play, and no render changes (only the file's own hash, where it is written). The running project takes the same edit of its own text (never the file's, which may hold an edit the watch has not applied yet), so the watch reloads nothing unless something else changed; in a project that includes others the running project is not rebuilt (`WriteError::Includes`) and the watch's reload applies it and pushes `describe`.
- **Checked under the lock:** the edit is checked on `project.toml` as read under the writer lock, the project loaded and planned with it, before the rename: a name that is no control, or a text that would not plan, is refused and nothing is written (never against a stale panel).
- **Effect:** save leaves an excluded control out; a restore (or undo) of a snapshot that still holds its value skips it (reason `excluded`); the undo snapshot does not hold it.
- **Editing:** the Parameters tab's row has a "snapshots" box (the control's properties there, beside Reset); `snapshot.exclude`, `snapshot_exclude`, `signal snapshot exclude`. Written with `toml_edit` (only that key changes; names in byte order; the key removed when empty) under the writer lock, compared just before the rename.
- Tests: `snapshot_exclude_is_text_next_to_the_control` (`crates/signal-project/tests/snapshot.rs`), `excluded_controls_survive_save_restore_and_undo` (`crates/signal-ui/tests/snapshot.rs`), `snapshots` in `mcp.rs`, `cli.rs`.

## Consequences
- A person keeps and recalls sounds by name, in the project's own text, diffable and packed with it.
- A restore is as safe as a commit: the same write path, the same unsaved bookkeeping, the same conflict rule, the same gain guard in `signal mcp`.
- Parameters inside composites are not in snapshots. A person who wants one in a snapshot exports it from the composite (`[exports] parameters`), which makes it a control.
- Hand edits of `snapshots.toml` lose their comments at the next save.
- Tests: `crates/signal-project/src/snapshot.rs` (names, canonical text, refused files), `crates/signal-project/tests/snapshot.rs` (save, overwrite, delete, names with spaces and accents, byte-stable output, a conflict from another writer, a bad file and a link refused, packed into `.signal`), `crates/signal-ui/tests/snapshot.rs` over both transports (save, change, restore in one write with nothing unsaved; skipped and clamped entries; overwrite, conflicts, delete; a mixed restore whose pushed `describe` shows every sent value; a stopped project edited by another program; a failed write that still sends and reports; the files-only restore), `crates/signal-cli/tests/mcp.rs` (`snapshots`), `crates/signal-cli/tests/mcp_gain.rs` (`a_snapshot_above_the_ceiling_is_not_restored`), `crates/signal-cli/tests/cli.rs` (`signal snapshot`), `ui/app/lib/snapshots.test.js` and `ui/app/lib/menu.test.js` (the card).

## Alternatives considered
- **Snapshots inside `project.toml`** (a `[snapshots]` table). Every save would then edit the file the engine plays and trigger a reload, and a snapshot would change the project's render hash. A file of its own changes nothing that plays.
- **One file per snapshot** (`snapshots/<name>.toml`). Names would have to be file names (no `/`, case folded on APFS), and listing or renaming would touch many files. One file keeps any name and one writer.
- **Nested tables per instance** (`[snapshots."Verse A".drums] kick_decay = 300`). Shorter for many parameters of one instance, but the address is what the protocol, `signal mcp` and the panel use; flat addresses can be searched for as they are written everywhere else.
- **Keeping comments with `toml_edit`.** The file is Signal's to write; a canonical text is byte-stable and simpler to review. Comments in `project.toml` are kept, as before.
- **Restoring every parameter, including those inside composites.** That would write composite files that every instance shares, which no parameter edit does.
