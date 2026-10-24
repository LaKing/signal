# ADR 0040: Several writers on one project folder

- **Status:** Accepted (operator decision, 2026-10-24, a technical call the owner delegates). It answers two data-loss defects an external audit reproduced on 2026-10-23. It builds on ADR 0009 (text is the truth), ADR 0025 (project files), ADR 0027 §4 (`signal vendor`) and ADR 0029 (`signal mcp`, the write paths and their `base` checks). It supersedes nothing: the file formats, the UI protocol's messages and their `conflict` refusals are unchanged.
- **Date:** 2026-10-24

## Context
Several writers can write one project folder at once:
- two `signal ui` or Signal.app processes;
- `signal mcp`, which runs a host of its own unless it attaches;
- `signal vendor`;
- a person's editor.

The audit found two defects.

1. **A shared staging name.** Every write through `signal_ui::panel::write_file` (commit, patch, panel, pattern, file and schematic writes) staged through the same `.<name>.tmp`, and first removed whatever was there. Two writers could therefore remove or rename each other's staging files. In a controlled interleaving, writer 1 returned `Ok(())` but had committed writer 2's unfinished one-byte file. `keys.toml` and the Setup used a shared `<name>.tmp` too, created with truncation.
2. **Vendoring committed stale text.** `vendor::prepare` read the project's files, and `vendor::write` later replaced them without checking that they were unchanged. An edit made in between (output gain 0.25 → 0.5) was overwritten with the prepared text, and the edit was lost. The rollback wrote the originals back in the same blind way.

The host's other write paths compared the file with `base` just before writing. Nothing stopped another process from writing between that check and the rename.

## Decision
`signal_project::transaction` is the one small layer every project writer uses.

1. **Staging files of the writer's own.** `stage` creates `.<name>.<pid>.<n>.tmp` beside the target. `n` is a counter of the process. The file is created exclusively (`O_EXCL`, which never follows a link), synced, and then:
   - renamed over the target (`Staged::replace`), or
   - hard-linked to a new name, which never replaces a file (`Staged::create`).

   An entry already at a name is left alone and the next name is taken. A writer removes only the staging file it created, so another writer's file is never deleted, renamed or written through. `write_atomic` is `stage` plus `replace`. `panel::write_file`, `archive::write_atomic`, `keys::write_atomic`, `Setup::save` and vendoring's writer all use it. The temporary name is not semantics (rule 3): it never reaches a file, render or log.
2. **One writer at a time per project.** `ProjectLock` is an advisory `flock` taken through `std::fs::File::lock` on the project folder itself, so no lock file is left in the person's folder and no `unsafe` is needed. A writer takes it before its conflict check and holds it until its files are written.
   - **Who takes it:** `write_project` (commit, the M2.5 panel, `retry`), `patch`, `file.write`, `pattern.write`, `panel.write`, `schematic.*`, the `ui/windows.toml` layout writes, `vendor::write`, and the packing of a `.signal` working folder on Save.
   - **Waiting:** a writer waits at most 10 s, then fails with `write-failed`.
   - **Release:** explicit when the guard drops, so a child process forked meanwhile does not keep the lock. The system also releases it when a process ends.
   - **Re-entry:** the lock is per open file, so a writer must not take it twice. Writers that hold it never call another writer that takes it.
3. **Vendoring revalidates.** Under the lock, `vendor::write` first reads the project's files again. Any file changed, added or removed since `prepare` refuses the commit with `VendorError::Changed`, and nothing is written. The UI protocol maps it to `conflict`, and `signal vendor` reports it. The host also checks that the `project.toml` it prepared from is the version `base` names. Rollback writes an original back, or removes a new copy, only while the file still holds exactly what vendoring wrote, so an edit made in between is kept.

## Consequences
- Two Signal processes on one project no longer lose or mix each other's bytes. A version check and the write that follows it are one step for every Signal writer.
- A person's editor takes no lock. Its edits are still caught by the byte comparisons (`base`, and vendoring's revalidation), but an editor that saves in the instant between a check and the rename can still be overwritten. Closing that gap would need every editor to cooperate, so it stays open.
- A planted entry at an old staging name (`.<name>.tmp`) is now left in place rather than removed: a writer cannot tell a stale file from another writer's file in progress. The hidden name keeps it out of the watch and out of packed archives.
- Tests:
  - `crates/signal-project/tests/transaction.rs`: the two-writer interleaving, a foreign staging file, vendoring's conflict on an edited and on an added file, and the lock released;
  - unit tests in `transaction.rs` (staging apart, lock exclusion, `create` never replacing), `vendor.rs` (rollback keeps an edit) and `panel.rs` (the auditor's interleaving through `write_file`).

## Alternatives considered
- **A lock file (`.signal.lock`).** It works on any file system, but it leaves a file in the person's folder. Locking the folder itself needs no file.
- **`libc::flock`.** It needs `unsafe`, which is denied outside the audited modules (rule 9). `std` has offered `File::lock` since Rust 1.89, and the pinned toolchain is 1.99.
- **Random temporary names.** These would need a random source and would add nothing over pid plus counter, which are unique on one machine.
- **A full transaction journal (write-ahead, crash recovery for several files).** This would be a larger rewrite. The all-or-nothing writer with a careful rollback is enough for vendoring, the only multi-file commit.
