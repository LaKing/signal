# ADR 0011: Audio I/O via cpal behind a driver trait

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
We should not reimplement CoreAudio access. cpal 0.18 is active, Apache-2.0, and uses objc2 CoreAudio bindings; its unreleased changes include breaking renames (checked 2026-10-11).

## Decision
- Use cpal behind Signal's own small driver trait, with the version pinned.
- Drop to `objc2-audio-toolbox` only for gaps (e.g. IO buffer size, AudioWorkgroup; availability unverified).
- An offline driver and a null driver implement the same trait for tests.

## Alternatives considered
Direct objc2 CoreAudio: more control, more code.

## Consequences
cpal upgrades are isolated to one module.
