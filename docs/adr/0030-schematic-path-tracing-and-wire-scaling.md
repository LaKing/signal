# ADR 0030: Path tracing across composite boundaries, continuous in-place zoom, and wires that scale

- **Status:** Accepted (owner's report of 2026-10-19: "synth moog_voice has about 20 inputs. Inside the structure it is fine, but the outer shell is a wire diagram dead end. We can not trace neither visually, nor in the side panel where the signal goes and comes from. I would suggest a 'scaling' view of the wires, and a fully working path tracing"; then "I like this zooming-in, just let there be a down-scaling wiring — but really I leave it to you, it's fine if it's a bigger task"; design by the operator, delegated). Refines ADR 0026 §2 (the schematic); supersedes nothing.
- **Date:** 2026-10-19

## Context
ADR 0026 promised semantic zoom: "a composite … zoomed in past a threshold, or double-clicked, opens in place and shows its inside". In practice the schematic shows one level at a time: at the top level a composite such as `lib.ladder_voice` or `moog_voice` is a box with about twenty exported ports, and the wires end at its pins; inside, the exported ports appear as boundary pins whose outside is not shown. A signal's path is therefore cut at every composite boundary, both on the canvas and in the sidebar (which lists only the wires at the shown level). With many ports on one box, wires also run together into an unreadable fan.

## Decision
1. **Path tracing, across levels (first).**
   - A *trace* starts from a selected pin, wire or instance and follows connections **upstream, downstream or both** through composite boundaries (exported ports map to the inner ports they forward, both ways), through fan-in and fan-out, and through inserted converters, to the real endpoints: primitive ports, sinks, sources and unconnected ends. It is computed from the `graph` message's structure (as written, every level), so it shows exactly what the project text says; a pure, tested function in `ui/lib/`.
   - **On the canvas:** every wire and pin of the trace at the shown level is highlighted, the rest dimmed; a boundary pin of a closed composite gets a badge summarising what lies behind it ("→ 3 inside: osc1.freq, osc2.freq, osc3.freq"; "← from /melody.hz"), and the composite's box shows which of its ports take part.
   - **In the sidebar:** the trace as a tree by level (top level → composite → inner instance → port), each node clickable to reveal it (zoom and centre, opening composites as needed), with counts and the direction toggle. Escape ends the trace.
   - The trace survives zooming into and out of composites, so the person can walk the path level by level.
2. **Continuous in-place zoom (second).** A composite opens **where it is**: as the view zooms past its threshold (or on double-click), its box grows into a frame that draws its inside at that position, the surrounding level stays visible and dimmed around it, and the outer wires continue across the frame's edge to the inner pins they reach, so a path is one continuous drawing. Zooming out closes it again. Breadcrumbs and the sidebar follow the innermost open level. Layout positions stay per level in `ui/schematic.toml` (ADR 0026), unchanged in format; editing (patching, moving) still happens at one level at a time, the innermost open one.
3. **Wires that scale (with 2).**
   - **Bundles:** at small scale, the wires between the same two boxes (or into one composite) are drawn as one bundle with a count; zooming in fans the bundle out into its wires, smoothly, with a threshold in screen pixels per wire. A traced wire is always drawn on its own, above its bundle.
   - **Port groups on dense boxes:** a box with many ports orders and groups them (by the composite's own export order and name prefixes, e.g. `osc1_*`, `env_*`, or by an optional `group` on an export, a later text addition), and at small scale collapses a group to one labelled pin with a count.
   - Hit testing, hover and selection work on bundles (hover lists the wires; click selects the bundle, a second click a wire).
4. **Protocol:** the `graph` message already carries every level's structure; if the trace needs anything more (e.g. converter detail per connection, export-to-inner mapping made explicit), it is added there, additively, documented in `ui-protocol.md`.
5. **Performance:** the studio (≈180 instances, 20-port boxes) and `library-demo` stay fluid (≥ 50 fps pan/zoom on the Mac); traces of a whole project compute in < 10 ms.

## Alternatives considered
- **A flattened view only:** shows every connection but loses the structure the person wrote; tracing over the structure keeps both.
- **Only tracing, no in-place zoom:** solves the dead end but keeps the path in pieces; the owner likes zooming in, and ADR 0026 already promised in-place opening.

## Consequences
- New pure modules in `ui/lib/` (trace, bundle, port groups) with `node:test` tests; changes to `<signal-schematic>`, its sidebar and its editor; possibly additive fields in `graph`.
- Visual checks in headless Chrome by the operator; the owner judges readability.
