# ADR 0011: Audio I/O via cpal behind a driver trait

- **Status:** Accepted. Amended by ADR 0022 (M2.7-9): the driver trait and the null driver live in `signal-output`, with the audio adapter.
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

## Notes
- 2026-10-11 (M2-4): cpal 0.18.2 provides the gaps this ADR anticipated for buffer size: the IO buffer-size range (`SupportedBufferSize::Range`, from `kAudioDevicePropertyBufferFrameSizeRange`) and a fixed buffer (`BufferSize::Fixed`, set through `kAudioDevicePropertyBufferFrameSize`), plus the supported sample rates and overload reporting as `ErrorKind::Xrun` (`src/host/coreaudio/macos/device.rs`, `mod.rs`). `CoreAudioDriver` needs no `objc2-audio-toolbox`. AudioWorkgroup remains unverified.
