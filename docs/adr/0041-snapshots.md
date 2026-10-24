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

- **Names** are any text, trimmed, 1 to 64 characters (Unicode scalar values, so accents and other scripts count as one each), without control characters. They are TOML keys, quoted when they need it, so spaces, accents and quotes are fine. Names are exact: `Chorus` and `chorus` are two snapshots.
- **Values** are keyed by the parameter's full address, `/<instance>.<parameter>`: the address the UI protocol and `signal mcp` use for a control.
- **Canonical text.** Signal writes the file whole: the header, `schema_version`, then the snapshots in byte order of their names, each one's values in byte order of their addresses, each value in the shortest form that reads back to the same `f64` (an integer when it is one, as a parameter edit writes it). The same snapshots always give the same bytes, whatever order they were saved in, and saving the same values again writes nothing. A person may edit the file; it is read by the rules of §16 (unknown keys, bad names, non-finite values are errors), and a file that does not read is refused and never overwritten.
- **No time in it.** There is no `saved` field: a save time would be wall-clock time in a project file, and a deterministic counter would add nothing a name does not.
- **Not played.** `signal render`, `signal play` and the plan never read it; no render, golden or hash changes. A `.signal` file packs it (`project-format.md` §10), since it is the project's.
- **The last snapshot deleted** leaves the file with `schema_version` alone, not removed: `transaction` replaces files and has no compared removal, and an empty file says the same as no file.

### 2. Save: every control's value as it plays
`snapshot.save` records every control of the project, each with the value the engine plays for it now: its **live base** (`describe`'s `base`: the last value the Controller took for every port, so an unsaved live edit is saved as it plays; each port's default after a reset), defaults included. Restoring therefore also undoes a change made later to a parameter the file did not set. A modulated control is saved as its base (what the text says, or the live edit on top), never the modulated signal. A parameter inside a composite is in no snapshot (§Context).

Saving under a name in use is refused (`exists`) unless the message says `overwrite`; the UI asks "Replace?" first. `signal snapshot save` (the files alone) saves the files' values.

### 3. Restore: a parameter edit, as one change
`snapshot.restore` writes the snapshot's values where `commit` writes them and sends them live as `commit` sends them, through the same code (`Host::write_controls`, `panel::write_project_all`, `panel::after_write_all`):

- **One change.** Every value that differs from the file is written into `project.toml` in one read, edit and rename under the writer lock, compared just before the rename (ADR 0040). A file changed meanwhile is a `conflict` and nothing is written.
- **Live.** Each value is ramped to over the panel's 5 ms, as a committed value is, so there is no glitch beyond what a parameter change makes. The running project takes the same edit, so the watch's reload applies nothing twice. A control whose file already holds the value but whose live value differs (an unsaved edit) is only sent back to it. Afterwards nothing the snapshot names is unsaved.
- **Gone entries** (no such instance, no such parameter, a nested address) are skipped and listed in the reply (`skipped`, with the reason). **Out-of-range values** are clamped to the control's range, as `commit` clamps, and listed (`clamped`). Controls the snapshot does not name are left alone.
- **Failures** are a commit's: a write that fails keeps each value live and unsaved with its error (Retry and Revert apply); a refused live send stays unsaved until the reload sends the file's value.
- **Included projects (ADR 0035).** A snapshot holds the host's controls, and a restore writes the host's `project.toml` only, exactly as a parameter edit does: an included project's exported parameters are controls of the host, written into its include entry; the included project's own files are never written, and its own snapshots stay its own. As for any parameter write in a project that includes others (`WriteError::Includes`), the running project does not take the write in memory; the watch's reload applies it.
- **No undo machinery.** The app has none for parameter edits, so a restore is undone the way a commit is: by restoring a snapshot saved before it, or from the files' history (git). The Snapshots card's Save is one click away for that.

### 4. Delete
`snapshot.delete` removes one snapshot; the UI confirms first.

### 5. Writers (ADR 0040)
Every write of `snapshots.toml` goes through `signal_project::snapshot::edit`: under the project's writer lock it reads the file, checks the message's optional `base` (the SHA-256 the list was read with; `null`: no file yet), edits and writes the canonical text with `transaction::replace_unchanged`. A file changed by another writer meanwhile is a `conflict` carrying the current `base`, never a lost edit; a link is refused. The watch takes the written file as seen (it plays nothing, so nothing reloads), and every client is pushed the list.

### 6. Surfaces
- **UI protocol:** `snapshot.list`, `snapshot.save`, `snapshot.restore`, `snapshot.delete`, and the pushed `snapshots` (`ui-protocol.md` "Snapshots").
- **Signal.app:** a Snapshots card in the Parameters tab, before the parameter list: a name field with Save (Enter saves), a row per snapshot with Restore and Delete; Replace? and Delete? are asked in the page (Cancel focused, Escape cancels); the outcome, with what was skipped or clamped, in a line of fixed height (`ui/app/app.js` `SignalAppSnapshots`, `ui/app/lib/snapshots.js`).
- **`signal mcp`:** `snapshot_list` (inspect, kept under `--read-only`), `snapshot_save`, `snapshot_restore`, `snapshot_delete` (`agent-interface.md`). `snapshot_restore` is checked by the gain guard as a `commit` of each value.
- **CLI:** `signal snapshot list|save|restore|delete <project> [name]` works on the files alone; a restore writes `project.toml` as one change, which a running Signal applies through its watch.

## Consequences
- A person keeps and recalls sounds by name, in the project's own text, diffable and packed with it.
- A restore is as safe as a commit: the same write path, the same unsaved bookkeeping, the same conflict rule, the same gain guard in `signal mcp`.
- Parameters inside composites are not in snapshots. A person who wants one in a snapshot exports it from the composite (`[exports] parameters`), which makes it a control.
- Hand edits of `snapshots.toml` lose their comments at the next save.
- Tests: `crates/signal-project/src/snapshot.rs` (names, canonical text, refused files), `crates/signal-project/tests/snapshot.rs` (save, overwrite, delete, names with spaces and accents, byte-stable output, a conflict from another writer, a bad file and a link refused, packed into `.signal`), `crates/signal-ui/tests/snapshot.rs` over both transports (save, change, restore in one write with nothing unsaved; skipped and clamped entries; overwrite, conflicts, delete; the files-only restore), `crates/signal-cli/tests/mcp.rs` (`snapshots`), `crates/signal-cli/tests/mcp_gain.rs` (`a_snapshot_above_the_ceiling_is_not_restored`), `crates/signal-cli/tests/cli.rs` (`signal snapshot`), `ui/app/lib/snapshots.test.js` and `ui/app/lib/menu.test.js` (the card).

## Alternatives considered
- **Snapshots inside `project.toml`** (a `[snapshots]` table). Every save would then edit the file the engine plays and trigger a reload, and a snapshot would change the project's render hash. A file of its own changes nothing that plays.
- **One file per snapshot** (`snapshots/<name>.toml`). Names would have to be file names (no `/`, case folded on APFS), and listing or renaming would touch many files. One file keeps any name and one writer.
- **Nested tables per instance** (`[snapshots."Verse A".drums] kick_decay = 300`). Shorter for many parameters of one instance, but the address is what the protocol, `signal mcp` and the panel use; flat addresses can be searched for as they are written everywhere else.
- **Keeping comments with `toml_edit`.** The file is Signal's to write; a canonical text is byte-stable and simpler to review. Comments in `project.toml` are kept, as before.
- **Restoring every parameter, including those inside composites.** That would write composite files that every instance shares, which no parameter edit does.
