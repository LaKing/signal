# Security

Signal is a desktop application. It has no accounts and no cloud service, and it does not send anything over the network on its own. What it exposes is local: a UI server for its own window and pages, and an agent interface over stdio. The full rules are in [`docs/ui-protocol.md`](docs/ui-protocol.md) ("Security") and [`docs/agent-interface.md`](docs/agent-interface.md) ("Safety").

## The local UI server

Signal.app's window, and `signal ui`, talk to the running project through a small HTTP server and a WebSocket (ADR 0023).

- **Loopback only.** Both servers bind `127.0.0.1`. Signal.app never binds anything else. `signal ui --remote` binds every interface, for a tablet or another Mac. It is off by default and prints a warning, because remote access then rests on the token alone, sent in clear text without TLS.
- **A token per session.** Each run of the server makes a new 32-byte random token and compares it in constant time. Every WebSocket connection must present it in its first message, within 5 seconds. The token travels in the URL's fragment, which browsers never send in an HTTP request. Anyone with the full URL can control the run and write the project, so treat that URL as a password. `signal ui` prints it only on its own terminal.
- **Origin and Host checks.** A browser page from another site is refused at the WebSocket handshake, before the token is checked. Without `--remote`, the HTTP server answers only the loopback names, which defeats DNS rebinding.
- **Bounded clients.** At most 32 connections, and a client message is at most 64 KiB.

## Files

- **Writes stay inside the project.** Clients change the project only through the protocol's write paths: parameter commits, patches, schematic layout, panels, patterns, whole project and composite files, and vendored library composites. File edits (patches, panels, patterns, whole files) check their base version; every write validates before writing, writes atomically, and never writes through a symbolic link or outside the project folder.
- **What `signal ui` serves.** It serves only the UI library and the project's own `ui/` folder, and it refuses to open any other project. Signal.app opens the folders you choose.
- **Signal's own files.** Besides the project, Signal writes only its machine Setup and unpacked `.signal` documents (both under `~/Library/Application Support/Signal/`), and temporary copies of `.signal` files that `signal play` and `signal ui` unpack into the system's temporary folder. Renders, sessions and replays go to the output folder you name; for `signal mcp` that is a temporary folder by default, never the project.
- **`.signal` files.** Unpacking checks every entry before anything is written: relative paths only, no `..` and no control characters. A file that fails the check writes nothing.

## `signal mcp`: the agent interface

`signal mcp` lets an LLM client (Claude Code, Claude Desktop and others) do what a person at the panel can do (ADR 0029). It speaks the Model Context Protocol over stdio and opens no network listener. `--attach` connects it to a running `signal ui` over loopback, with that server's token.

- **No sound unless `--audio`.** By default the project runs on the null driver, and nothing reaches a speaker.
- **An output ceiling.** Every sample `signal mcp` hands to the audio device is held within ±`--max-gain` (default 0.25, range 0–4). This is enforced in the real-time path, whatever the project or a patch does.
- **A gain guard.** A tool may not raise any output gain above `--max-gain`; such a change is refused with an explanation before anything is sent. When the project files do not load, every tool that changes something is refused.
- **`--read-only`** offers only the inspect, observe and render tools.

The guards cover what `signal mcp`'s own tools do. If an agent also edits the project files with tools of its own, Signal treats those edits like a person's. Without `--attach`, the output ceiling still holds.

## Distribution

Release builds are signed ad hoc with the hardened runtime and are not notarized (ADR 0031). Check downloads against the release's `SHA256SUMS`. The app asks for microphone access only for its input meters.

## Reporting a vulnerability

Please report security problems privately through GitHub's **private vulnerability reporting**: open the repository's **Security** tab, then choose **Report a vulnerability**. Do not open a public issue. Include the version (`signal --version`), what you did, what happened, and, if you can, a project folder that shows the problem. You will get an answer in the advisory thread. Fixes are released as a new version and noted in [`CHANGELOG.md`](CHANGELOG.md).
