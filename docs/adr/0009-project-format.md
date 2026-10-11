# ADR 0009: Project format: TOML plus generated JSON Schema

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Projects and composites must be readable, diffable, schema-validated, and safely editable by humans, agents, and the GUI.

## Decision
- Project and composite files are **TOML** (toml 1.x), with flat named instances and a connection list. Machine edits use `toml_edit` to preserve formatting and comments.
- The serde model is separate from the syntax. A JSON Schema is generated (schemars) from the model plus the primitive catalogue (`signal modules --json`).
- Every file carries a schema version. Migrations are tooling, not manual work.
- Machine settings (*Setup*: devices, channel mapping) live outside projects so projects stay portable.

## Alternatives considered
- RON: better nesting, smaller tooling ecosystem.
- KDL: nicest syntax, weaker serde story.
- YAML: serde_yaml is deprecated.

## Consequences
`docs/project-format.md` (M1) is normative.
