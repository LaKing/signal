# ADR 0040: Several writers on one project folder

- **Status:** Accepted (operator decision, 2026-10-24, a technical call the owner delegates). It answers two data-loss defects an external audit reproduced on 2026-10-23. It builds on ADR 0009 (text is the truth), ADR 0025 (project files), ADR 0027 §4 (`signal vendor`) and ADR 0029 (`signal mcp`, the write paths and their `base` checks). It supersedes nothing: the file formats and the UI protocol's messages are unchanged; `commit` and `retry` gain a `conflict` refusal (item 5).
- **Date:** 2026-10-24
- **Partly superseded by [ADR 0048](0048-the-control-thread-never-waits-for-the-lock.md)** (2026-10-31): on the UI host's control thread a writer no longer waits for the lock (item 2's "Waiting"), and a thread holding it lets its own writers re-enter it (item 2's "Re-entry"). Everything else stands.

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
   - hard-linked to a new name, which never replaces a file (`Staged::create`). On a file system without hard links (exFAT answers `ENOTSUP`), the bytes are instead copied into the new name, created exclusively, which never replaces a file either.

   An entry already at a name is left alone and the next name is taken. A writer removes only the staging file it created, so another writer's file is never deleted, renamed or written through. `write_atomic` is `stage` plus `replace`. `panel::write_file`, `archive::write_atomic`, `keys::write_atomic`, `Setup::save` and vendoring's writer all use it. The temporary name is not semantics (rule 3): it never reaches a file, render or log.
2. **One writer at a time per project.** `ProjectLock` is an advisory `flock` taken through `std::fs::File::lock` on the project folder itself, so no lock file is left in the person's folder and no `unsafe` is needed. A writer takes it before its conflict check and holds it until its files are written.
   - **Who takes it:** `write_project` (commit, the M2.5 panel, `retry`), `patch`, `file.write`, `pattern.write`, `panel.write`, `schematic.*`, the `ui/windows.toml` layout writes, and `vendor::write`.
   - **Who does not:** packing a `.signal` working folder on Save, and `Host::save_as` packing a folder. A working folder is private to its process. Packing can take long (and may run on a background save thread), so holding the lock would stall every control-thread writer for the whole pack, and fail them after 10 s. A pack's consistency is the save's own check (pack again until the folder has settled), not the lock's.
   - **Waiting:** a writer waits at most 10 s, then fails with `write-failed`.
   - **Release:** explicit when the guard drops, so a child process forked meanwhile does not keep the lock. The system also releases it when a process ends.
   - **No lock from the volume:** a file system with no locks (`Unsupported`, `ENOTSUP`, `EOPNOTSUPP`, as some network and FAT volumes answer), or one with no lock available now (`ENOLCK`, a lock table full or a lock daemon away), gives a lock that holds nothing. A warning is printed once per process and the writer goes on unlocked; every writer asks again, so `ENOLCK` lasts only while the volume answers it. `signal mcp`'s `project_info` says it (`writerLock: false`). What holds unlocked: every writer still compares what it writes (item 5), so a change made before its last read is refused, but that read and the rename are not one step, so a writer that renames in the instant between them is not seen and one of the two edits is lost. With the lock, Signal's writers never interleave, and only an editor (which takes no lock) can land in that instant.
   - **Re-entry:** the lock is per open file, so a writer must not take it twice. Writers that hold it never call another writer that takes it.
3. **Vendoring revalidates.** Under the lock, `vendor::write` first reads the project's files again. Any file changed, added or removed since `prepare` refuses the commit with `VendorError::Changed`, and nothing is written. The UI protocol maps it to `conflict`, and `signal vendor` reports it. The host also checks that the `project.toml` it prepared from is the version `base` names. Rollback writes an original back, or removes a new copy, only while the file still holds exactly what vendoring wrote, so an edit made in between is kept.
4. **`patch` checks every file it writes.** A patch that moves a position writes `ui/schematic.toml` as well as the level's file. Under the lock, it reads the positions file again, and if the text differs from the text the patch was built on (a `schematic.move` from another process meanwhile), the patch is a `conflict` and nothing is written.
5. **Every writer compares just before its rename** (review of 2026-10-24). `transaction::replace_unchanged` stages the new bytes, reads the file once more, and renames only while it still holds the bytes the writer's check (or its edit) was based on.
   - `write_project` (commit, the panel, `retry`) has no `base`: its edit is based on `project.toml` as it reads it under the lock. In a project that includes others (ADR 0035) the running project does not take the commit (`WriteError::Includes`): rebuilt from the folder, it would take an included project changed on disk but not reloaded yet, and the reload would then never apply that change; the watch's reload applies the commit with it. A file that no longer holds that text is `conflict` (`panel::ProjectWriteError::Changed`), nothing is written, and the value stays live and unsaved.
   - `patch` (its level's file and `ui/schematic.toml`), `file.write`, `pattern.write` and `panel.write` compare with the text their `base` check read; a change is their `conflict`, as at the check. A positions file changed in that instant (before its rename, or before its removal) puts the level's file back and is `conflict`. The level's file is put back only while it still holds the patch; one changed meanwhile keeps the other writer's edit and is `conflict` too.
   - The writers without a `base` (`schematic.*` and the `ui/windows.toml` layout writes) use `transaction::edit_text`: a file changed in that instant is read and edited again, at most 3 times, so the other writer's edit is kept.
   - `vendor::write` compares every project file once, before its first write (item 3), not before each rename.
   - `schematic.reset` without a level removes the file whatever it holds: that is what it means.

## Consequences
- Two Signal processes on one project no longer lose or mix each other's bytes. A version check and the write that follows it are one step for every Signal writer.
- A person's editor takes no lock. Its edits are still caught by the byte comparisons (`base`, the comparison just before each rename, and vendoring's revalidation), but an editor that saves in the instant between the last read and the rename can still be overwritten. Closing that gap would need every editor to cooperate, so it stays open.
- **Where hard links are missing, a new file shows while it is copied.** `Staged::create`'s copy fallback creates the new name exclusively and then copies the bytes into it, so for that moment the name holds part of the file: a watch may see it briefly (it reloads again once the copy ends), and a crash leaves it truncated under its own name. Only new files (vendoring's copies) take this path; a replaced file is always renamed whole.
- **A background save can pack a vendoring half-written.** Packing a `.signal` working folder takes no lock (item 2), so a pack running while `vendor::write` writes several files can hold some of them new and some old. The folder stays marked edited, so the next save packs it again, whole; until then the `.signal` file holds the mix.
- **Stale staging files stay.** A staging file left by a crashed writer is never cleaned up, because a writer cannot tell it from another writer's file in progress. It is hidden, so the watch and `pack` skip it, and it costs only disk space. Removing one is the person's call.
- A planted entry at an old staging name (`.<name>.tmp`) is now left in place rather than removed: a writer cannot tell a stale file from another writer's file in progress. The hidden name keeps it out of the watch and out of packed archives.
- Tests:
  - `crates/signal-project/tests/transaction.rs`: the two-writer interleaving, a foreign staging file, vendoring's conflict on an edited and on an added file, and the lock released;
  - `crates/signal-ui/tests/patch.rs` `a_position_written_meanwhile_is_a_conflict` (a test hook writes the positions just before the patch takes the lock);
  - unit tests in `transaction.rs` (staging apart, lock exclusion, `create` never replacing, a planted link at the next staging name skipped, the copy without hard links, which errors mean "unsupported", `replace_unchanged` and `edit_text` with a change before the rename), `vendor.rs` (rollback keeps an edit) and `panel.rs` (the auditor's interleaving through `write_file`; a commit refused when `project.toml` changes before its rename).

## Alternatives considered
- **A lock file (`.signal.lock`).** It works on any file system, but it leaves a file in the person's folder. Locking the folder itself needs no file.
- **`libc::flock`.** It needs `unsafe`, which is denied outside the audited modules (rule 9). `std` has offered `File::lock` since Rust 1.89, and the pinned toolchain is 1.99.
- **Random temporary names.** These would need a random source and would add nothing over pid plus counter, which are unique on one machine.
- **A full transaction journal (write-ahead, crash recovery for several files).** This would be a larger rewrite. The all-or-nothing writer with a careful rollback is enough for vendoring, the only multi-file commit.
