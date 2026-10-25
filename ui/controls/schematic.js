// <signal-schematic>: the project's signal path as an infinite-zoom SVG
// schematic with clickable parts (M2.9-1, ADR 0026 §2–3,
// docs/ui-controls.md "Schematic"). It draws the `graph` message: the top
// level's instances and connections, laid out (ui/lib/schematic-layout.js)
// with the positions stored in ui/schematic.toml; a composite opens in place
// when clicked or double-clicked and closes by the × in its frame's header
// or Escape, never by the zoom (2026-10-22; ui/lib/schematic-scene.js,
// ui/lib/schematic-open.js). One view
// transform, wheel and pinch zoom about the pointer, the background drags to
// pan, keys + − 0 and the arrows. Clicking selects an instance, a port or a
// wire and shows it in the sidebar; dragging a box moves it, its wires
// following (`schematic.move`).
// M2.9-3: the bar's Edit patch switch edits the patch
// (ui/controls/schematic-editor.js); off, nothing of it takes an input.
// The selected instance's signal inputs, outputs and parameters on screen carry a meter, scaled to each port's range, from scope and parameters streams
// (ui/lib/schematic-levels.js). Nothing is drawn or streamed while hidden or
// switched off, and nothing runs between events: no animation loop.
// M2.9-1b: each drawn level is one group, kept with its elements; a pan or a
// zoom changes only the groups' matrices, and elements are built again, by
// key, only when what is visible changes (ui/lib/schematic-draw.js). A
// project that stops loading (`graph.error`) leaves the last drawing on
// screen, marked stale, under its diagnostics.
// 2026-10-17 (the owner's "a selection is not visible", "every node and
// every cable should have its own context menu"): the selection is marked
// unmistakably without moving anything (a selected wire drawn again, thick,
// over a halo, above its level's other wires), a wire under the pointer is
// drawn thicker, and an instance, a wire, a pin and the background have a
// context menu (ui/lib/schematic-menu.js): Signal.app's, which shows these
// items first, or in a browser this element's own.
// 2026-10-17 (the owner's "a properties window appears in the top right
// corner which sometimes covers things. Better would be a fixed sidebar on
// the left", "it should be possible to select … grouped groups too"): the
// card is a fixed sidebar left of the drawing area (ui/lib/schematic-sidebar.js),
// collapsible, and several parts can be selected at once
// (ui/lib/schematic-select.js): Shift- or ⌘-click, a Shift-drag on the
// background, ⌘A; a group's boxes drag together and Delete removes them all.
// ADR 0030 §1 (the owner's "we can not trace neither visually, nor in the
// side panel where the signal goes and comes from"): Trace signal (the
// context menu, T on the selection) follows a pin's, a wire's or a box's
// connections across composite boundaries (ui/lib/schematic-trace.js); the
// trace is highlighted at every level drawn and the rest dimmed, boundary
// pins' labels say what lies behind them, and the sidebar's Trace section
// lists it by level, each node a link that reveals it. It survives
// entering and leaving composites and a `graph` push; Escape ends it; it
// goes to the next module or through modules (M2.13-2).
// M2.13-2 (ADR 0030 §1, §3; the owner's "on the outside of the composite
// you can't see which of the 20 inputs is what"): composites' and frames'
// pins are labelled (at first at a fixed size; see 2026-10-20 below), grouped where they would collide
// (ui/lib/schematic-labels.js), a frame's pins with where they connect
// outside and a box's, hovered or traced, where they go inside; a pair's
// wires are one bundle at small scale, fanning out as the view zooms in
// (ui/lib/schematic-bundle.js).
// 2026-10-20 (the owner's "everything should zoom nicely", "on the outside
// and on the inside of the composite, the connecting interface should be
// named", "tracing must be able to go through the composite"): every text
// is drawn in its level's units and scales with the view (labels, summaries,
// counts in each level's group; detail by size, too small hidden); an open
// composite's box pins are joined to its frame pins and named outside its
// edge; a boundary pin's sidebar lists its other side, Go inside / outside.
// 2026-10-22 (the owner's "when I zoom in, a composite opens automatically.
// It would be better if it opened on a click, and there were an × in the
// top-right corner to close it"): a click on a closed composite's box opens
// it where it is, the box's point under the pointer kept in its frame; the
// frame's × (and Escape) closes it and what is open inside it, the view
// kept still; zooming and panning open and close nothing.

import { onDefaultConnection } from '../signal-ui.js';
import { ENTRY_CSS, ValueEntry } from './entry-field.js';
import { EDITOR_CSS, SchematicEditor } from './schematic-editor.js';
import { ContextMenu, MENU_CSS, menusClaimed } from './context-menu.js';
import { paletteLevel, removeConnection } from '../lib/schematic-patch.js';
import { disconnectOps, schematicMenu } from '../lib/schematic-menu.js';
import { LEGEND, addressesText, sidebar, tooLargeText, traceSection } from '../lib/schematic-sidebar.js';
import { badgeText, behind, parentPath, resolver, splitAddress, startOf, trace, wireKey } from '../lib/schematic-trace.js';
import { moduleInfo, projectKey } from '../lib/schematic-palette.js';
import { allOf, deletePlan, dragGroup, has, hitKey, inside, moveMessages, reselectAll, toggled, traceTarget, union } from '../lib/schematic-select.js';
import { formatFixed } from '../lib/format.js';
import { HEADER, dragPosition, layout } from '../lib/schematic-layout.js';
import { DragFollow } from '../lib/schematic-drag.js';
import { aroundReason, closeBox, closeShown, closeText, crumbView, drawOrder, editable, editLevel, enterView, pinOne, unpin } from '../lib/schematic-open.js';
import { boundaryLinks, breadcrumbs, chain, levelRect, nodeRect, openAnchor, rebase, reroot, scene } from '../lib/schematic-scene.js';
import { bake, bakeHolds, bakedMatrix, bakedPath, bakedPin, bakedPoint, bakedRect, inBounds, reconcile, sameItems } from '../lib/schematic-draw.js';
import { hitTest, instanceHit, portHit, reselect } from '../lib/schematic-hit.js';
import { cardStream, converterText, dragMessage, releaseMessage, resetMessage, rowValues, staleNotice } from '../lib/schematic-inspector.js';
import { LEVEL_RATE, SLOTS, candidates, levelPlan, levelSpan, mutedOf, parameterSpan } from '../lib/schematic-levels.js';
import { isDiscrete, selectOptions, selectValue, valueLabel } from '../lib/value-set.js';
import { IDENTITY, commandView, fit, focus, matrix, pan, toLocal, toScreen, wheelFactor, zoomAt } from '../lib/schematic-view.js';
import { keyCommand, keymap, shownKeys, wheelCommand } from '../lib/keys.js';
import { registerWheel, takesWheel, verticalDelta } from './keyboard.js';
import { COUNT_UNITS, LABEL_PX, PITCH_PX, READ_PX, frameSides, insideNote, levelCounts, levelLabels, outsideNote, pinText, setMeasure } from '../lib/schematic-labels.js';
import { bundleClick, bundleText, bundleWires, countPlaces, countsShown, fanOf, nearestWire } from '../lib/schematic-bundle.js';
import { collapsedAt, exportFacts, groupAnchors, labelDetail, notesAt, portGroups } from '../lib/schematic-groups.js';

const SVG = 'http://www.w3.org/2000/svg';
/** A press that moves less than this (px) is a click. */
const DRAG_PX = 3;
/** A control-click is a right click on a Mac only. */
const MAC = /Mac/i.test(globalThis.navigator?.userAgentData?.platform ?? globalThis.navigator?.platform ?? '');
/** Text smaller than this on screen (px) is not drawn (READ_PX: every text of the drawing scales with it, 2026-10-20). */
const MIN_TEXT_PX = READ_PX;
/** An open composite's box's pin names are hidden above this text size on screen (px): zoomed into it, its frame inside names the same ports. */
const OUTWARD_MAX_PX = 40;
/** After the view settles (ms), the level streams follow it. */
const SETTLE_MS = 150;
/** Above this many parts selected, each is marked by its stroke alone. */
const MANY_MARKS = 64;
/** Where the sidebar's collapse is kept, per viewer (localStorage). */
const SIDE_KEY = 'signal.schematic.sidebar';
/** An open composite's own pins are at most this radius on screen (px). */
const OPEN_PIN_PX = 4;
/** A view's margin (px) on its other edges. */
const MARGIN_PX = 24;
/** A composite's inside fitted to the view keeps this far (px) from the left and right edges: room for its frame's labels, outside its pins. */
const FRAME_SIDE_PX = 120;
/** A second click within this long (ms) and DOUBLE_PX of a click that opened or closed a composite is the double-click's, not one of its own. */
const DOUBLE_MS = 500;
const DOUBLE_PX = 6;

const CSS = `
:host { display: block; position: relative; overflow: hidden; height: 100%; outline: none; --side-width: 300px;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-bg, #16181d);
  font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); }
:host([side-collapsed]) { --side-width: 28px; }
/* Everything right of the sidebar: the schematic's toolbar, then the drawing area below it (2026-10-22: nothing lies over the drawing). */
.main { position: absolute; top: 0; right: 0; bottom: 0; left: var(--side-width); display: flex; flex-direction: column; }
/* The drawing area. Its size changes only with the element's or the sidebar's collapse. */
.stage { position: relative; flex: 1; min-height: 0; overflow: hidden; }
svg { position: absolute; inset: 0; width: 100%; height: 100%; display: block; touch-action: none; user-select: none; -webkit-user-select: none; }
.box { fill: var(--signal-panel, #1f232b); stroke: var(--signal-knob-edge, #3a414d); vector-effect: non-scaling-stroke; }
.composite > .box { stroke: var(--signal-muted, #8a93a3); stroke-width: 2; }
.composite > .frame { fill: none; stroke: var(--signal-muted, #8a93a3); stroke-dasharray: 4 3; vector-effect: non-scaling-stroke; }
.open > .box { fill: var(--signal-bg, #16181d); }
.close-button { cursor: pointer; }
.close-button > rect { fill: transparent; }
.close-button > text { fill: var(--signal-muted, #8a93a3); }
.close-button:hover > text { fill: var(--signal-fg, #d6dbe4); }
svg.closing { cursor: pointer; }
.pins > .box { fill: none; stroke: var(--signal-track, #2a2f38); stroke-dasharray: 2 4; }
/* The target of an open context menu. */
.hover > .box { stroke: var(--signal-fg, #d6dbe4); stroke-width: 2; }
/* The selection (2026-10-17): strokes and a glow only, nothing moves. A box: a thick focus-coloured border with a halo. */
.selected > .box { stroke: var(--signal-focus, #5aa9ff); stroke-width: 3; filter: drop-shadow(0 0 4px var(--signal-focus, #5aa9ff)); }
.name { fill: var(--signal-fg, #d6dbe4); font-weight: 600; }
.module, .pin-label { fill: var(--signal-muted, #8a93a3); }
.pin { fill: var(--signal-track, #2a2f38); stroke: var(--signal-muted, #8a93a3); vector-effect: non-scaling-stroke; }
/* A pin is coloured as its wires (its shape); filled when a wire reaches it, hollow when nothing does. A parameter left to its value keeps the muted ring. */
.pin.signal { stroke: var(--signal-accent, #4fc3f7); }
.pin.vector { stroke: var(--signal-effective, #ffb74d); }
.pin.events { stroke: var(--signal-meter-rms, #3ecf8e); }
.pin.parameter:not(.connected) { stroke: var(--signal-muted, #8a93a3); }
.pin.signal.connected { fill: var(--signal-accent, #4fc3f7); }
.pin.vector.connected { fill: var(--signal-effective, #ffb74d); }
.pin.events.connected { fill: var(--signal-meter-rms, #3ecf8e); }
.node > .pin.hover { stroke: var(--signal-fg, #d6dbe4); stroke-width: 2.5; }
/* A selected pin: its stroke in the focus colour, and a ring twice its size around it, with a halo. */
.node > .pin.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 2; }
.ring { fill: none; stroke: var(--signal-focus, #5aa9ff); stroke-width: 2.5; vector-effect: non-scaling-stroke; pointer-events: none; filter: drop-shadow(0 0 3px var(--signal-focus, #5aa9ff)); }
.wire { fill: none; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.wire.signal { stroke: var(--signal-accent, #4fc3f7); }
.wire.vector { stroke: var(--signal-effective, #ffb74d); }
.wire.events { stroke: var(--signal-meter-rms, #3ecf8e); }
.wire.dashed { stroke-dasharray: 6 4; }
/* Under the pointer (the hit test's, not :hover, which a thin path hardly takes), or a menu's target. */
.wire.hover { stroke-width: 2.5; }
/* A selected wire, drawn again in its level's overlay above the other wires: thick, over a soft focus-coloured halo. */
.halo { fill: none; stroke: var(--signal-focus, #5aa9ff); stroke-width: 11; stroke-opacity: 0.4; stroke-linecap: round; stroke-linejoin: round; vector-effect: non-scaling-stroke; }
.wire.raised { stroke-width: 4; }
.overlay { pointer-events: none; }
/* Many selected (more than MANY_MARKS): strokes only, no glow, halo or ring, which would cost a filter or two paths each. */
svg.many .selected > .box { filter: none; }
/* An open composite selected: its outline only; its glow would grow with the zoom over its inside. */
.selected.open > .box { filter: none; }
svg.many .wire.selected { stroke-width: 4; }
.level { fill: var(--signal-track, #2a2f38); }
.level-fill { fill: var(--signal-meter-rms, #3ecf8e); }
.level-mark { fill: var(--signal-muted, #8a93a3); visibility: hidden; }
/* 2026-10-25: an instance the fault guard muted (app.muted), or a composite holding one: a dashed red edge and the word. */
.muted > .box { stroke: var(--signal-error, #e5534b); stroke-width: 2; stroke-dasharray: 4 3; }
.muted-mark { fill: var(--signal-error, #e5534b); font-weight: 600; }
[data-state="no-slot"] > .level-mark { visibility: visible; }
[data-state="no-slot"] > .level-fill, [data-state="idle"] > .level-fill, [data-state="pending"] > .level-fill { visibility: hidden; }
/* The toolbar (the owner's, 2026-10-22: "these buttons should sit on a toolbar that belongs to the schematic view"): Edit patch and the breadcrumbs left, Fit and the resets right. */
.bar { flex: none; display: flex; gap: 6px; align-items: center; height: 32px; box-sizing: border-box; padding: 0 8px;
  background: var(--signal-panel, #1f232b); border-bottom: 1px solid var(--signal-knob-edge, #3a414d); }
.bar > button { flex: none; }
/* The breadcrumbs, cut in the middle when they do not fit (\`fitCrumbs\`), never wrapping. */
.crumbs { display: flex; gap: 2px; flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; align-items: center; }
.crumbs > button { flex: none; }
.crumbs > button:last-child { flex: 0 1 auto; min-width: 2em; overflow: hidden; text-overflow: ellipsis; }
.crumbs > .cut { flex: none; padding: 0 2px; color: var(--signal-muted, #8a93a3); }
button { font: inherit; color: inherit; background: var(--signal-knob, #262b34); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 4px; padding: 2px 8px; }
.message { position: absolute; left: 8px; bottom: 8px; color: var(--signal-muted, #8a93a3); pointer-events: none; }
svg.stale { opacity: 0.4; filter: grayscale(1); }
.diagnostics { position: absolute; left: 8px; right: 8px; bottom: 28px; max-height: 40%; overflow: hidden; visibility: hidden; padding: 6px 8px;
  white-space: pre-wrap; background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-effective, #ffb74d); border-radius: var(--signal-radius, 6px); }
.diagnostics h3 { margin: 0 0 4px; font-size: 12px; color: var(--signal-effective, #ffb74d); }
.tip { position: absolute; left: 0; top: 0; pointer-events: none; visibility: hidden; white-space: pre; padding: 2px 6px;
  background: var(--signal-knob, #262b34); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 4px; }
/* The sidebar (2026-10-17): a fixed column at the left, never over the drawing; its content scrolls inside it. */
.side { position: absolute; left: 0; top: 0; bottom: 0; width: var(--side-width); box-sizing: border-box; display: flex; flex-direction: column;
  background: var(--signal-panel, #1f232b); border-right: 1px solid var(--signal-knob-edge, #3a414d); }
.side-head { flex: none; display: flex; align-items: center; gap: 6px; height: 32px; box-sizing: border-box; padding: 0 4px; border-bottom: 1px solid var(--signal-knob-edge, #3a414d); }
.side-head h2 { flex: 1; min-width: 0; margin: 0; font-size: 11px; font-weight: 600; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.side-head .collapse { flex: none; width: 20px; height: 22px; padding: 0; }
:host([side-collapsed]) .side-head h2, :host([side-collapsed]) .side-body { display: none; }
:host([side-collapsed]) .side-head { border-bottom: none; }
.side-body { flex: 1; min-height: 0; overflow-y: auto; overflow-x: hidden; scrollbar-gutter: stable; padding: 8px; box-sizing: border-box; }
.side h3 { margin: 0; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.side h4 { margin: 10px 0 4px; font-size: 11px; color: var(--signal-muted, #8a93a3); font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; }
.side .subtitle { color: var(--signal-muted, #8a93a3); margin-bottom: 6px; }
.side .about { margin: 0 0 6px; padding: 4px 6px; line-height: 1.35; overflow-wrap: anywhere; border-left: 2px solid var(--signal-accent, #4fc3f7); background: var(--signal-track, #2a2f38); border-radius: 0 3px 3px 0; }
.side dl { display: grid; grid-template-columns: 6.5rem 1fr; gap: 2px 8px; margin: 0; }
.side dt { color: var(--signal-muted, #8a93a3); }
.side dd { margin: 0; overflow-wrap: anywhere; }
.side .actions { display: flex; flex-wrap: wrap; gap: 4px; margin: 6px 0; }
.side .actions button:disabled { opacity: 0.45; }
.side .note { color: var(--signal-effective, #ffb74d); margin-top: 4px; }
.side button.link { border: none; background: none; padding: 0; color: var(--signal-accent, #4fc3f7); text-align: left; cursor: pointer; overflow-wrap: anywhere; }
.side button.link:hover { text-decoration: underline; }
.side button.link:disabled { color: var(--signal-muted, #8a93a3); cursor: default; text-decoration: none; }
.side ul { list-style: none; margin: 0; padding: 0; }
.side li { margin: 2px 0; }
.side .pinrow { display: grid; grid-template-columns: 10px 1fr auto; gap: 4px; align-items: baseline; margin-top: 4px; }
.side .pinrow .what { color: var(--signal-muted, #8a93a3); white-space: nowrap; }
.side .dot { width: 8px; height: 8px; border-radius: 50%; box-sizing: border-box; border: 1.5px solid var(--signal-muted, #8a93a3); align-self: center; }
.side .dot.signal { border-color: var(--signal-accent, #4fc3f7); }
.side .dot.vector { border-color: var(--signal-effective, #ffb74d); }
.side .dot.events { border-color: var(--signal-meter-rms, #3ecf8e); }
.side .dot.signal.connected { background: var(--signal-accent, #4fc3f7); }
.side .dot.vector.connected { background: var(--signal-effective, #ffb74d); }
.side .dot.events.connected { background: var(--signal-meter-rms, #3ecf8e); }
.side .wires { margin: 0 0 0 14px; }
.side .wires li { display: grid; grid-template-columns: 1fr auto; gap: 4px; align-items: baseline; }
.side .remove { padding: 0 5px; }
.side .remove:disabled { opacity: 0.45; cursor: default; }
.side .legend { display: grid; grid-template-columns: 28px 1fr; gap: 4px 8px; align-items: center; }
.side .legend svg { position: static; width: 28px; height: 10px; }
.side .meter-row { display: grid; grid-template-columns: 1fr 9ch; gap: 6px; align-items: center; margin-top: 4px; }
.side .meter-track { height: 4px; background: var(--signal-track, #2a2f38); position: relative; overflow: hidden; }
.side .meter-fill { position: absolute; left: 0; top: 0; bottom: 0; width: 100%; transform-origin: left; transform: scaleX(0); background: var(--signal-meter-rms, #3ecf8e); }
.side .meter-state { color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
/* The marquee: a Shift-drag on the background. */
.marquee { position: absolute; left: 0; top: 0; width: 0; height: 0; visibility: hidden; pointer-events: none; box-sizing: border-box;
  border: 1px dashed var(--signal-focus, #5aa9ff); background: rgba(90, 169, 255, 0.12); }
/* A trace (ADR 0030 §1): its wires and pins stand out, the rest is dimmed; boundary pins carry badges. */
svg.tracing .wire:not(.traced) { opacity: 0.12; }
svg.tracing .wire.traced { stroke-width: 3; }
svg.tracing .node:not(.traced) > :not(.frame) { opacity: 0.35; }
svg.tracing .node.traced > .box { stroke: var(--signal-fg, #d6dbe4); }
svg.tracing .pin:not(.traced) { opacity: 0.4; }
svg.tracing .pin.traced { stroke: var(--signal-fg, #d6dbe4); stroke-width: 2.5; }
/* Around a composite open in place (M2.13-3): its level dimmed, all but its frame, the wires crossing into it, its edge links and what is traced. */
.surround .node:not(.open):not(.traced) { opacity: 0.4; }
.surround .wire:not(.crossing):not(.link):not(.traced) { opacity: 0.3; }
/* A composite's or a frame's port label, a summary, a bundle's count (M2.13-2; 2026-10-20: in the level's units, scaling with the drawing, ui/lib/schematic-labels.js). */
.labels { pointer-events: none; }
.lbl { fill: var(--signal-muted, #8a93a3); white-space: pre; paint-order: stroke; stroke: var(--signal-bg, #16181d); stroke-linejoin: round; }
.lbl.count { stroke: none; }
.lbl.centre, .lbl.noted, .lbl.traced { fill: var(--signal-fg, #d6dbe4); }
.lbl.count { fill: var(--signal-bg, #16181d); font-weight: 600; }
.lbg { fill: var(--signal-panel, #1f232b); stroke: none; }
.lbg.traced { stroke: var(--signal-focus, #5aa9ff); stroke-width: 1; vector-effect: non-scaling-stroke; }
.lbg.count { fill: var(--signal-muted, #8a93a3); }
/* An open composite's edge crossed (2026-10-20): its box pin to its frame pin, so a path is one drawing. */
.wire.link { stroke-width: 1; opacity: 0.3; pointer-events: none; }
/* Traced (or under a trace): the path crossing the edge stands out. */
.wire.link.traced, svg.tracing .wire.link.traced { opacity: 1; stroke-width: 3; }
.side .trace { margin: 0 0 10px; padding: 6px; border: 1px solid var(--signal-focus, #5aa9ff); border-radius: var(--signal-radius, 6px); }
.side .trace-head { display: flex; align-items: center; gap: 4px; }
.side .trace-head h3 { flex: 1; min-width: 0; }
.side .trace .toggle { display: flex; gap: 2px; margin: 6px 0 2px; }
.side .trace .toggle button[aria-pressed="true"] { background: var(--signal-focus, #5aa9ff); color: var(--signal-bg, #16181d); border-color: var(--signal-focus, #5aa9ff); }
.side .trace .end { display: grid; grid-template-columns: 1fr; margin: 3px 0 3px 4px; }
.side .trace .end .what { color: var(--signal-muted, #8a93a3); overflow-wrap: anywhere; }
.side .trace .tree { margin-left: 10px; }
.side .trace .tree .count { color: var(--signal-muted, #8a93a3); margin-left: 4px; }
.side .trace .tree .role-start { font-weight: 600; }
.side .trace .tree .role-endpoint::after { content: " ●"; color: var(--signal-focus, #5aa9ff); }
/* An open end (M2.13-2): leads nowhere; marked apart from the real ends. */
.side .trace .tree .role-open::after { content: " ○ open end"; color: var(--signal-muted, #8a93a3); font-weight: normal; }
.side .trace .end.open { border-left: 2px dashed var(--signal-muted, #8a93a3); padding-left: 4px; }
.side .trace .end.open .what { font-style: italic; }
.side .trace summary .count { color: var(--signal-muted, #8a93a3); margin-left: 4px; }
.side .trace details > summary { cursor: pointer; }
.row { display: grid; grid-template-columns: 1fr auto auto 1ch; gap: 2px 6px; align-items: center; margin-top: 4px; }
.row .readout { position: relative; font-variant-numeric: tabular-nums; white-space: pre; color: var(--signal-muted, #8a93a3); }
.row input { grid-column: 1 / 4; width: 100%; margin: 0; }
.row .mark { visibility: hidden; color: var(--signal-effective, #ffb74d); }
.row .mark.on { visibility: visible; }
${ENTRY_CSS}
${EDITOR_CSS}
${MENU_CSS}`;

function el(name, attributes = {}, text = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  if (text !== null) node.textContent = text;
  return node;
}

function html(name, properties = {}) {
  return Object.assign(document.createElement(name), properties);
}

/** `hit` as the part it stands for: an open frame's × (2026-10-22) as its composite's box, for a menu; any other as it is. */
function partHit(drawn, hit) {
  if (hit.kind !== 'close') return hit;
  const entry = drawn.levels.find((l) => l.path === hit.path);
  return entry ? instanceHit(entry, hit.node) : hit;
}

/** Where the box a gesture drags goes (`dragPosition`). */
function dragTo(gesture) {
  return dragPosition(gesture.hit.node, { x: gesture.last.x - gesture.start.x, y: gesture.last.y - gesture.start.y }, gesture.k);
}

export class SignalSchematic extends HTMLElement {
  constructor() {
    super();
    this.view = IDENTITY;
    this.viewport = { w: 0, h: 0 };
    this.graph = null;
    /** What each module type is (`moduleInfo` of the last `modules` reply), and of which project; a request in flight. */
    this.modulesKnown = { info: null, project: null, asking: null };
    this.layouts = new Map();
    /** Positions moved here and not yet in a pushed `graph`: level → instance → {x, y}. */
    this.moved = new Map();
    this.open = new Set();
    this.pinned = new Set();
    /** How far opening each composite by a click panned the view (its parent's units, by path): undone when it closes. */
    this.openShifts = new Map();
    this.drawn = null;
    /** Each drawn level's group and its elements, by path. */
    this.groups = new Map();
    /** The paths of the groups in the SVG, in order. */
    this.order = [];
    this.crumbKey = null;
    /** The elements marked `selected` (a Set) and `hover`. */
    this.marked = { selected: new Set(), hover: null };
    /** The wire under the pointer (a hit), and the target of an open context menu. */
    this.hovered = null;
    this.menuState = null;
    /** The keyboard's menu anchor while its right click is dispatched. */
    this.keyMenu = null;
    /** Each selected wire's copy and halo, in its level's overlay; each selected pin's ring (by the element marked). */
    this.halos = new Map();
    this.rings = new Map();
    /** The host's last word on its scope slots (`scopes`, else `describe`). */
    this.scopes = null;
    this.scopesPushed = false;
    this.fitted = false;
    /** The selection: hits, each part once, the last the one added last (ui/lib/schematic-select.js). */
    this.selected = [];
    this.gesture = null;
    this.pending = false;
    this.barsPending = false;
    this.stops = [];
    this.levelSubs = new Map();
    /** Each metered plan port's last span, `{lo, hi, text}`. */
    this.levels = new Map();
    /** Each metered scope port's range (null: the dBFS scale). */
    this.levelRanges = new Map();
    /** The parameters stream of the selected instance's parameters: `{key, stop, addresses}`, or null. */
    this.paramStream = null;
    /** The instances the fault guard muted in the run (`app.mutedInstances`), as a set of addresses. */
    this.mutedSet = new Set();
    this.mutedKey = '';
    this.refused = new Map();
    this.bars = new Map();
    /** A node's element → its level bars, `[[address, {bar, fill, width}]]`. */
    this.barsOf = new WeakMap();
    this.levelTimer = null;
    this.retryTimer = null;
    this.cardValues = new Map();
    this.cardStop = null;
    this.cardRows = [];
    /** The sidebar's live level of a selected pin: `{address, fill, state}`, or null. */
    this.sideMeter = null;
    /** The trace (ADR 0030 §1): `{start, direction, result, badges, version}`, or null. */
    this.tracing = null;
    this.traceVersion = 0;
    /** A boundary pin's one-hop note, by side and address, for the `graph` shown. */
    this.notes = new Map();
    /** The closed composite under the pointer (its address), whose pins show where they go inside. */
    this.hoveredBox = null;
  }

  /** The part selected last (the context menu's and the keyboard's), or null. */
  get selection() {
    return this.selected.at(-1) ?? null;
  }

  /**
   * The instance whose ports carry meters (2026-10-25: only the selected
   * one): the selected instance, or the instance of a selected pin.
   */
  get metered() {
    const s = this.selection;
    if (s?.kind === 'instance') return s.address;
    if (s?.kind === 'port' && s.node?.kind === 'instance') return `${s.path}/${s.node.name}`;
    return null;
  }

  /** `app.mutedInstances`: the boxes marked muted change only when the list does. */
  mutedSeen(app) {
    const list = Array.isArray(app?.mutedInstances) ? app.mutedInstances : [];
    const key = list.join('\t');
    if (key === this.mutedKey) return;
    this.mutedKey = key;
    this.mutedSet = new Set(list);
  }

  connectedCallback() {
    if (!this.shadowRoot) this.build();
    this.resize = new ResizeObserver(() => this.measure());
    this.resize.observe(this);
    this.resize.observe(this.stage);
    this.onVisibility = () => this.activity();
    document.addEventListener('visibilitychange', this.onVisibility);
    this.stopDefault = onDefaultConnection((connection) => this.attach(connection));
    this.menuCloser = (event) => {
      if (this.menu.isOpen && !event.composedPath().includes(this.menu.list)) this.menu.close();
    };
    this.menuBlur = () => this.menu.close();
    window.addEventListener('pointerdown', this.menuCloser, true);
    window.addEventListener('blur', this.menuBlur);
  }

  disconnectedCallback() {
    this.resize?.disconnect();
    document.removeEventListener('visibilitychange', this.onVisibility);
    this.stopDefault?.();
    window.removeEventListener('pointerdown', this.menuCloser, true);
    window.removeEventListener('blur', this.menuBlur);
    this.menu.close();
    this.detach();
  }

  build() {
    const root = this.attachShadow({ mode: 'open' });
    this.tabIndex = 0;
    const style = html('style', { textContent: CSS });
    this.stage = html('div', { className: 'stage' });
    this.svg = el('svg', { role: 'img', 'aria-label': 'Signal schematic' });
    this.bar = html('div', { className: 'bar' });
    this.crumbs = html('nav', { className: 'crumbs' });
    const fitButton = html('button', { type: 'button', textContent: 'Fit', title: 'Fit (0)' });
    fitButton.addEventListener('click', () => this.fitTop());
    const reset = html('button', { type: 'button', textContent: 'Reset layout', title: 'Forget the moved positions of the level shown' });
    reset.addEventListener('click', () => this.resetLayout(this.currentLevel()));
    const resetAll = html('button', { type: 'button', textContent: 'Reset all', title: 'Forget every moved position (removes ui/schematic.toml)' });
    resetAll.addEventListener('click', () => this.resetLayout(null));
    this.bar.append(this.crumbs, fitButton, reset, resetAll);
    this.message = html('div', { className: 'message' });
    this.staleBox = html('section', { className: 'diagnostics' });
    this.tip = html('div', { className: 'tip' });
    this.marquee = html('div', { className: 'marquee' });
    this.stage.append(this.svg, this.message, this.staleBox, this.tip, this.marquee);
    const main = html('div', { className: 'main' });
    main.append(this.bar, this.stage);
    root.append(style, this.buildSide(), main);
    this.editor = new SchematicEditor(this);
    this.editor.build(this.bar, this.stage);
    this.svg.addEventListener('pointerdown', (e) => this.pointerDown(e));
    this.svg.addEventListener('pointermove', (e) => this.pointerMove(e));
    this.svg.addEventListener('pointerup', (e) => this.pointerUp(e));
    this.svg.addEventListener('pointercancel', () => this.cancelGesture());
    // Capture lost mid-gesture (not the release, which ends it first): let go, as cancelled.
    this.svg.addEventListener('lostpointercapture', (e) => {
      if (this.gesture && e.pointerId === this.gesture.pointer) this.cancelGesture();
    });
    this.svg.addEventListener('pointerleave', () => {
      this.hideTip();
      this.hoverWire(null);
      this.hoverBox(null);
    });
    this.svg.addEventListener('dblclick', (e) => this.doubleClick(e));
    this.svg.addEventListener('wheel', (e) => this.wheel(e), { passive: false });
    registerWheel(this);
    // Safari's pinch.
    this.svg.addEventListener('gesturestart', (e) => this.pinch(e, true));
    this.svg.addEventListener('gesturechange', (e) => this.pinch(e, false));
    this.addEventListener('keydown', (e) => this.key(e));
    // Its own context menu, where no page menu took the right click (in a browser).
    this.menu = new ContextMenu({ label: 'Schematic' });
    root.append(this.menu.list);
    this.addEventListener('contextmenu', (e) => this.ownMenu(e));
  }

  // The connection.

  attach(connection) {
    this.detach();
    this.connection = connection;
    if (!connection) return;
    const on = (event, fn) => {
      const stop = connection.on(event, fn);
      if (typeof stop === 'function') this.stops.push(stop);
    };
    on('graph', (graph) => this.setGraph(graph));
    on('graph.error', (error) => this.stale(error));
    on('scopes', (scopes) => this.scopesSeen(scopes, true));
    on('welcome', () => this.fetch());
    on('describe', () => this.described());
    on('unsaved', () => this.refreshCard());
    on('app', (app) => {
      this.mutedSeen(app);
      this.activity();
    });
    if (connection.status === undefined || connection.status === 'open') this.fetch();
  }

  detach() {
    for (const stop of this.stops.splice(0)) stop();
    this.releaseLevels();
    this.stopCardStream();
    this.connection = null;
  }

  fetch() {
    this.connection?.request?.({ type: 'graph' }).then(
      (graph) => this.setGraph(graph),
      (error) => {
        if (error.code === 'does-not-load') this.stale(error.reply ?? error);
        else this.say(error.code === 'closed' ? '' : `No schematic: ${error.message}`);
      },
    );
  }

  /** `graph.error`: the last drawing stays, marked stale, under the diagnostics. */
  stale(error) {
    // Another project's error: its drawing is not this one's, so it goes.
    if (this.graph && 'folder' in error && error.folder !== this.graph.folder) {
      this.graph = null;
      this.layouts.clear();
      this.selected = [];
      this.tracing = null;
      this.closeMenu();
      this.fitted = false;
      this.svg.replaceChildren();
      this.crumbs.replaceChildren();
      this.showSide();
    }
    const notice = staleNotice(error, Boolean(this.graph));
    const title = html('h3', { textContent: notice.title });
    this.staleBox.replaceChildren(title, document.createTextNode(notice.text));
    this.staleBox.style.visibility = 'visible';
    this.svg.classList.toggle('stale', Boolean(this.graph));
  }

  setGraph(graph) {
    this.staleBox.style.visibility = 'hidden';
    this.svg.classList.remove('stale');
    const old = new Map(this.layouts);
    // Another project: its own view, fitted again, nothing open or selected.
    if (this.graph && graph.project !== this.graph.project) {
      this.moved.clear();
      this.open = new Set();
      this.pinned = new Set();
      this.fitted = false;
      this.selected = [];
      this.view = IDENTITY;
      this.tracing = null;
      old.clear();
    }
    this.graph = graph;
    this.notes = new Map();
    this.noteResolver = resolver(graph);
    this.editor.graphSeen(graph);
    this.layouts.clear();
    // A move the graph holds, or written, goes; one still being written (a
    // group's later message) stays, so no box jumps back meanwhile.
    for (const [level, names] of this.moved) {
      const stored = graph.layout?.positions?.[level] ?? {};
      for (const [name, at] of names) if (!at.pending || (stored[name]?.x === at.x && stored[name]?.y === at.y)) names.delete(name);
      if (!names.size) this.moved.delete(level);
    }
    // What did not move stays where it is on screen.
    if (old.size) this.view = rebase(this.view, (level) => old.get(level) ?? null, (level) => this.layoutOf(level));
    this.say(graph.layout?.error ? `Positions ignored: ${graph.layout.error}` : '');
    // The selection is found again in the new graph; what went is let go.
    this.selected = reselectAll(this.selected, (level) => this.layoutOf(level));
    // So is the trace, computed again on the new structure.
    if (this.tracing) this.retrace();
    // So is an open menu's target; a menu whose target went closes.
    const target = this.menuState?.target;
    if (target && target.hit.kind !== 'background') {
      const again = reselect(target.hit, (level) => this.layoutOf(level));
      if (again) target.hit = again;
      else this.closeMenu();
    }
    this.showSide();
    this.schedule();
  }

  /**
   * The host's free scope slots: a `scopes` push (`pushed`), or `describe`
   * until the first push. Kept with when it came and the slots held then.
   */
  scopesSeen(scopes, pushed) {
    if (!pushed && this.scopesPushed) return;
    if (typeof scopes?.free !== 'number') return;
    this.scopesPushed ||= pushed;
    this.scopes = { free: scopes.free, sinceFreedMs: scopes.sinceFreedMs ?? null, at: performance.now(), held: this.levelSubs.size };
    if (pushed) this.updateLevels();
  }

  described() {
    this.scopesSeen(this.connection?.description?.scopes, false);
    this.activity();
    // The sidebar is built again only when its rows change (a `describe`
    // that arrives after the selection, or another structure); values in place.
    const model = this.sideModel();
    const rows = (m) => (m?.parameters ?? []).map((r) => `${r.address} ${r.editable}`).join('\n');
    if (rows(model) !== rows(this.cardModel)) this.showSide();
    else this.refreshCard();
  }

  say(text) {
    this.message.textContent = text;
  }

  // What is shown.

  /** Streams flow only while shown, switched on and playing. */
  get active() {
    const c = this.connection;
    return Boolean(this.viewport.w > 0 && document.visibilityState !== 'hidden' && c && c.streaming !== false && c.description?.playing);
  }

  activity() {
    this.updateLevels();
    this.updateCardStream();
    if (this.viewport.w > 0) this.schedule();
  }

  /** The drawing area (right of the sidebar): what the view, Fit and every screen coordinate are of. */
  measure() {
    const rect = this.stage.getBoundingClientRect();
    this.viewport = { w: rect.width, h: rect.height };
    if (this.crumbs.children.length) this.fitCrumbs();
    // A hidden tab is 0×0: nothing to draw, nothing to stream.
    this.activity();
  }

  layoutOf(level) {
    if (!this.graph) return null;
    if (!this.layouts.has(level)) {
      const source = level === '' ? this.graph.top : this.graph.composites?.[level];
      if (!source) return null;
      const stored = { ...(this.graph.layout?.positions?.[level] ?? {}), ...Object.fromEntries(this.moved.get(level) ?? []) };
      // M2.13-2: a composite's box lays its pins out in groups (ui/lib/schematic-groups.js).
      this.layouts.set(level, layout(source, stored, { factsOf: (module) => exportFacts(this.graph.composites?.[module] ?? null) }));
    }
    return this.layouts.get(level);
  }

  rootBounds() {
    return this.layoutOf('')?.bounds ?? { x: 0, y: 0, w: 1, h: 1 };
  }

  /** The whole top level in view on the stage (its composites are closed by Fit itself, `fitTop`). */
  fitAll() {
    return fit(this.rootBounds(), this.viewport, MARGIN_PX);
  }

  /** Fit, 0 and the top breadcrumb: every composite closed, the whole top level in view (M2.13-3). */
  fitTop() {
    this.pinned = new Set();
    this.openedAt = null;
    this.setView(this.fitAll());
  }

  /** The view set (a zoom, a pan, Enter, a breadcrumb, Fit): it only moves the view, opening and closing nothing (2026-10-22). */
  setView(view) {
    this.view = view;
    this.schedule();
  }

  /** One redraw at the next frame, only when something changed. */
  schedule() {
    if (this.pending || this.viewport.w === 0) return;
    this.pending = true;
    requestAnimationFrame(() => {
      this.pending = false;
      this.render();
    });
  }

  render() {
    if (!this.graph || this.viewport.w === 0) return;
    if (!this.fitted) {
      this.view = this.fitAll();
      this.fitted = true;
    }
    const layoutOf = (level) => this.layoutOf(level);
    // Only what the person opened is open (`pinned`, 2026-10-22).
    const openedAt = this.openedAt;
    this.openedAt = null;
    this.drawn = scene({ layoutOf, view: this.view, viewport: this.viewport, pinned: this.pinned });
    // A composite a click opened keeps what was under the pointer there, its header on the stage (the operator's review).
    const anchor = openedAt ? openAnchor(this.drawn, this.open, openedAt, this.viewport) : null;
    if (anchor && (anchor.dx || anchor.dy)) {
      // Kept, in its parent's units, so that closing it puts the view back (`closeInPlace`).
      this.openShifts.set(anchor.path, { dx: anchor.dx / anchor.k, dy: anchor.dy / anchor.k });
      this.drawn = scene({ layoutOf, view: pan(this.drawn.view, anchor.dx, anchor.dy), viewport: this.viewport, pinned: this.pinned });
    }
    // Kept in the coordinates of the deepest level under the centre.
    this.view = this.drawn.view;
    this.open = this.drawn.open;
    for (const path of this.openShifts.keys()) if (!this.open.has(path)) this.openShifts.delete(path);
    // A level with nothing on screen is not drawn, nor a parent of which
    // only open boxes' insides show (the background's colour).
    // An open composite's inside is drawn even with nothing of it on screen: the links across its edge lead into it (2026-10-20).
    const shown = this.drawn.levels.filter((level) => (level.nodes.length || level.wires.length || level.parent) && !level.behind);
    let changed = false;
    const paths = [];
    for (const level of shown) {
      const drew = this.drawLevel(level);
      if (drew === null) continue;
      paths.push(level.path);
      changed = drew || changed;
    }
    for (const path of this.groups.keys()) {
      if (!paths.includes(path)) {
        this.groups.delete(path);
        changed = true;
      }
    }
    // Only when levels open or close: a pan or a zoom changes matrices.
    if (!sameItems(paths, this.order)) {
      this.svg.replaceChildren(...paths.map((path) => this.groups.get(path).g));
      this.order = paths;
    }
    if (changed) this.collectBars();
    this.applyMarks();
    this.applyTrace();
    this.applyDrag();
    this.drawCrumbs();
    // A selected composite opened or closed (a click, its ×, the sidebar's
    // own Open in place or Close): its buttons follow.
    if (this.sideStateKey() !== this.sideShownKey) this.showSide();
    this.applyBars();
    clearTimeout(this.levelTimer);
    this.levelTimer = setTimeout(() => this.updateLevels(), SETTLE_MS);
  }

  /**
   * Level `level` (a scene entry) in its group: its matrix always, its
   * elements only when what it shows changed. Returns whether they did, or
   * null for a level its coordinates cannot place on screen (past the
   * camera's precision), which is not drawn: never a matrix out of bounds.
   */
  drawLevel(level) {
    let drawn = this.groups.get(level.path);
    if (!drawn) {
      const wires = el('g');
      const overlay = el('g', { class: 'overlay' });
      const nodes = el('g');
      // An open composite's edge crossed (its box's pins to its frame's), and the level's labels in its own units (2026-10-20).
      const links = el('g');
      const labels = el('g', { class: 'labels' });
      // An open frame's × (2026-10-22), over everything of its level.
      const close = el('g');
      drawn = { g: el('g'), wires, links, overlay, nodes, labels, close, closeKey: null, baked: null, wireCache: new Map(), nodeCache: new Map(), bundleCache: new Map(), shown: null, labelKey: null, sizes: [] };
      drawn.g.append(wires, links, overlay, nodes, labels, close);
      this.groups.set(level.path, drawn);
    }
    const transform = level.transform;
    if (!drawn.baked || drawn.layout !== level.layout || !bakeHolds(drawn.baked, transform, this.viewport)) {
      if (drawn.layout !== level.layout) drawn.bundleCache = new Map();
      drawn.baked = bake(transform, this.viewport);
      drawn.layout = level.layout;
      drawn.wireCache = new Map();
      drawn.nodeCache = new Map();
      drawn.shown = null;
    }
    const placed = bakedMatrix(drawn.baked, transform);
    if (!drawn.baked.representable || !inBounds(placed)) return null;
    const text = (px) => px * transform.k >= MIN_TEXT_PX;
    const open = level.nodes.filter((n) => n.composite && this.drawn.open.has(`${level.path}/${n.name}`)).map((n) => n.name).join('/');
    // An open composite's own pins keep about their size on screen (in half-octave steps of the scale): never huge discs over its frame.
    level.pinK = open ? 2 ** (Math.floor(Math.log2(transform.k) * 2) / 2) : 1;
    level.openKey = open;
    // The box around this level's inside, when it is open in place: its pins are joined to the frame's (its groups collapsed or not).
    const parentCollapsed = Boolean(level.parent?.collapsed);
    const sizes = `${text(12)} ${text(9)}${open ? ` ${level.pinK}` : ''} ${parentCollapsed}`;
    // M2.13-2: a pair's wires bundled at small scale (ui/lib/schematic-bundle.js), the traced and selected ones on their own above.
    const solo = this.soloWires(level);
    // A level a box of which is dragged: every wire on its own, as DragFollow moves them by id.
    const dragging = this.levelDragged(level);
    const fan = dragging ? 1 : fanOf(transform.k);
    // Small, a composite's port groups are one pin each, its wires joining it (not while a box of the level is dragged).
    const collapsed = !dragging && collapsedAt(transform.k);
    level.collapsed = collapsed;
    const soloKey = [...solo].join('\t');
    const shown = drawn.shown;
    const same =
      shown &&
      shown.sizes === sizes &&
      shown.open === open &&
      shown.fan === fan &&
      shown.collapsed === collapsed &&
      shown.solo === soloKey &&
      shown.metered === this.metered &&
      shown.mutedKey === this.mutedKey &&
      sameItems(shown.nodes, level.nodes) &&
      sameItems(shown.wires, level.wires);
    if (same) level.bundles = shown.items;
    else {
      const baked = drawn.baked;
      const ends = collapsed ? this.groupEnds(level.layout) : null;
      const items = bundleWires(level.wires, transform.k, { solo, bottom: level.layout.bottom, cache: drawn.bundleCache, fanned: dragging, ends, avoid: level.layout.avoid ?? null });
      level.bundles = items;
      const wires = reconcile(
        drawn.wireCache,
        items,
        (w) => (w.kind === 'bundle' ? `bundle ${w.key}` : w.id),
        () => '',
        (w) => (w.kind === 'bundle' ? this.bundleElement(level, w, baked) : this.wireElement(level, w, baked)),
      );
      // An open composite's frame last, over the neighbours it grew over (M2.13-3; hit testing takes it there first).
      const nodes = reconcile(
        drawn.nodeCache,
        drawOrder(level.nodes, level.layout.opened),
        (n) => n.id,
        (n) => {
          const address = `${level.path}/${n.name}`;
          return `${this.drawn.open.has(address)} ${sizes} ${collapsed} ${this.metered === address} ${mutedOf(this.mutedSet, address)}`;
        },
        (n) => this.nodeElement(level, n, baked, text, collapsed),
      );
      if (!sameItems([...drawn.wires.children], wires.elements)) drawn.wires.replaceChildren(...wires.elements);
      drawn.links.replaceChildren(...this.linkElements(level, baked));
      if (!sameItems([...drawn.nodes.children], nodes.elements)) drawn.nodes.replaceChildren(...nodes.elements);
      drawn.wireCache = wires.cache;
      drawn.nodeCache = nodes.cache;
      drawn.shown = { sizes, open, fan, collapsed, solo: soloKey, metered: this.metered, mutedKey: this.mutedKey, items, nodes: level.nodes, wires: level.wires };
    }
    drawn.placed = placed;
    drawn.g.setAttribute('transform', matrix(placed));
    this.drawClose(level, drawn);
    // Around an open composite's frame the level is dimmed (M2.13-3): its other boxes and wires, not the frame or the wires crossing into it.
    const surround = Boolean(level.layout.opened);
    if (drawn.surround !== surround) {
      drawn.surround = surround;
      drawn.g.setAttribute('class', surround ? 'surround' : '');
    }
    return !same;
  }

  /**
   * The × of the frame open in `level` (a scene entry), in its group
   * `drawn` (2026-10-22): in the frame's top right, at least CLOSE_MIN_PX
   * on screen (`closeBox`), hidden where it would cover the frame's title
   * or leave the frame (`closeShown`; Escape closes it then). Built again
   * only when its square changes: never on a pan.
   */
  drawClose(level, drawn) {
    const name = level.layout.opened;
    const frame = name ? level.layout.nodes.find((n) => n.kind === 'instance' && n.name === name) : null;
    const k = level.transform.k;
    const r = frame && closeShown(frame, k) ? closeBox(frame, k) : null;
    const key = r ? `${r.x} ${r.y} ${r.w}` : '';
    if (drawn.closeKey === key && drawn.closeBaked === drawn.baked) return;
    drawn.closeKey = key;
    drawn.closeBaked = drawn.baked;
    if (!r) {
      drawn.close.replaceChildren();
      return;
    }
    const baked = drawn.baked;
    const address = `${level.path}/${name}`;
    const button = el('g', { class: 'close-button', 'data-address': address });
    const at = bakedRect(baked, r);
    if (at) button.append(el('rect', at));
    const size = closeText(r);
    const c = bakedPoint(baked, { x: r.x + r.w / 2, y: r.y + r.h / 2 + size * 0.35 });
    button.append(el('text', { x: c.x, y: c.y, 'font-size': size * baked.m, 'text-anchor': 'middle' }, '×'), el('title', {}, `Close ${address}`));
    drawn.close.replaceChildren(button);
  }

  wireElement(level, wire, baked) {
    const from = `${level.path}/${String(wire.written.from).replace(/^\//, '')}`;
    return el('path', {
      class: ['wire', wire.shape, wire.dashed ? 'dashed' : '', wire.crossing ? 'crossing' : ''].filter(Boolean).join(' '),
      d: bakedPath(baked, wire.points, wire.curve),
      'data-address': from,
      'data-label': wire.label,
      'data-part': 'schematic',
    });
  }

  /** A bundle (M2.13-2): one thick path of its pair while folded, else its wires fanning out; its count is with the level's labels (drawLabels). */
  bundleElement(level, bundle, baked) {
    const shape = bundle.wires[0].shape;
    const group = el('g', { class: 'bundle', 'data-label': bundle.wires.map((w) => w.label).join('\n'), 'data-part': 'schematic' });
    if (!bundle.members.length) {
      const path = el('path', { class: `wire ${shape}`, d: bakedPath(baked, bundle.points, bundle.curve) });
      path.style.strokeWidth = `${1.5 + Math.log2(bundle.count)}px`;
      group.append(path);
    }
    for (const m of bundle.members) group.append(el('path', { class: ['wire', m.wire.shape, m.wire.dashed ? 'dashed' : ''].join(' ').trim(), d: bakedPath(baked, m.points, m.curve) }));
    return group;
  }

  /** Whether a box of `level` (a scene entry) is being dragged: its wires each on their own, its groups open. */
  levelDragged(level) {
    return Boolean(this.gesture?.follow && this.gesture.moved && this.gesture.hit.level === level.level);
  }

  /**
   * The ends of `layout`'s wires to and from a collapsed port group (M2.13-2,
   * ui/lib/schematic-groups.js): its pin, and the group's key, for
   * `bundleWires`; null for a wire with neither end in a group of several.
   */
  groupEnds(layout) {
    let ends = this.groupEndsOf?.get(layout);
    if (ends) return ends;
    const anchors = new Map(layout.nodes.filter((n) => n.groups).map((n) => [n.id, groupAnchors(n)]));
    ends = (w) => {
      const a = anchors.get(w.from.node)?.get(w.from.port);
      const b = anchors.get(w.to.node)?.get(w.to.port);
      if (!a && !b) return null;
      return { a: a ? { x: a.x, y: a.y } : w.curve.start, b: b ? { x: b.x, y: b.y } : w.curve.segs.at(-1).to, key: `${a?.group.key ?? ''}|${b?.group.key ?? ''}` };
    };
    this.groupEndsOf ??= new WeakMap();
    this.groupEndsOf.set(layout, ends);
    return ends;
  }

  /**
   * The `wireKey`s of `level`'s wires drawn on their own over their
   * bundles: the selected, the traced, and those crossing into an open
   * composite's frame (M2.13-3: each continues to its own pin, never
   * folded into a bundle at the edge).
   */
  soloWires(level) {
    const path = level.path;
    const solo = new Set(this.tracing?.result?.wires.get(path) ?? []);
    for (const s of this.selected) if (s.kind === 'wire' && s.path === path) solo.add(wireKey(s.wire.written));
    if (level.layout.opened) for (const w of level.layout.wires) if (w.crossing) solo.add(wireKey(w.written));
    return solo;
  }

  nodeElement(level, node, baked, text, collapsed = false) {
    const m = baked.m;
    const address = `${level.path}/${node.name}`;
    const isOpen = this.drawn.open.has(address);
    const muted = node.kind === 'instance' ? mutedOf(this.mutedSet, address) : null;
    const classes = ['node', node.kind === 'instance' ? '' : 'pins', node.composite ? 'composite' : '', isOpen ? 'open' : '', muted ? 'muted' : ''];
    const group = el('g', { class: classes.join(' ').replace(/\s+/g, ' ').trim() });
    if (node.kind === 'instance') {
      group.dataset.address = address;
      group.dataset.label = node.name;
      group.dataset.part = 'schematic';
    }
    const rect = (r, attributes) => {
      const at = bakedRect(baked, r);
      if (at) group.append(el('rect', { ...attributes, ...at }));
    };
    const label = (p, size, attributes, content) => {
      const at = bakedPoint(baked, p);
      group.append(el('text', { ...attributes, x: at.x, y: at.y, 'font-size': size * m }, content));
    };
    rect(node, { class: 'box', rx: 4 * m });
    if (node.composite) rect({ x: node.x + 3, y: node.y + 3, w: node.w - 6, h: node.h - 6 }, { class: 'frame', rx: 3 * m });
    if (text(12) && node.kind === 'instance') {
      label({ x: node.x + 8, y: node.y + 14 }, 12, { class: 'name' }, node.name);
      label({ x: node.x + 8, y: node.y + 27 }, 10, { class: 'module' }, node.module);
    }
    if (muted && text(9)) label({ x: node.x + node.w - 8, y: node.y + 14 }, 9, { class: 'muted-mark', 'text-anchor': 'end' }, muted === 'self' ? 'muted' : 'muted inside');
    const metered = node.kind === 'instance' && !isOpen && this.metered === address;
    const bars = [];
    // An open composite's pins drawn at OPEN_PIN_PX on screen at most (zoomed into its inside, they would grow over its frame).
    const radius = isOpen ? Math.min(3, OPEN_PIN_PX / (level.pinK ?? 1)) : 3;
    // A collapsed box (M2.13-2): one pin per group of several, at its middle.
    const anchors = collapsed && node.groups ? groupAnchors(node) : null;
    const grouped = new Set();
    for (const pin of [...node.ins, ...node.outs]) {
      const out = pin.direction === 'out';
      const anchor = anchors?.get(pin.name);
      if (anchor) {
        if (grouped.has(anchor.group)) continue;
        grouped.add(anchor.group);
        const g = anchor.group;
        const address = node.kind === 'instance' ? `${level.path}/${node.name}.${pin.name}` : `${level.path}.${pin.name}`;
        const shape = bakedPin(baked, level.transform, this.viewport, { ...pin, x: anchor.x, y: anchor.y }, radius);
        const prefix = node.kind === 'instance' ? `${level.path}/${node.name}.` : `${level.path}.`;
        const data = {
          class: `pin group ${pin.shape} connected`,
          'data-address': address,
          'data-members': g.pins.map((p) => `${prefix}${p.name}`).join(' '),
          'data-direction': pin.direction,
          'data-label': `${g.label} (${g.pins.length})`,
          'data-part': 'schematic',
        };
        if (shape.circle) group.append(el('circle', { ...data, ...shape.circle }));
        else if (shape.polygon.length) group.append(el('polygon', { ...data, points: shape.polygon.map((p) => `${p.x},${p.y}`).join(' ') }));
        continue;
      }
      const portAddress = node.kind === 'instance' ? `${address}.${pin.name}` : `${level.path}.${pin.name}`;
      const kind = ['pin', pin.shape, pin.parameter ? 'parameter' : '', pin.connected ? 'connected' : ''].filter(Boolean).join(' ');
      const data = { class: kind, 'data-address': portAddress, 'data-direction': pin.direction, 'data-label': pin.name, 'data-part': 'schematic' };
      const shape = bakedPin(baked, level.transform, this.viewport, pin, radius);
      if (shape.circle) group.append(el('circle', { ...data, ...shape.circle }));
      else if (shape.polygon.length) group.append(el('polygon', { ...data, points: shape.polygon.map((p) => `${p.x},${p.y}`).join(' ') }));
      // A composite's and a frame's pins are labelled by the level's labels (drawLabels), in its units as well.
      if (text(9) && !isOpen && !node.composite && node.kind === 'instance') {
        label({ x: pin.x + (out ? -6 : 6), y: pin.y + 3 }, 9, { class: 'pin-label', 'text-anchor': out ? 'end' : 'start' }, pin.name);
      }
      if (metered && pin.shape === 'signal') {
        // Fixed size whatever its state: only the fill's place and width change. An input's inside its box at the left, an output's at the right.
        const bar = el('g', { class: 'meter', 'data-state': 'idle' });
        const x = out ? pin.x - 34 : pin.x + 6;
        const y = pin.y + 5;
        const track = bakedRect(baked, { x, y, w: 28, h: 3 });
        if (track) bar.append(el('rect', { class: 'level', ...track }));
        const at = bakedPoint(baked, { x, y });
        const fill = el('rect', { class: 'level-fill', x: at.x, y: at.y, width: 0, height: 3 * m });
        bar.append(fill);
        const mark = bakedRect(baked, { x, y: y + 1, w: 28, h: 1 });
        if (mark) bar.append(el('rect', { class: 'level-mark', ...mark }));
        group.append(bar);
        bars.push([portAddress, { bar, fill, x: at.x, width: 28 * m }]);
      }
    }
    this.barsOf.set(group, bars);
    return group;
  }

  /** The level bars of every node drawn. */
  collectBars() {
    this.bars.clear();
    for (const drawn of this.groups.values()) {
      for (const { el: group } of drawn.nodeCache.values()) for (const [address, bar] of this.barsOf.get(group) ?? []) this.bars.set(address, bar);
    }
  }

  /** The element drawn for `hit` (an instance's group, a wire, a pin), or null when it is not drawn. */
  partElement(hit) {
    if (hit?.kind === 'instance') return this.nodeGroup(hit.address);
    if (hit?.kind === 'wire') return this.groups.get(hit.path)?.wireCache.get(hit.wire.id)?.el ?? null;
    if (hit?.kind === 'port') {
      // A frame's pin: its frame's group, by the frame's id.
      const group = hit.node.kind === 'instance' ? this.nodeGroup(`${hit.path}/${hit.node.name}`) : (this.groups.get(hit.path)?.nodeCache.get(hit.node.id)?.el ?? null);
      for (const pin of group?.querySelectorAll('.pin') ?? []) {
        if (pin.dataset.address === hit.address && pin.dataset.direction === hit.port.direction) return pin;
      }
      // A member of a collapsed group (M2.13-2): the group's pin is what is drawn of it.
      for (const pin of group?.querySelectorAll('.pin') ?? []) {
        if (pin.dataset.members?.split(' ').includes(hit.address) && pin.dataset.direction === hit.port.direction) return pin;
      }
    }
    return null;
  }

  /** Class `name` on `element` alone, written only when it moves. */
  mark(name, element) {
    const had = this.marked[name];
    if (had === element) return;
    had?.classList.remove(name);
    element?.classList.add(name);
    this.marked[name] = element;
  }

  /** Class `selected` on `elements` (a Set) alone, each written only when it changes. */
  markSelected(elements) {
    for (const had of this.marked.selected) if (!elements.has(had)) had.classList.remove('selected');
    for (const element of elements) if (!this.marked.selected.has(element)) element.classList.add('selected');
    this.marked.selected = elements;
  }

  /** The selection, and a menu's target or the wire under the pointer, marked on their elements (built again or not). */
  applyMarks() {
    this.svg.classList.toggle('many', this.selected.length > MANY_MARKS);
    this.markSelected(new Set(this.selected.map((s) => this.partElement(s)).filter(Boolean)));
    this.mark('hover', this.partElement(this.menuState?.target.hit ?? this.hovered));
    this.syncHalo();
  }

  /** The wire under the pointer (a hit, or null), marked. */
  hoverWire(hit) {
    this.hovered = hit;
    this.mark('hover', this.partElement(this.menuState?.target.hit ?? hit));
  }

  /**
   * Each selected wire drawn again above the other wires of its level,
   * thick, over a soft halo: two paths in the level's overlay (between its
   * wires and its boxes) with the wire's own `d`, copied whenever it
   * differs (a new bake, a drag that rewrites it, a cancelled drag).
   * Nothing else is written: the wire keeps its place among the others.
   * Each selected pin (a disc; not one drawn as a polygon, past 1e7 px)
   * gets a ring of twice its radius in its box's group, which a drag moves
   * with the box. What is no longer selected or drawn loses its own.
   */
  syncHalo() {
    const rings = new Map();
    const halos = new Map();
    for (const s of this.selected.length > MANY_MARKS ? [] : this.selected) {
      const element = this.partElement(s);
      if (!element) continue;
      if (s.kind === 'port' && element.localName === 'circle') {
        const ring = this.rings.get(element) ?? el('circle', { class: 'ring' });
        if (ring.parentNode !== element.parentNode) element.parentNode.append(ring);
        for (const [name, value] of [['cx', element.getAttribute('cx')], ['cy', element.getAttribute('cy')], ['r', String(2 * Number(element.getAttribute('r')))]]) {
          if (ring.getAttribute(name) !== value) ring.setAttribute(name, value);
        }
        rings.set(element, ring);
      } else if (s.kind === 'wire') {
        const overlay = this.groups.get(s.path)?.overlay;
        if (!overlay) continue;
        const halo = this.halos.get(element) ?? { under: el('path', { class: 'halo' }), over: el('path') };
        const { under, over } = halo;
        if (under.parentNode !== overlay) overlay.append(under, over);
        const d = element.getAttribute('d');
        if (over.getAttribute('d') !== d) {
          under.setAttribute('d', d);
          over.setAttribute('d', d);
        }
        const classes = ['wire', s.wire.shape, s.wire.dashed ? 'dashed' : '', 'raised'].filter(Boolean).join(' ');
        if (over.getAttribute('class') !== classes) over.setAttribute('class', classes);
        halos.set(element, halo);
      }
    }
    for (const [element, ring] of this.rings) if (rings.get(element) !== ring) ring.remove();
    for (const [element, halo] of this.halos) {
      if (halos.get(element) === halo) continue;
      halo.under.remove();
      halo.over.remove();
    }
    this.rings = rings;
    this.halos = halos;
  }

  /**
   * A drag in progress: its box, the wires that change with it and an open
   * composite's inside follow it, in place (ui/lib/schematic-drag.js),
   * built again or not. The box moves in whole layout units, as it is
   * stored, so the release draws it where it was let go. Cheap when nothing
   * moved: a redraw during the drag (a stream frame) writes nothing.
   */
  applyDrag() {
    const gesture = this.gesture;
    if (!gesture?.follow || !gesture.moved || !this.drawn) return;
    gesture.follow.apply({ layout: this.layoutOf(gesture.hit.level), levels: this.drawn.levels, groups: this.groups, to: dragTo(gesture) });
    this.syncHalo();
  }

  /** The drag's next step at the next frame: one per frame, however many moves. */
  scheduleDrag() {
    if (this.dragPending) return;
    this.dragPending = true;
    requestAnimationFrame(() => {
      this.dragPending = false;
      this.applyDrag();
    });
  }

  drawCrumbs() {
    const crumbs = breadcrumbs(this.drawn);
    const key = `${this.graph.project}\n${crumbs.map((c) => c.path).join('\n')}`;
    if (key === this.crumbKey) return;
    this.crumbKey = key;
    // Nothing selected, the sidebar names the level shown.
    if (!this.selected.length) this.showSide();
    this.crumbs.replaceChildren(
      ...crumbs.map((crumb, i) => {
        const button = html('button', { type: 'button', textContent: i === 0 ? this.graph.project ?? 'project' : crumb.name });
        button.addEventListener('click', () => this.goTo(crumb.path));
        return button;
      }),
    );
    this.fitCrumbs();
  }

  /**
   * The breadcrumbs cut in the middle to fit the toolbar (2026-10-22: never
   * wrapping over the drawing, nor scrolling): the top level and the
   * innermost stay, the ones after the first hidden one by one behind a
   * "…" until the rest fit (the last cut short, `text-overflow`, if even
   * that is too wide).
   */
  fitCrumbs() {
    const buttons = [...this.crumbs.children].filter((b) => b.tagName === 'BUTTON');
    this.crumbs.querySelector('.cut')?.remove();
    for (const b of buttons) b.hidden = false;
    if (buttons.length < 3 || this.crumbs.scrollWidth <= this.crumbs.clientWidth) return;
    const cut = html('span', { className: 'cut', textContent: '…' });
    buttons[0].after(cut);
    for (let i = 1; i < buttons.length - 1 && this.crumbs.scrollWidth > this.crumbs.clientWidth; i++) buttons[i].hidden = true;
    cut.title = buttons.filter((b) => b.hidden).map((b) => b.textContent).join(' / ');
  }

  /** The level shown: the deepest breadcrumb's (`""` the top level, else its composite). */
  currentLevel() {
    return this.drawn ? breadcrumbs(this.drawn).at(-1).level : '';
  }

  /** Back out to the level at `path` (a breadcrumb), its inside filling the view, what is open below it closed. */
  goTo(path) {
    if (path === '') {
      this.fitTop();
      return;
    }
    for (const open of [...this.pinned]) if (open !== path && open.startsWith(`${path}/`)) this.pinned.delete(open);
    const rect = this.drawn && levelRect(this.drawn, path);
    const level = rect && this.levelOf(path);
    // Its inside filling the view, readable (M2.13-3, `crumbView`).
    if (level) this.setView(crumbView(this.view, level, rect, this.viewport, MARGIN_PX, MARGIN_PX, FRAME_SIDE_PX));
    else this.schedule();
  }

  /**
   * Composite instance `hit` (a closed one's box, clicked at screen point
   * `at`) opened in place (2026-10-22): pinned, one per level (`pinOne`),
   * its frame growing where its box is, the box's point under `at` kept as
   * the same point of the frame (`openAnchor`), so the view does not jump.
   * Whether it opened: a library composite (its inside unknown) does not.
   */
  openInPlace(hit, at) {
    if (hit?.kind !== 'instance' || hit.node.kind !== 'instance' || !hit.node.composite || this.drawn?.open.has(hit.address)) return false;
    if (!this.layoutOf(hit.node.module) || !chain((level) => this.layoutOf(level), hit.address)) return false;
    this.pinned = pinOne(this.pinned, hit.address);
    this.openedAt = at;
    this.schedule();
    return true;
  }

  /**
   * The open composite at `address` closed (its ×, Escape, Close;
   * 2026-10-22), and what is open inside it: the view is not zoomed, and
   * the pan its opening by a click made (`openAnchor`, kept in its
   * parent's units) is undone, so its box comes back where it was before
   * it opened, at the scale shown now (opened otherwise, the levels around
   * stay still and the box lies where its frame grew from).
   */
  closeInPlace(address) {
    const shift = this.openShifts.get(address);
    const parent = this.drawn?.levels.find((l) => l.path === address.slice(0, address.lastIndexOf('/')));
    this.pinned = unpin(this.pinned, address);
    if (this.revealPinned) this.revealPinned = unpin(this.revealPinned, address);
    if (shift && parent) this.setView(pan(this.view, -shift.dx * parent.transform.k, -shift.dy * parent.transform.k));
    else this.schedule();
  }

  /** Whether a click at `at` (screen px, at `time`) is the second of a double-click whose first opened or closed a composite (`last`, else `lastToggle`). */
  secondClick(at, time, last = this.lastToggle) {
    return Boolean(last) && time - last.time <= DOUBLE_MS && Math.hypot(at.x - last.at.x, at.y - last.at.y) <= DOUBLE_PX;
  }

  // Input.

  point(event) {
    const rect = this.svg.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  pointerDown(event) {
    // A control-click is the Mac's right click: the context menu's, not a press.
    if (event.button !== 0 || (MAC && event.ctrlKey) || !this.drawn) return;
    // Another press while one is held: the first is let go, as cancelled.
    if (this.gesture) this.cancelGesture();
    this.focus({ preventScroll: true });
    const at = this.point(event);
    const hit = hitTest(this.drawn, at);
    this.svg.setPointerCapture?.(event.pointerId);
    // Shift or ⌘ adds to the selection or takes out of it.
    const additive = event.shiftKey || event.metaKey || (!MAC && event.ctrlKey);
    const base = { start: at, last: at, hit, moved: false, additive, pointer: event.pointerId };
    // An open frame's × (2026-10-22): closes on release, never selects, drags or pans.
    if (hit.kind === 'close') {
      this.gesture = { ...base, close: true };
      return;
    }
    // Shift-drag on the background: the marquee (a plain drag pans).
    if (hit.kind === 'background' && event.shiftKey) {
      this.gesture = { ...base, marquee: true };
      return;
    }
    // Edit patch: a press on an output starts a connection (a Shift- or
    // ⌘-click selects the pin instead).
    // Patching and moving happen at the innermost open level only (M2.13-3): around it a press selects or pans.
    const own = editable(this.drawn, hit);
    if (!additive && own && this.editor.starts(hit)) {
      this.gesture = { ...base, connect: true };
      this.editor.connectStart(hit, at);
      return;
    }
    // The level's scale is kept from the start: a render during the drag
    // (a push, a zoom) may replace the group or close the level. A box of
    // a group selected moves the group's boxes of its level with it.
    const k = hit.kind === 'instance' ? this.levelOf(hit.path)?.transform.k : undefined;
    const follow = k && own && this.nodeGroup(hit.address) ? new DragFollow(hit.level, dragGroup(this.selected, hit)) : null;
    // A box around the innermost open level is not moved (and the view not panned instead).
    const held = hit.kind === 'instance' && !own && !additive;
    this.gesture = { ...base, k, follow, held };
  }

  nodeGroup(address) {
    const cut = address.lastIndexOf('/');
    return this.groups.get(address.slice(0, cut))?.nodeCache.get(address.slice(cut + 1))?.el ?? null;
  }

  pointerMove(event) {
    const at = this.point(event);
    const gesture = this.gesture;
    if (!gesture) {
      this.hover(at);
      return;
    }
    if (event.pointerId !== gesture.pointer) return;
    if (!gesture.moved && Math.hypot(at.x - gesture.start.x, at.y - gesture.start.y) > DRAG_PX) {
      gesture.moved = true;
      // Its level drawn again unbundled, so that the wires of a box dragged follow it.
      if (gesture.follow) this.schedule();
    }
    if (gesture.connect) {
      this.editor.connectMove(at);
      return;
    }
    if (!gesture.moved || gesture.close) return;
    const last = gesture.last;
    gesture.last = at;
    if (gesture.marquee) this.showMarquee(gesture);
    else if (gesture.follow) this.scheduleDrag();
    else if (gesture.held) {
      // A box around the open composite (M2.13-3): not moved, and said so once.
      if (!gesture.said) this.say(`Not moved: ${gesture.hit.address} is around the open ${editLevel(this.drawn).path}; close it (×) first`);
      gesture.said = true;
    } else this.setView(pan(this.view, at.x - last.x, at.y - last.y));
  }

  /** The marquee's rectangle on screen, from the press to the pointer (`{x, y, w, h}`, `w` and `h` positive). */
  marqueeRect(gesture) {
    const x = Math.min(gesture.start.x, gesture.last.x);
    const y = Math.min(gesture.start.y, gesture.last.y);
    return { x, y, w: Math.abs(gesture.last.x - gesture.start.x), h: Math.abs(gesture.last.y - gesture.start.y) };
  }

  showMarquee(gesture) {
    const r = this.marqueeRect(gesture);
    Object.assign(this.marquee.style, { transform: `translate(${Math.round(r.x)}px, ${Math.round(r.y)}px)`, width: `${Math.round(r.w)}px`, height: `${Math.round(r.h)}px`, visibility: 'visible' });
  }

  hideMarquee() {
    this.marquee.style.visibility = 'hidden';
  }

  levelOf(path) {
    return this.drawn.levels.find((l) => l.path === path);
  }

  pointerUp(event) {
    const gesture = this.gesture;
    if (!gesture || event.pointerId !== gesture.pointer) return;
    this.gesture = null;
    const at = this.point(event);
    if (gesture.connect && gesture.moved) {
      this.editor.connectEnd(hitTest(this.drawn, at));
      return;
    }
    if (gesture.connect) this.editor.cancelConnect();
    if (gesture.close) {
      // Let go on its × still (a press slid off it does nothing).
      const over = gesture.moved ? hitTest(this.drawn, at) : gesture.hit;
      if (over.kind === 'close' && over.address === gesture.hit.address) {
        this.closeInPlace(gesture.hit.address);
        this.lastToggle = { at, time: event.timeStamp };
      }
      return;
    }
    if (gesture.marquee) {
      this.hideMarquee();
      if (!gesture.moved) return;
      gesture.last = at;
      // Shift held throughout: what it takes is added to the selection.
      this.select(union(this.selected, inside(this.drawn, this.marqueeRect(gesture))));
      return;
    }
    if (!gesture.moved) {
      // The second click of a double-click on what the first opened or closed: the double-click's alone (`doubleClick`).
      if (this.secondClick(at, event.timeStamp)) return;
      const hit = gesture.hit.kind === 'background' ? null : gesture.hit;
      // A bundle (M2.13-2): its wires, selected together; they are then drawn on their own, so a second click takes one.
      // A collapsed port group (M2.13-2): its members, never one picked for the user.
      if (hit?.kind === 'group') {
        this.select(gesture.additive ? union(this.selected, hit.ports) : hit.ports);
        return;
      }
      if (hit?.kind === 'bundle') {
        this.select(gesture.additive ? union(this.selected, hit.wires) : bundleClick(this.selected, hit, () => this.singleHit(hit, at)));
        return;
      }
      if (gesture.additive) {
        if (hit) this.select(toggled(this.selected, hit));
      } else {
        this.select(hit ? [hit] : []);
        // A plain click on a closed composite's box opens it in place (2026-10-22: the zoom no longer does).
        if (this.openInPlace(hit, at)) this.lastToggle = { at, time: event.timeStamp, hit };
      }
      return;
    }
    // A drag whose level is no longer the innermost open one (a zoom opened another meanwhile) moves nothing (M2.13-3).
    if (gesture.follow && !editable(this.drawn, gesture.hit)) {
      gesture.follow.restore();
      this.schedule();
      this.say(`Not moved: ${gesture.hit.address} is around the open ${editLevel(this.drawn).path}`);
      return;
    }
    if (gesture.follow) {
      this.drop(gesture, at);
      // Its level bundled again.
      this.schedule();
    }
  }

  /**
   * A drag of `gesture` let go at `at`: every box it moved (its group's,
   * each by as much as the one dragged) kept until the pushed `graph` holds
   * it and written, one `schematic.move` (its `moves` form for a group).
   */
  drop(gesture, at) {
    const { node, level, path } = gesture.hit;
    gesture.last = at;
    const to = dragTo(gesture);
    const dx = to.x - node.x;
    const dy = to.y - node.y;
    const byName = new Map((this.layoutOf(level)?.nodes ?? []).filter((n) => n.kind === 'instance').map((n) => [n.name, n]));
    const moves = [];
    for (const name of gesture.follow.names) {
      const box = name === node.name ? node : byName.get(name);
      if (box) moves.push({ instance: name, x: box.x + dx, y: box.y + dy });
    }
    // Drawn at the next frame from the layout with them, which is what the
    // drag showed; kept while written (`pending`) and until a graph holds them.
    if (!this.moved.has(level)) this.moved.set(level, new Map());
    const kept = this.moved.get(level);
    for (const m of moves) kept.set(m.instance, { x: m.x, y: m.y, pending: true });
    const old = new Map(this.layouts);
    const before = (l) => old.get(l) ?? null;
    this.layouts.delete(level);
    // Inside a composite the view is in (its breadcrumb), that inside
    // stays where it is and the levels around it are fitted to it again;
    // elsewhere what did not move stays (and an inside seen from outside
    // is fitted to its box again).
    const viewed = this.view.path ?? '';
    const within = path !== '' && (viewed === path || viewed.startsWith(`${path}/`));
    this.view = within ? reroot(this.view, before, path) : rebase(this.view, before, (l) => this.layoutOf(l));
    this.schedule();
    // A group too large for one message goes as several (`moveMessages`).
    for (const message of moveMessages(level, moves)) {
      const mine = (message.moves ?? [message]).map((m) => [m.instance, kept.get(m.instance)]);
      const settle = (failed) => {
        let changed = false;
        for (const [name, at] of mine) {
          if (kept.get(name) !== at) continue;
          at.pending = false;
          // Not written: the box goes back to where the graph has it.
          if (failed) changed = kept.delete(name) || changed;
        }
        if (changed) {
          this.layouts.delete(level);
          this.schedule();
        }
      };
      const request = this.connection?.request?.(message);
      if (!request) settle(true);
      else
        request.then(
          () => settle(false),
          (error) => {
            settle(true);
            this.say(`Not moved: ${error.message}`);
          },
        );
    }
  }

  cancelGesture() {
    this.gesture?.follow?.restore();
    if (this.gesture?.follow) this.schedule();
    if (this.gesture?.marquee) this.hideMarquee();
    if (this.gesture?.connect) this.editor.cancelConnect();
    this.gesture = null;
    this.syncHalo();
  }

  hover(at) {
    if (!this.drawn) return;
    const hit = hitTest(this.drawn, at);
    // An open frame's × (2026-10-22): a pointer, and its title says what it closes.
    this.svg.classList.toggle('closing', hit.kind === 'close');
    this.hoverWire(hit.kind === 'wire' ? hit : null);
    // A closed composite's box or pin under the pointer: its pins say where they go inside.
    const box = (hit.kind === 'instance' || hit.kind === 'port') && hit.node.kind === 'instance' && hit.node.composite ? `${hit.path}/${hit.node.name}` : null;
    this.hoverBox(box && !this.drawn.open.has(box) ? box : null);
    if (hit.kind === 'port' && (hit.node.composite || hit.node.kind !== 'instance')) {
      // A boundary pin: its label's whole text, which a small box shows cut short.
      this.showTip(this.pinLabel(hit.node.kind === 'instance', hit.address, hit.port.name, true), at);
      return;
    }
    if (hit.kind === 'bundle') {
      this.showTip(bundleText(hit.bundle), at);
      return;
    }
    if (hit.kind === 'group') {
      // A collapsed port group (M2.13-2): its members; zoom in to take one.
      this.showTip([`${hit.label} (${hit.ports.length})`, ...hit.ports.map((p) => p.address)].join('\n'), at);
      return;
    }
    if (hit.kind !== 'wire') {
      this.hideTip();
      return;
    }
    const converters = hit.wire.converters ?? [];
    this.showTip(converters.length ? `${hit.from} → ${hit.to}: ${converters.map(converterText).join('; ')}` : `${hit.from} → ${hit.to}`, at);
  }

  /** `hit`, or for a bundle (M2.13-2) the hit of its wire nearest screen `point`: what a menu acts on, one wire. */
  singleHit(hit, point) {
    if (hit.kind !== 'bundle') return hit;
    const entry = this.levelOf(hit.path);
    const wire = nearestWire(hit.bundle, toLocal(entry.transform, point));
    return hit.wires.find((w) => w.wire === wire) ?? hit.wires[0];
  }

  showTip(text, at) {
    this.tip.textContent = text;
    this.tip.style.transform = `translate(${Math.round(at.x + 12)}px, ${Math.round(at.y + 12)}px)`;
    this.tip.style.visibility = 'visible';
  }

  /** The closed composite under the pointer (its address, or null): its pins' labels say where they go inside. */
  hoverBox(address) {
    if (this.hoveredBox === address) return;
    this.hoveredBox = address;
    this.drawLabels();
  }

  hideTip() {
    this.tip.style.visibility = 'hidden';
  }

  doubleClick(event) {
    if (!this.drawn) return;
    // Its first click opened the composite (or closed it, by its ×): what it opened is entered, nothing else happens.
    const last = this.lastToggle;
    this.lastToggle = null;
    if (last && this.secondClick(this.point(event), event.timeStamp, last)) {
      if (last.hit) this.enter(last.hit);
      return;
    }
    const hit = hitTest(this.drawn, this.point(event));
    // Edit patch: the add palette, where the background or an open
    // composite's inside was double-clicked.
    const level = this.editor.editing ? paletteLevel(hit, this.drawn, this.point(event)) : null;
    if (level) {
      this.editor.openPalette(this.point(event), level);
      return;
    }
    if (hit.kind !== 'instance' || !hit.node.composite) return;
    this.enter(hit);
  }

  /**
   * Composite instance `hit` opened and its inside filling the view (a
   * double-click, the menu's Enter): the inside's own layout fitted, so it
   * is drawn readable however tall and narrow its box (a box of many pins
   * holds a wide inside small, centred in it), and never less readable
   * than READ_K (`enterView`: a wide inside overflows the view); the box's
   * frame reaches past the view's edges. One composite is pinned per level
   * (M2.13-3); a library composite (its inside unknown) is only focused.
   */
  enter(hit) {
    const inner = hit.node?.composite ? this.layoutOf(hit.node.module) : null;
    if (inner && chain((level) => this.layoutOf(level), hit.address)) {
      this.openShifts.delete(hit.address);
      this.pinned = pinOne(this.pinned, hit.address);
      this.setView({ ...enterView(inner.bounds, this.viewport, MARGIN_PX, MARGIN_PX, FRAME_SIDE_PX), path: hit.address });
      return;
    }
    const rect = nodeRect(this.drawn, hit.address);
    if (rect) this.setView(focus(this.view, rect, this.viewport, MARGIN_PX));
  }

  /** The person's zoom by `factor` about screen point `at` (null: the centre): the view only, opening and closing nothing (2026-10-22). */
  zoomBy(factor, at = null) {
    this.setView(zoomAt(this.view, factor, at ?? { x: this.viewport.w / 2, y: this.viewport.h / 2 }));
  }

  /** The wheel zooms (`schematic.zoom`): the schematic fills its window, so it owns the wheel (ADR 0034); smaller, in a page, it takes it once clicked. */
  wheel(event) {
    if (!takesWheel(this, event)) return;
    event.preventDefault();
    if (wheelCommand('schematic', event)?.id !== 'schematic.zoom') return;
    this.closeMenu();
    const delta = verticalDelta(event);
    if (!delta) return;
    this.zoomBy(wheelFactor(delta, { deltaMode: event.deltaMode, pinch: event.ctrlKey }), this.point(event));
  }

  pinch(event, start) {
    event.preventDefault();
    if (start) this.pinchScale = 1;
    this.closeMenu();
    const factor = event.scale / (this.pinchScale || 1);
    this.pinchScale = event.scale;
    if (Number.isFinite(factor) && factor > 0) this.zoomBy(factor, this.point(event));
  }

  /** The `schematic` scope's keys (ui/lib/keys.default.js), then Edit patch's (`schematic_edit`). */
  key(event) {
    // The menu's keys are its own (it stops them too): never the selection's Delete or ⌘Z.
    if (event.composedPath().includes(this.menu.list)) return;
    const own = event.composedPath()[0] === this;
    const command = keyCommand('schematic', event)?.id;
    // Edit patch is the child scope: its chords come first, but for the
    // built-in Escape order (a trace, an open composite and the selection go
    // before Edit patch is left).
    if (this.editor.editing) {
      const edit = keyCommand('schematic_edit', event)?.id;
      const cascade = edit === 'schematic_edit.leave' && command === 'schematic.back';
      if (edit && !cascade && this.editor.key(event)) return;
    }
    const back = own && command === 'schematic.back' && !this.editor.placing;
    // Escape (`schematic.back`) ends a trace first (ADR 0030 §1), then
    // closes the innermost open composite in place (2026-10-22; a click
    // that opens one also selects it), then lets the selection go (the add
    // palette's Escape is the palette's), then leaves Edit patch.
    if (back && this.tracing) {
      event.preventDefault();
      this.endTrace();
      return;
    }
    if (back && this.drawn && breadcrumbs(this.drawn).length > 1) {
      event.preventDefault();
      this.closeInPlace(breadcrumbs(this.drawn).at(-1).path);
      return;
    }
    // Trace the selection (the part selected last; several pins, such as a collapsed group's, together).
    if (own && command === 'schematic.trace') {
      event.preventDefault();
      if (this.selection) this.startTrace(traceTarget(this.selected));
      else this.say(`Trace: select a pin, a wire or a box first, then press ${shownKeys(keymap(), 'schematic.trace') || 'T'}`);
      return;
    }
    if (back && this.selected.length) {
      event.preventDefault();
      this.select([]);
      return;
    }
    // Every instance and connection of the level shown.
    if (own && command === 'schematic.select_all') {
      event.preventDefault();
      this.selectAll();
      return;
    }
    // Edit patch takes its keys first: Escape leaves edit mode before it
    // goes back out of a level.
    if (this.editor.key(event)) return;
    // Keys of the sidebar's sliders and buttons and the bar's are theirs (an
    // event from the shadow tree reaches this listener retargeted to the host).
    if (!own || !command) return;
    // The context menu from the keyboard, for the selection (else the background).
    if (command === 'schematic.context_menu') {
      event.preventDefault();
      this.keyboardMenu();
      return;
    }
    if (command === 'schematic.back') {
      event.preventDefault();
      return;
    }
    // Out to the top level as its breadcrumb does, every composite closed.
    if (command === 'schematic.top_level') {
      event.preventDefault();
      this.closeMenu();
      this.goTo('');
      return;
    }
    const view = commandView(this.view, command, this.viewport, this.rootBounds());
    if (view && view.k !== this.view.k && command !== 'schematic.fit') {
      event.preventDefault();
      this.closeMenu();
      this.zoomBy(view.k / this.view.k);
      return;
    }
    if (view) {
      // An arrow or a page pans, as a pointer pan: nothing opens or closes.
      event.preventDefault();
      this.closeMenu();
      this.setView(view);
    }
  }

  // The context menu.

  /**
   * The menu of what `event` (a right click, or the keyboard's) is over:
   * `{items, act(id), closed()}` (`schematicMenu`'s items), or null outside
   * the drawing (the bar, the sidebar, a field), whose right clicks are not
   * the schematic's. Signal.app's menu shows the items first; in a browser
   * this element's own does (`ownMenu`). `close()` closes whichever shows
   * them: called when its target goes with a pushed `graph`, and on a
   * scroll or a zoom. Until it closes the target is marked as hovered.
   */
  contextMenu(event, close = null) {
    if (!this.drawn) return null;
    const origin = event.composedPath?.()[0];
    // The keyboard's (`keyboardMenu`), or the system's on the focused element.
    const anchored = this.keyMenu ?? (origin === this ? this.menuAnchor() : null);
    if (!anchored && origin !== this.svg && !this.svg.contains(origin)) return null;
    const point = anchored ? anchored.point : this.point(event);
    const target = { hit: anchored ? anchored.hit : this.singleHit(partHit(this.drawn, hitTest(this.drawn, point)), point), point };
    this.endMenu();
    this.menuState = { target, close };
    this.hoverWire(this.hovered);
    return {
      items: schematicMenu(target.hit, this.menuContext(target.hit, point)),
      act: (id) => {
        if (this.menuState?.target === target) this.endMenu();
        this.focus({ preventScroll: true });
        this.menuAct(id, target);
      },
      closed: () => {
        if (this.menuState?.target === target) this.endMenu();
      },
    };
  }

  /**
   * A right click where no page menu claims the document (Signal.app's
   * does, `claimMenus`, and shows these items as the click reaches it) and
   * nothing took it before: this element's own menu.
   */
  ownMenu(event) {
    if (event.defaultPrevented || menusClaimed(this.ownerDocument)) return;
    const menu = this.contextMenu(event, () => this.menu.close());
    if (!menu) return;
    event.preventDefault();
    this.menu.show(menu.items, { x: event.clientX, y: event.clientY, pick: menu.act, closed: menu.closed, back: this });
  }

  /**
   * ContextMenu or Shift+F10: the menu of the selection (the background
   * without one, or when it is not drawn), at it, by way of a right click
   * on the drawing so that Signal.app's menu takes it as any other.
   */
  keyboardMenu() {
    if (!this.drawn) return;
    const anchored = this.menuAnchor();
    const rect = this.svg.getBoundingClientRect();
    this.keyMenu = anchored;
    try {
      this.svg.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, composed: true, cancelable: true, clientX: rect.left + anchored.point.x, clientY: rect.top + anchored.point.y }));
    } finally {
      this.keyMenu = null;
    }
  }

  /** The selection, where the keyboard's menu opens (`{hit, point}`, the point inside the view), or the background at the centre. */
  menuAnchor() {
    const s = this.selection;
    const level = s && this.partElement(s) ? this.levelOf(s.path) : null;
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    if (!level) return { hit: { kind: 'background', path: '', level: '' }, point: centre };
    let at = centre;
    if (s.kind === 'instance') at = toScreen(level.transform, { x: s.node.x + s.node.w / 2, y: s.node.y + s.node.h / 2 });
    else if (s.kind === 'port') at = toScreen(level.transform, s.port);
    else if (s.kind === 'wire') at = toScreen(level.transform, s.wire.points[Math.floor(s.wire.points.length / 2)]);
    const point = { x: Math.max(0, Math.min(this.viewport.w, at.x)), y: Math.max(0, Math.min(this.viewport.h, at.y)) };
    return { hit: s, point };
  }

  /** What `schematicMenu` needs of `hit`, opened at `point`. */
  menuContext(hit, point) {
    const context = { editing: this.editor.editing, level: this.currentLevel(), around: aroundReason(this.drawn, hit) };
    // One of several selected: the menu's Delete selected removes them all.
    if (this.selected.length > 1 && has(this.selected, hit)) {
      const plan = deletePlan(this.selected);
      Object.assign(context, { group: this.selected.length, deleted: plan.deleted, tooLarge: tooLargeText(plan) });
    }
    if (hit.kind === 'background') context.addable = Boolean(paletteLevel(hit, this.drawn, point));
    if (hit.kind === 'instance' && hit.node.composite) {
      context.open = this.drawn.open.has(hit.address);
      context.addable = context.open && Boolean(paletteLevel(hit, this.drawn, point));
      // A library composite (its inside not in `graph`) is not opened in place.
      context.opaque = !this.graph?.composites?.[hit.node.module];
    }
    if (hit.kind === 'port') {
      context.connected = this.portOps(hit).length;
      // Wires of the level's exports reaching it (not edited here).
      const ends = (w) => (hit.port.direction === 'out' ? w.from : w.to);
      context.exported = (this.levelOf(hit.path)?.layout.wires ?? []).filter((w) => w.kind === 'export' && ends(w).node === hit.node.id && ends(w).port === hit.port.name).length;
      // A library composite's box pin: its inside is not in `graph`, so Go inside is off.
      context.opaque = hit.node.kind === 'instance' && Boolean(hit.node.composite) && !this.graph?.composites?.[hit.node.module];
    }
    return context;
  }

  /** "Disconnect all" on pin `hit`: one op per connection of it at its level. */
  portOps(hit) {
    // A frame's pin has no connection of its own here: only exports reach it.
    if (hit.node.kind !== 'instance') return [];
    return disconnectOps(this.editor.levelGraph(hit.level), hit.node.name, hit.port);
  }

  /** The menu's target let go: no longer marked. */
  endMenu() {
    if (!this.menuState) return;
    this.menuState = null;
    this.hoverWire(this.hovered);
  }

  /** The menu closed by the schematic (a scroll, a zoom, its target gone), whichever menu shows it. */
  closeMenu() {
    const close = this.menuState?.close;
    this.endMenu();
    close?.();
  }

  /** Item `id` of the menu opened on `target` (`{hit, point}`). */
  menuAct(id, { hit, point }) {
    if (id === 'inspect') {
      this.select(hit.kind === 'group' ? hit.ports : [hit]);
      this.revealSide();
    } else if (id === 'trace') this.startTrace(hit);
    else if (id === 'delete-selected') this.deleteSelection();
    else if (id === 'open') this.openInPlace(hit, point);
    else if (id === 'close') this.closeInPlace(hit.address);
    else if (id === 'enter') this.enter(hit);
    // A boundary pin's other side (2026-10-20): the frame's pin inside its composite, or its box's pin around it.
    else if (id === 'go-inside') this.reveal({ kind: 'pin', address: hit.address, side: 'frame' });
    else if (id === 'go-outside') this.reveal({ kind: 'pin', address: hit.address, side: 'box' });
    else if (id === 'copy-address') this.copy(hit.kind === 'wire' ? `${hit.from} → ${hit.to}` : hit.address);
    else if (id === 'delete') this.editor.remove(hit);
    else if (id === 'scale') {
      this.select([hit]);
      this.revealSide();
      this.editor.openScale();
    } else if (id === 'source' || id === 'target') this.goToEnd(hit, id === 'source' ? 'from' : 'to');
    else if (id === 'disconnect') {
      // Its connections as now known: a push since the menu opened may have taken them.
      const ops = this.portOps(hit);
      if (ops.length) this.editor.write(0, hit.level, ops);
      else this.say(`Nothing to disconnect at ${hit.address}`);
    }
    else if (id === 'add') this.editor.openPalette(point, paletteLevel(hit, this.drawn, point));
    else if (id === 'fit') this.fitTop();
    else if (id === 'reset-layout') this.resetLayout(this.currentLevel());
    else if (id === 'edit') this.editor.setEditing(!this.editor.editing);
  }

  copy(text) {
    const written = navigator.clipboard?.writeText(text);
    if (!written) this.say('Not copied: no clipboard here');
    else written.catch((error) => this.say(`Not copied: ${error.message}`));
  }

  /**
   * End `side` (`from`, `to`) of wire `hit`: its pin selected (an
   * instance's; an export's end on the frame has no card) and the view
   * centred on it, at the same zoom.
   */
  goToEnd(hit, side) {
    const level = this.levelOf(hit.path);
    const end = hit.wire[side];
    const node = level?.layout.nodes.find((n) => n.id === end.node);
    const port = (side === 'from' ? node?.outs : node?.ins)?.find((p) => p.name === end.port);
    if (!port) return;
    this.select([portHit(level, node, port)]);
    const at = toScreen(level.transform, port);
    this.setView(pan(this.view, this.viewport.w / 2 - at.x, this.viewport.h / 2 - at.y));
  }

  resetLayout(level) {
    const message = level === null ? { type: 'schematic.reset' } : { type: 'schematic.reset', level };
    this.connection?.request?.(message).catch((error) => this.say(`Not reset: ${error.message}`));
  }

  // The selection and the sidebar.

  /** `selected` (a list of hits; `[]` none) is the selection, shown in the sidebar and marked. */
  select(selected) {
    this.selected = selected;
    this.showSide();
    this.schedule();
  }

  /** ⌘A: every instance and connection of the level shown (the deepest breadcrumb's). */
  selectAll() {
    if (!this.drawn) return;
    const path = breadcrumbs(this.drawn).at(-1).path;
    this.select(allOf(this.levelOf(path)));
  }

  /**
   * Delete (Edit patch) of the selection, or of a group from the menu: one
   * `patch` per level (`deletePlan`), written one after the other, each
   * once the last is answered; a refused one stops the rest.
   */
  async deleteSelection() {
    if (this.selected.length === 1) {
      this.editor.remove(this.selected[0]);
      return;
    }
    const plan = deletePlan(this.selected);
    // A patch is one message and one undo: one too large is not split, and nothing is written.
    if (plan.tooLarge.length) {
      this.editor.showNotice('Not written', tooLargeText(plan));
      return;
    }
    for (const { level, ops } of plan.patches) if (!(await this.editor.write(0, level, ops))) return;
  }

  // The sidebar.

  buildSide() {
    this.side = html('aside', { className: 'side' });
    this.side.setAttribute('aria-label', 'Selection');
    const head = html('div', { className: 'side-head' });
    this.collapseButton = html('button', { type: 'button', className: 'collapse' });
    this.collapseButton.addEventListener('click', () => {
      this.setCollapsed(!this.hasAttribute('side-collapsed'));
      this.focus({ preventScroll: true });
    });
    this.sideTitle = html('h2', { textContent: 'Selection' });
    head.append(this.collapseButton, this.sideTitle);
    this.sideBody = html('div', { className: 'side-body' });
    this.side.append(head, this.sideBody);
    let collapsed = false;
    try {
      collapsed = globalThis.localStorage?.getItem(SIDE_KEY) === 'collapsed';
    } catch {
      // No storage here (a private window, blocked site data): open.
    }
    this.setCollapsed(collapsed, false);
    return this.side;
  }

  /** The sidebar collapsed to a thin strip, or open; kept for this viewer when `keep`. */
  setCollapsed(collapsed, keep = true) {
    this.toggleAttribute('side-collapsed', collapsed);
    this.collapseButton.textContent = collapsed ? '›' : '‹';
    this.collapseButton.title = collapsed ? 'Show the sidebar' : 'Hide the sidebar';
    this.collapseButton.setAttribute('aria-expanded', String(!collapsed));
    if (!keep) return;
    try {
      globalThis.localStorage?.setItem(SIDE_KEY, collapsed ? 'collapsed' : 'open');
    } catch {
      // Not kept: it opens next time.
    }
  }

  /** The sidebar shown (opened if collapsed) and scrolled to its top: the menu's Inspect. */
  revealSide() {
    if (this.hasAttribute('side-collapsed')) this.setCollapsed(false);
    this.sideBody.scrollTop = 0;
  }

  /** What the sidebar's buttons depend on besides the selection: a selected composite open (its Open in place or Close). */
  sideStateKey() {
    if (!this.drawn) return '';
    // The level edited (M2.13-3): a part around it has its edits off, so its buttons follow it opening and closing.
    const edited = this.selected.length ? editLevel(this.drawn).path : '';
    const s = this.selected.length === 1 ? this.selected[0] : null;
    if (s?.kind !== 'instance' || !s.node.composite) return edited;
    return `${edited} ${this.drawn.open.has(s.address)}`;
  }

  /**
   * A `modules` reply (the add palette's, or the sidebar's own request):
   * what each type is, for the sidebar's description and group.
   */
  modulesSeen(modules) {
    this.modulesKnown = { info: moduleInfo(modules), project: projectKey(this.graph), asking: null };
    if (this.selected.length === 1 && this.selected[0].kind === 'instance') this.showSide();
  }

  /**
   * The types' descriptions for the sidebar: asked for once per project
   * when a primitive is selected (ADR 0027 §6: a native primitive shows its
   * formula when selected); null until known.
   */
  typeInfo() {
    const known = this.modulesKnown;
    const project = projectKey(this.graph);
    if (known.info && known.project === project) return known.info;
    const want = this.selected.length === 1 && this.selected[0].kind === 'instance' && !this.selected[0].node.composite;
    if (want && known.asking !== project && this.connection?.request) {
      known.asking = project;
      this.connection.request({ type: 'modules' }).then(
        (modules) => {
          if (projectKey(this.graph) === project) this.modulesSeen(modules);
        },
        () => {
          if (this.modulesKnown === known) known.asking = null;
        },
      );
    }
    return known.project === project ? known.info : null;
  }

  /** The sidebar's model of the selection now (ui/lib/schematic-sidebar.js). */
  sideModel() {
    const drawn = this.drawn;
    const crumb = drawn ? breadcrumbs(drawn).at(-1) : { path: '', level: '' };
    return sidebar(this.selected, {
      graph: this.graph,
      description: this.connection?.description,
      types: this.typeInfo(),
      layoutOf: (level) => this.layoutOf(level),
      editing: this.editor.editing,
      shown: { path: crumb.path, level: crumb.level },
      menu: (hit) => (drawn ? this.menuContext(hit, { x: this.viewport.w / 2, y: this.viewport.h / 2 }) : {}),
    });
  }

  /**
   * The sidebar built again for the selection. Its width never changes,
   * whatever it shows; its scroll is kept while it shows the same parts.
   */
  showSide() {
    if (!this.sideBody) return;
    this.stopCardStream();
    const model = this.sideModel();
    this.cardModel = model;
    this.cardRows = [];
    this.sideMeter = null;
    this.sideShownKey = this.sideStateKey();
    const key = this.selected.map(hitKey).join('\n');
    // A Trace section's link keeps the sidebar where it is (the person walks down its tree).
    const scroll = key === this.sideKey || this.keepSideScroll ? this.sideBody.scrollTop : 0;
    this.keepSideScroll = false;
    this.sideKey = key;
    this.sideTitle.textContent = model.kind === 'project' ? 'Project' : model.kind === 'group' ? (model.pinGroup ? 'Pin group' : 'Selection') : model.kind === 'port' ? 'Pin' : model.kind === 'wire' ? 'Connection' : 'Instance';
    const parts = [...this.traceParts(), html('h3', { textContent: model.title, title: model.title }), html('div', { className: 'subtitle', textContent: model.subtitle })];
    // What a primitive's type is: its rustdoc summary with its formula (ADR 0027 §6).
    if (model.about) parts.push(html('p', { className: 'about', textContent: model.about }));
    if (model.actions?.length) parts.push(this.actionButtons(model.actions));
    if (model.fields.length) {
      const list = html('dl');
      for (const [label, value] of model.fields) list.append(html('dt', { textContent: label }), html('dd', { textContent: value }));
      parts.push(list);
    }
    if (model.kind === 'project') parts.push(...this.legendParts());
    if (model.kind === 'instance') parts.push(...this.pinParts(model));
    if (model.kind === 'wire') parts.push(...this.wireParts(model));
    if (model.kind === 'port') parts.push(...this.portParts(model));
    if (model.kind === 'group') parts.push(...this.groupParts(model));
    if (model.parameters?.length) {
      parts.push(html('h4', { textContent: model.editable ? 'Parameters' : 'Parameters (set in the composite’s file)' }));
      for (const row of model.parameters) parts.push(this.parameterRow(row));
    }
    if (this.selected.length === 1) parts.push(...this.editor.cardParts(this.selected[0]));
    this.sideBody.replaceChildren(...parts);
    this.sideBody.scrollTop = scroll;
    this.refreshCard();
    this.updateCardStream();
    this.applyBars();
  }

  /** The actions' buttons: menu items (`{id, label, disabled, title}`), done on the selection. */
  actionButtons(actions) {
    const row = html('div', { className: 'actions' });
    for (const action of actions) {
      const button = html('button', { type: 'button', textContent: action.label, disabled: Boolean(action.disabled) });
      if (action.title) button.title = action.title;
      button.addEventListener('click', () => this.sideAct(action.id));
      row.append(button);
    }
    return row;
  }

  /** Action `id` of the sidebar, on the selection. */
  sideAct(id) {
    const selected = this.selected;
    this.focus({ preventScroll: true });
    if (id === 'delete-all') this.deleteSelection();
    else if (id === 'copy-addresses') this.copy(addressesText(selected));
    else if (id === 'trace' && selected.length > 1) this.startTrace(traceTarget(selected));
    else if (selected.length === 1) this.menuAct(id, { hit: selected[0], point: { x: this.viewport.w / 2, y: this.viewport.h / 2 } });
    // What the action changed (Open in place, Close) shows at the next frame's check; the rest now.
    this.schedule();
  }

  /** A link that selects `hit` (Shift or ⌘: adds it or takes it out), or plain text when there is nothing to select. */
  link(label, hit) {
    const button = html('button', { type: 'button', className: 'link', textContent: label, disabled: !hit });
    if (hit) {
      button.title = `Select ${label}`;
      button.addEventListener('click', (event) => {
        this.focus({ preventScroll: true });
        this.select(event.shiftKey || event.metaKey || (!MAC && event.ctrlKey) ? toggled(this.selected, hit) : [hit]);
      });
    }
    return button;
  }

  legendParts() {
    const list = html('div', { className: 'legend' });
    for (const entry of LEGEND) {
      const swatch = el('svg', { viewBox: '0 0 28 10', 'aria-hidden': 'true' });
      if (entry.swatch.startsWith('wire')) swatch.append(el('path', { class: entry.swatch, d: 'M1 5 C 10 5, 18 5, 27 5' }));
      else swatch.append(el('circle', { class: entry.swatch, cx: 14, cy: 5, r: 3.5 }));
      list.append(swatch, html('span', { textContent: entry.label }));
    }
    return [html('h4', { textContent: 'Legend' }), list, html('div', { className: 'subtitle', textContent: 'A click on a composite opens it in place, the × in its corner (or Escape) closes it. Shift- or ⌘-click adds to the selection, Shift-drag on the background selects the boxes inside, ⌘A the level shown, Escape lets go.' })];
  }

  /** A connection's line: the wire (a link), its other end (a link), and in Edit patch its removal. */
  connectionItem(c, level) {
    const item = html('li');
    const what = html('span');
    // In the signal's direction: an output's wire, then the pin it reaches
    // (`→ wire /echo.in`); an input's source, then its wire (`/melody.hz wire →`).
    const out = c.wire.label.startsWith('→');
    const wire = this.link(out ? '→ wire' : 'wire →', c.wire.hit);
    wire.title = `Select the wire ${c.wire.hit.from} → ${c.wire.hit.to}`;
    const end = this.link(c.end.label, c.end.hit);
    if (out) what.append(wire, ' ', end);
    else what.append(end, ' ', wire);
    item.append(what);
    if (this.editor.editing && c.removable) {
      // Around the innermost open composite (M2.13-3): shown, off, its title saying why.
      const around = this.drawn && level !== editLevel(this.drawn).level ? aroundReason(this.drawn, c.wire.hit) : null;
      const remove = html('button', { type: 'button', className: 'remove', textContent: '×', title: around ?? 'Remove this connection', disabled: Boolean(around) });
      remove.addEventListener('click', () => {
        this.focus({ preventScroll: true });
        const { from, to } = c.wire.hit.wire.written;
        this.editor.write(0, level, [removeConnection(from, to)]);
      });
      item.append(remove);
    } else item.append(html('span'));
    return item;
  }

  pinParts(model) {
    const parts = [html('h4', { textContent: 'Pins' })];
    for (const pin of model.pins) {
      const row = html('div', { className: 'pinrow' });
      const dot = html('span', { className: ['dot', pin.shape, pin.connected ? 'connected' : ''].filter(Boolean).join(' ') });
      const what = html('span', { className: 'what', textContent: [pin.role === 'parameter' ? 'param' : pin.direction, pin.kind, pin.rate == null ? null : `÷${pin.rate}`].filter(Boolean).join(' · ') });
      row.append(dot, this.link(pin.name, pin.hit), what);
      parts.push(row);
      if (pin.connections.length) {
        const wires = html('ul', { className: 'wires' });
        for (const c of pin.connections) wires.append(this.connectionItem(c, pin.hit.level));
        parts.push(wires);
      }
    }
    return parts;
  }

  wireParts(model) {
    const list = html('dl');
    for (const end of model.links) {
      const dd = html('dd');
      dd.append(this.link(end.label, end.hit));
      list.append(html('dt', { textContent: end.name }), dd);
    }
    return [html('h4', { textContent: 'Ends' }), list];
  }

  portParts(model) {
    const parts = [];
    if (model.meter) {
      // Fixed size whatever its state: only the fill's scale and the word change.
      const row = html('div', { className: 'meter-row' });
      const track = html('div', { className: 'meter-track' });
      const fill = html('div', { className: 'meter-fill' });
      track.append(fill);
      const state = html('span', { className: 'meter-state' });
      row.append(track, state);
      this.sideMeter = { address: this.selected[0].address, fill, state, shown: null };
      parts.push(html('h4', { textContent: 'Level' }), row);
    }
    parts.push(html('h4', { textContent: `Connections (${model.connections.length})` }));
    if (model.connections.length) {
      const list = html('ul', { className: 'wires' });
      for (const c of model.connections) list.append(this.connectionItem(c, this.selected[0].level));
      parts.push(list);
    } else parts.push(html('div', { className: 'subtitle', textContent: 'none' }));
    // A boundary pin's other side (2026-10-20): each a link that goes there (into the composite, or back out).
    const across = model.across;
    if (across) {
      parts.push(html('h4', { textContent: `${across.title} (${across.items.length})` }));
      if (across.items.length) {
        const list = html('ul', { className: 'wires across' });
        for (const item of across.items) {
          const li = html('li');
          li.append(this.revealLink(item.label, item.reveal));
          list.append(li);
        }
        parts.push(list);
      } else parts.push(html('div', { className: 'subtitle', textContent: across.empty }));
    }
    return parts;
  }

  groupParts(model) {
    const parts = model.notes.map((text) => html('div', { className: 'note', textContent: text }));
    parts.push(html('h4', { textContent: 'Members' }));
    const list = html('ul');
    for (const member of model.members) {
      const item = html('li');
      item.append(this.link(member.label, member.hit));
      list.append(item);
    }
    if (model.more) list.append(html('li', { className: 'subtitle', textContent: `and ${model.more} more` }));
    parts.push(list);
    if (model.counts.instance) parts.push(html('div', { className: 'subtitle', textContent: 'Drag one of the boxes to move those of its level together.' }));
    return parts;
  }

  parameterRow(row) {
    const box = html('div', { className: 'row' });
    const name = html('span', { textContent: row.name, title: row.address });
    const base = html('span', { className: 'readout', title: 'base' });
    const effective = html('span', { className: 'readout', title: 'effective' });
    const mark = html('span', { className: 'mark', textContent: '•', title: 'unsaved' });
    box.append(name, base, effective, mark);
    const shown = { row, base, effective, mark, input: null };
    if (row.editable) {
      // ADR 0043: a discrete parameter is a selector of its named values.
      const input = isDiscrete(row) ? html('select', {}) : html('input', { type: 'range', min: row.min, max: row.max, step: (row.max - row.min) / 1000 || 'any' });
      // ADR 0034: every control has a name a screen reader says.
      input.setAttribute('aria-label', row.name);
      if (isDiscrete(row)) for (const option of selectOptions(row.values)) input.append(html('option', { value: option.value, textContent: option.label, title: option.title }));
      input.dataset.address = row.address;
      input.dataset.label = row.name;
      input.addEventListener('input', () => this.send(dragMessage(row.address, Number(input.value))));
      input.addEventListener('change', () => this.send(releaseMessage(row.address, Number(input.value))));
      input.addEventListener('dblclick', () => this.send(resetMessage(row.address)));
      // A typed value (docs/ui-controls.md "Value entry"): a double-click on
      // the base, or Enter on the slider; sent as a release is, the clamp's
      // reason on the message line.
      shown.entry = new ValueEntry(base, {
        spec: () => ({ unit: row.unit, range: { min: row.min, max: row.max }, taper: row.taper, kind: row.kind }),
        value: () => shown.value,
        apply: (value) => {
          shown.value = value;
          input.value = isDiscrete(row) ? selectValue(row.values, value, row.snap) : String(value);
          this.send(dragMessage(row.address, value));
          this.send(releaseMessage(row.address, value));
        },
        note: (text) => {
          if (text) this.say(text);
          else if (this.message.textContent === shown.notice) this.say('');
          shown.notice = text;
        },
        closed: () => this.refreshCard(),
        focus: () => input.focus({ preventScroll: true }),
      });
      input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' && shown.entry.open()) event.preventDefault();
      });
      box.append(input);
      shown.input = input;
    }
    this.cardRows.push(shown);
    return box;
  }

  send(message) {
    this.connection?.request?.(message).catch((error) => this.say(error.message));
  }

  /** The card's values, in place: same boxes, same widths. */
  refreshCard() {
    for (const shown of this.cardRows) {
      const values = rowValues(shown.row, this.cardValues, this.connection?.unsaved, this.connection?.description);
      shown.value = values.base;
      const text = (v) => (isDiscrete(shown.row) ? valueLabel(shown.row.values, v, shown.row.snap) : formatFixed(v, shown.row.unit));
      if (!shown.entry?.isOpen) shown.base.textContent = text(values.base);
      shown.effective.textContent = text(values.effective);
      shown.mark.classList.toggle('on', Boolean(values.unsaved));
      if (shown.input && document.activeElement !== shown.input && this.shadowRoot.activeElement !== shown.input && Number.isFinite(values.base)) {
        shown.input.value = isDiscrete(shown.row) ? selectValue(shown.row.values, values.base, shown.row.snap) : String(values.base);
      }
    }
  }

  updateCardStream() {
    const stream = this.active ? cardStream(this.cardModel) : null;
    const key = stream ? JSON.stringify(stream) : null;
    if (key === this.cardKey) return;
    this.stopCardStream();
    if (!stream || !this.connection) return;
    this.cardKey = key;
    this.cardStop = this.connection.subscribe(stream, (frame) => {
      stream.addresses.forEach((address, i) => {
        const v = frame.values?.[i];
        if (v) this.cardValues.set(address, v);
      });
      this.refreshCard();
    });
  }

  stopCardStream() {
    this.cardStop?.();
    this.cardStop = null;
    this.cardKey = null;
    this.cardValues.clear();
  }

  // The trace (ADR 0030 §1).

  /** A trace of `hit` (a pin, a frame's pin, a wire, a box) in `direction` (the last trace's, else both): highlighted, badged, in the sidebar. */
  startTrace(hit, direction = this.traceDirectionLast ?? 'both', depth = this.traceDepthLast ?? 'through') {
    const start = startOf(hit);
    const result = start && this.graph ? trace(this.graph, start, direction, { depth }) : null;
    if (!result) {
      this.say('Nothing to trace there');
      return;
    }
    this.setTrace({ start, direction, depth, result });
    this.revealSide();
  }

  setTrace(tracing) {
    this.tracing = tracing ? { ...tracing, badges: new Map(), resolver: resolver(this.graph) } : null;
    if (tracing) {
      this.traceDirectionLast = tracing.direction;
      this.traceDepthLast = tracing.depth;
    }
    this.traceVersion += 1;
    this.showSide();
    this.schedule();
  }

  /** The trace's direction changed (the sidebar's toggle). */
  traceDirection(direction) {
    const t = this.tracing;
    if (!t || t.direction === direction) return;
    this.setTrace({ ...t, direction, result: trace(this.graph, t.start, direction, { depth: t.depth }) });
  }

  /** The trace's depth changed (the sidebar's toggle, M2.13-2): to the next module, or through modules. */
  traceDepth(depth) {
    const t = this.tracing;
    if (!t || t.depth === depth) return;
    this.setTrace({ ...t, depth, result: trace(this.graph, t.start, t.direction, { depth }) });
  }

  /** The trace computed again on a new `graph` (its start found by name); ended, and said, when its start went. */
  retrace() {
    const t = this.tracing;
    const result = trace(this.graph, t.start, t.direction, { depth: t.depth });
    if (!result) {
      this.say(`Trace ended: ${t.result.label} is no longer in the project`);
      this.tracing = null;
      this.traceVersion += 1;
      return;
    }
    this.tracing = { ...t, result, badges: new Map(), resolver: resolver(this.graph) };
    this.traceVersion += 1;
  }

  endTrace() {
    if (!this.tracing) return;
    this.setTrace(null);
  }

  /**
   * The trace on the elements drawn: `traced` on its wires, pins and boxes
   * (each level's written again only when its elements or the trace
   * changed; a pan writes nothing), `tracing` on the SVG, and the badges.
   */
  applyTrace() {
    const result = this.tracing?.result ?? null;
    this.svg.classList.toggle('tracing', Boolean(result));
    for (const [path, drawn] of this.groups) {
      if (drawn.tracedFor?.version === this.traceVersion && drawn.tracedFor.shown === drawn.shown) continue;
      drawn.tracedFor = { version: this.traceVersion, shown: drawn.shown };
      const wires = result?.wires.get(path);
      // A bundle stays as it is: its traced wires are drawn on their own above it.
      for (const { item, el: wire } of drawn.wireCache.values()) if (item.kind !== 'bundle') wire.classList.toggle('traced', Boolean(wires?.has(wireKey(item.written))));
      // The links across an open composite's edge: traced with their port.
      for (const link of drawn.links.children) link.classList.toggle('traced', Boolean(result?.pins.has(link.dataset.address)));
      for (const { item, el: group } of drawn.nodeCache.values()) {
        let any = false;
        for (const pin of group.querySelectorAll('.pin')) {
          // A collapsed group's pin is traced when any of its members is.
          const on = Boolean(result && (pin.dataset.members ? pin.dataset.members.split(' ').some((a) => result.pins.has(a)) : result.pins.has(pin.dataset.address)));
          pin.classList.toggle('traced', on);
          any ||= on;
        }
        const instance = item.kind === 'instance' && Boolean(result?.instances.has(`${path}/${item.name}`));
        group.classList.toggle('traced', instance || any);
      }
    }
    this.drawLabels();
  }

  /** A trace's summary for composite pin `address` on `side` (`inside`, `outside`), cached per trace. */
  badgeFor(address, side) {
    const t = this.tracing;
    const key = `${side} ${address}`;
    if (!t.badges.has(key)) {
      const answer = behind(this.graph, address, side, t.resolver);
      t.badges.set(key, answer ? badgeText(answer, address, side) : null);
    }
    return t.badges.get(key);
  }

  /**
   * The label of a boundary pin (M2.13-2): its name; on a frame's pin
   * (`box` false) where it connects outside, always; on a closed
   * composite's (`box`) where it goes inside, when `inside` (the box
   * hovered) or traced; and a traced pin's trace summary after its note,
   * never instead of it.
   */
  pinLabel(box, address, name, inside) {
    const side = box ? 'inside' : 'outside';
    const traced = Boolean(this.tracing?.result?.pins.has(address));
    let note = null;
    if (!box || inside || traced) {
      const key = `${side} ${address}`;
      if (!this.notes.has(key)) this.notes.set(key, (box ? insideNote : outsideNote)(this.graph, address, this.noteResolver));
      note = this.notes.get(key);
    }
    return pinText(name, note, traced ? this.badgeFor(address, side) : null);
  }

  /**
   * The port labels of the closed composites' boxes and of the frames, the
   * boxes' summaries and the bundles' counts (M2.13-2), for every level
   * drawn: in each level's own units, in its group, so they scale with the
   * drawing like its boxes (2026-10-20, the owner's "everything should zoom
   * nicely"). What they say goes by size (`labelDetail`): every pin, else
   * the groups, else a summary; a note where a pin connects from NOTE_PX;
   * a text smaller than READ_PX on screen is hidden, never kept at a size.
   * A level's labels are built again only when what they say changes; a
   * zoom shows or hides each size of text (its group) and moves nothing.
   */
  drawLabels() {
    // Widths measured in the labels' own font, once per text (canvas measureText), so a label always holds its text.
    if (!this.labelFont) {
      const family = getComputedStyle(this.svg).fontFamily;
      const context = document.createElement('canvas').getContext('2d');
      if (family && context) {
        this.labelFont = `${LABEL_PX}px ${family}`;
        context.font = this.labelFont;
        setMeasure((text) => context.measureText(text).width);
      }
    }
    if (!this.drawn || !this.graph) return;
    for (const level of this.drawn.levels) {
      const drawn = this.groups.get(level.path);
      if (!drawn || !drawn.baked || drawn.layout !== level.layout) continue;
      this.levelLabels(level, drawn);
    }
  }

  /** The labels of `level` (a scene entry) in its group `drawn`: built when what they say changed, each size shown or hidden by the zoom. */
  levelLabels(level, drawn) {
    const k = level.transform.k;
    let detail = labelDetail(k);
    // A box dragged: its level's pins are each drawn, so each is named.
    if (detail === 'groups' && !level.collapsed) detail = 'pins';
    const notes = detail === 'pins' && notesAt(k);
    const hovered = this.hoveredBox && parentPath(this.hoveredBox) === level.path ? this.hoveredBox : null;
    const counts = countsShown(k) ? level.bundles : null;
    const key = [detail, notes, level.collapsed, level.openKey, this.traceVersion, hovered, drawn.baked, level.layout, counts];
    if (!drawn.labelKey || key.some((v, i) => v !== drawn.labelKey[i])) {
      drawn.labelKey = key;
      drawn.labelBoxes = this.labelBoxes(level, { detail, notes, hovered });
      drawn.sidesKey = null;
    }
    // A frame's names kept in the drawing area (2026-10-22: at deep zoom they went past its left edge, by the sidebar): placed again only when a side turns.
    const corner = toLocal(level.transform, { x: 0, y: 0 });
    const far = toLocal(level.transform, { x: this.viewport.w, y: this.viewport.h });
    const viewport = { x: corner.x, y: corner.y, w: far.x - corner.x, h: far.y - corner.y };
    const sides = frameSides(drawn.labelBoxes.boxes, detail, viewport);
    if (sides !== drawn.sidesKey) {
      drawn.sidesKey = sides;
      this.buildLabels(level, drawn, { detail, counts, viewport });
    }
    for (const g of drawn.sizes) {
      const px = Number(g.dataset.size) * k;
      const display = px >= READ_PX && !(g.dataset.outward && px > OUTWARD_MAX_PX) ? '' : 'none';
      if (g.style.display !== display) g.style.display = display;
    }
  }

  /** The boxes whose pins `level`'s labels name (`placeLabels`'), and each one's pins' addresses: `{boxes, addresses}`. */
  labelBoxes(level, { detail, notes, hovered }) {
    const boxes = [];
    const addresses = [];
    for (const node of level.layout.nodes) {
      const frame = node.kind !== 'instance';
      const instance = `${level.path}/${node.name}`;
      if (frame ? !level.path : !node.composite) continue;
      // An open composite's box keeps its pins named, outside its edge (its inside drawn in it, its frame named there too).
      const open = !frame && this.drawn.open.has(instance);
      if (open && detail === 'summary') continue;
      const at = (p) => ({ name: p.name, x: p.x, y: p.y });
      const box = { kind: frame ? node.kind : 'instance', name: node.name, rect: { x: node.x, y: node.y, w: node.w, h: node.h }, ins: node.ins.map(at), outs: node.outs.map(at), texts: new Map(), header: HEADER, outward: open };
      // Its port groups (M2.13-2): a box's collapse with the level; a frame's (the same groups) are named as runs.
      if (node.groups) {
        box.groups = node.groups;
        box.collapsed = level.collapsed;
      } else if (frame) box.groups = this.frameGroups(level.level, node);
      const pinAddress = (name) => (frame ? `${level.path}.${name}` : `${instance}.${name}`);
      for (const p of [...node.ins, ...node.outs]) box.texts.set(p.name, notes && !open ? this.pinLabel(!frame, pinAddress(p.name), p.name, hovered === instance) : p.name);
      boxes.push(box);
      addresses.push(pinAddress);
    }
    return { boxes, addresses };
  }

  /** `level`'s labels (its boxes' as `labelBoxes` gave them, a frame's kept in `viewport`, the level's units) drawn in its group `drawn`. */
  buildLabels(level, drawn, { detail, counts, viewport }) {
    const baked = drawn.baked;
    const m = baked.m;
    const { boxes, addresses } = drawn.labelBoxes;
    const labels = levelLabels(boxes, detail, viewport);
    // A bundle's count (M2.13-2): along its curve where no other label is, else moved off it, never left out.
    if (counts) {
      const list = counts.filter((item) => item.kind === 'bundle').map((item) => ({ text: String(item.count), at: countPlaces(item) }));
      labels.push(...levelCounts(labels, list, COUNT_UNITS));
    }
    const pins = this.tracing?.result?.pins;
    const bySize = new Map();
    for (const label of labels) {
      const shown = label.shown ?? label.text;
      if (!shown) continue;
      const f = label.scale ?? 1;
      const size = LABEL_PX * f;
      const outward = Boolean(boxes[label.pins[0]?.box]?.outward);
      const group = `${size}${outward ? ' outward' : ''}`;
      let g = bySize.get(group);
      if (!g) {
        g = el('g', { 'data-size': size, ...(outward ? { 'data-outward': '1' } : {}) });
        bySize.set(group, g);
      }
      const one = label.pins.length === 1 ? label.pins[0] : null;
      const traced = Boolean(pins && label.pins.some((p) => pins.has(addresses[p.box](p.name))));
      const noted = Boolean(one && label.text !== one.name);
      const kind = label.count ? 'count' : `${label.align}${traced ? ' traced' : ''}${noted ? ' noted' : ''}`;
      if (label.count || label.align === 'centre' || traced || noted) {
        const r = bakedRect(baked, { x: label.x0, y: label.y0, w: label.x1 - label.x0, h: label.y1 - label.y0 });
        if (r) g.append(el('rect', { class: `lbg ${kind}`, ...r, rx: (label.count ? 6 : 4) * f * m }));
      }
      const pad = 3 * f;
      const x = label.align === 'left' ? label.x0 + pad : label.align === 'right' ? label.x1 - pad : (label.x0 + label.x1) / 2;
      const anchor = label.align === 'left' ? 'start' : label.align === 'right' ? 'end' : 'middle';
      shown.split('\n').forEach((line, i) => {
        const at = bakedPoint(baked, { x, y: label.y0 + PITCH_PX * f * (i + 0.5) + 0.34 * size });
        // A halo of the background's colour around its letters (as wide as the text is large), so a label reads over wires.
        const text = el('text', { class: `lbl ${kind}`, x: at.x, y: at.y, 'font-size': size * m, 'stroke-width': 0.3 * size * m, 'text-anchor': anchor }, line);
        if (shown !== label.text && i === 0) text.append(el('title', {}, label.text));
        g.append(text);
      });
    }
    drawn.sizes = [...bySize.values()];
    drawn.labels.replaceChildren(...drawn.sizes);
  }

  /** A frame's pins in its composite's groups (as its box outside, `portGroups`), per level layout. */
  frameGroups(module, node) {
    this.frameGroupsOf ??= new WeakMap();
    let groups = this.frameGroupsOf.get(node);
    if (!groups) {
      const facts = exportFacts(this.graph?.composites?.[module] ?? null);
      groups = { ins: portGroups(node.ins, facts), outs: portGroups(node.outs, facts) };
      this.frameGroupsOf.set(node, groups);
    }
    return groups;
  }

  /**
   * The links across the edge of the composite whose inside `level` is,
   * open in place (2026-10-20, `boundaryLinks`): from each of its box's
   * pins to its frame's pin of the same port, coloured as the port, traced
   * with it. None for the top level.
   */
  linkElements(level, baked) {
    const parent = level.parent;
    if (!parent) return [];
    const name = level.path.slice(level.path.lastIndexOf('/') + 1);
    const node = parent.layout.nodes.find((n) => n.kind === 'instance' && n.name === name);
    const anchors = parent.collapsed && node?.groups ? groupAnchors(node) : null;
    return boundaryLinks(node, level.layout, anchors).map((l) => {
      const dx = Math.max(Math.abs(l.to.x - l.from.x) / 2, 1);
      const c1 = { x: l.from.x + dx, y: l.from.y };
      const c2 = { x: l.to.x - dx, y: l.to.y };
      const points = [];
      for (let i = 0; i <= 16; i++) {
        const t = i / 16;
        const u = 1 - t;
        points.push({
          x: u * u * u * l.from.x + 3 * u * u * t * c1.x + 3 * u * t * t * c2.x + t * t * t * l.to.x,
          y: u * u * u * l.from.y + 3 * u * u * t * c1.y + 3 * u * t * t * c2.y + t * t * t * l.to.y,
        });
      }
      return el('path', { class: `wire link ${l.shape}`, d: bakedPath(baked, points), 'data-address': `${level.path}.${l.name}` });
    });
  }

  /** The sidebar's Trace section (above the selection's), or nothing without a trace. */
  traceParts() {
    const model = traceSection(this.tracing?.result ?? null, this.graph);
    if (!model) return [];
    const box = html('section', { className: 'trace' });
    box.setAttribute('aria-label', 'Trace');
    const head = html('div', { className: 'trace-head' });
    const end = html('button', { type: 'button', textContent: '×', title: 'End the trace (Escape)' });
    end.addEventListener('click', () => {
      this.focus({ preventScroll: true });
      this.endTrace();
    });
    head.append(html('h3', { textContent: `Trace: ${model.title}`, title: model.title }), end);
    const toggle = html('div', { className: 'toggle' });
    toggle.setAttribute('role', 'group');
    toggle.setAttribute('aria-label', 'Direction');
    for (const d of model.directions) {
      const button = html('button', { type: 'button', textContent: d.label, title: d.title });
      button.setAttribute('aria-pressed', String(d.on));
      button.addEventListener('click', () => {
        this.focus({ preventScroll: true });
        this.traceDirection(d.id);
      });
      toggle.append(button);
    }
    const depths = html('div', { className: 'toggle' });
    depths.setAttribute('role', 'group');
    depths.setAttribute('aria-label', 'Depth');
    for (const d of model.depths) {
      const button = html('button', { type: 'button', textContent: d.label, title: d.title });
      button.setAttribute('aria-pressed', String(d.on));
      button.addEventListener('click', () => {
        this.focus({ preventScroll: true });
        this.traceDepth(d.id);
      });
      depths.append(button);
    }
    box.append(head, html('div', { className: 'subtitle', textContent: model.subtitle }), toggle, depths);
    for (const note of model.notes) box.append(html('div', { className: 'note', textContent: note }));
    for (const side of model.sides) {
      box.append(html('h4', { textContent: `${side.title} (${side.counted ?? side.count})` }));
      if (!side.endpoints.length) box.append(html('div', { className: 'subtitle', textContent: side.empty }));
      for (const e of side.endpoints) {
        const row = html('div', { className: e.open ? 'end open' : 'end' });
        row.append(this.revealLink(e.label, e.reveal), html('span', { className: 'what', textContent: e.note }));
        box.append(row);
      }
    }
    box.append(html('h4', { textContent: 'By level' }));
    for (const level of model.levels) {
      // A long trace's levels start collapsed, but the start's (M2.13-2).
      const details = html('details', { open: !level.collapsed });
      const summary = html('summary');
      summary.append(this.revealLink(level.label, level.reveal), html('span', { className: 'count', textContent: level.count }));
      details.append(summary);
      const list = html('ul', { className: 'tree' });
      for (const inst of level.children) {
        const item = html('li');
        item.append(this.revealLink(inst.label, inst.reveal), html('span', { className: 'count', textContent: inst.note ?? '' }));
        const pins = html('ul', { className: 'tree' });
        for (const pin of inst.children) {
          const li = html('li');
          // The signal's path (2026-10-21): it leaves an output `label →` and enters an input `→ label`.
          const link = this.revealLink(pin.direction === 'out' ? `${pin.label} →` : `→ ${pin.label}`, pin.reveal);
          link.classList.add(`role-${pin.role}`);
          li.append(link);
          pins.append(li);
        }
        item.append(pins);
        list.append(item);
      }
      details.append(list);
      box.append(details);
    }
    return [box];
  }

  /** A link that reveals `target` (a Trace section node's `reveal`). */
  revealLink(label, target) {
    const button = html('button', { type: 'button', className: 'link', textContent: label, title: `Show ${label}` });
    button.addEventListener('click', () => {
      this.focus({ preventScroll: true });
      this.reveal(target);
    });
    return button;
  }

  /**
   * `target` (`{kind: 'level' | 'instance' | 'pin', path | address}`) shown:
   * the composites on its way opened (as a double-click opens them), the
   * view in its level's coordinates around it, and an instance or a pin
   * selected. A pin of the composite whose inside is shown is shown as its
   * frame's pin there; any other at the level of its box.
   */
  reveal(target) {
    if (!this.graph) return;
    const shown = this.drawn ? breadcrumbs(this.drawn).at(-1).path : '';
    let path;
    let pick = null;
    if (target.kind === 'level') path = target.path;
    else if (target.kind === 'instance') {
      path = parentPath(target.address);
      pick = { name: target.address.slice(target.address.lastIndexOf('/') + 1) };
    } else {
      const at = splitAddress(target.address);
      if (!at) return;
      // `side` (Go inside, Go outside): a composite's port as its frame's pin inside it, or as its box's pin around it.
      const inside = target.side ? target.side === 'frame' : at.instance === shown && shown !== '';
      // Going outside closes what a double-click or a reveal held open there, so its box's pins show.
      if (target.side === 'box') this.pinned = unpin(this.pinned, at.instance);
      path = inside ? at.instance : parentPath(at.instance);
      pick = inside ? { frame: true, port: at.port } : { name: at.instance.slice(at.instance.lastIndexOf('/') + 1), port: at.port };
    }
    const links = chain((level) => this.layoutOf(level), path);
    if (!links) {
      this.say(`Cannot show ${target.address ?? target.path}: its level is not drawn`);
      return;
    }
    const entry = links.at(-1);
    // Every composite on the way stays open (Escape or a breadcrumb closes
    // them); those an earlier reveal opened off this way close.
    const way = new Set(links.slice(1).map((link) => link.path));
    for (const path of this.revealPinned ?? []) if (!way.has(path)) this.pinned.delete(path);
    this.revealPinned = new Set([...way].filter((path) => !this.pinned.has(path) || this.revealPinned?.has(path)));
    for (const path of way) {
      if (!this.pinned.has(path)) this.openShifts.delete(path);
      this.pinned = pinOne(this.pinned, path);
    }
    let rect = entry.layout.bounds;
    let hit = null;
    if (pick) {
      const node = entry.layout.nodes.find((n) => (pick.frame ? n.kind !== 'instance' && [...n.ins, ...n.outs].some((p) => p.name === pick.port) : n.kind === 'instance' && n.name === pick.name));
      if (node) {
        const pin = pick.port ? [...node.ins, ...node.outs].find((p) => p.name === pick.port) : null;
        hit = pin ? portHit(entry, node, pin) : pick.port ? null : instanceHit(entry, node);
        // Around the pin, or the box, at a readable size (pins about 20 px
        // apart, so their badges read): a few boxes' room on each side.
        const w = Math.max(node.w * 5, 800);
        const h = pin ? 440 : Math.max(node.h * 1.3, 440);
        const c = pin ?? { x: node.x + node.w / 2, y: node.y + node.h / 2 };
        rect = { x: c.x - w / 2, y: c.y - h / 2, w, h };
      }
    }
    const view = fit(rect, this.viewport, MARGIN_PX);
    this.view = { ...view, path };
    this.keepSideScroll = true;
    if (hit) this.select([hit]);
    else this.schedule();
  }

  // Level bars.

  updateLevels() {
    clearTimeout(this.retryTimer);
    this.retryTimer = null;
    if (!this.connection) return;
    const all = this.drawn && this.graph ? candidates(this.drawn, this.graph, this.metered, this.viewport) : [];
    const wanted = all.filter((c) => c.source === 'scope');
    for (const c of wanted) this.levelRanges.set(c.port, c.range);
    this.updateParameters(all.filter((c) => c.source === 'parameter'));
    const plan = levelPlan({
      wanted,
      granted: new Set(this.levelSubs.keys()),
      refused: this.refused,
      active: this.active,
      now: performance.now(),
      slots: this.connection.description?.scopes?.slots ?? SLOTS,
      scopes: this.scopes,
    });
    for (const port of plan.unsubscribe) this.dropLevel(port);
    for (const port of plan.subscribe) this.takeLevel(port);
    this.states = new Map(wanted.map((c) => [c.address, { state: plan.states.get(c.address), port: c.port }]));
    for (const c of all) {
      if (c.source !== 'parameter') continue;
      const live = this.active && this.paramStream?.subscribed;
      this.states.set(c.address, { state: live ? 'live' : this.active ? 'pending' : 'idle', port: `param ${c.port}` });
    }
    this.applyBars();
    // Only while a request could succeed; otherwise the next `scopes` says.
    if (this.active && plan.retry !== null) this.retryTimer = setTimeout(() => this.updateLevels(), plan.retry);
  }

  takeLevel(port) {
    this.refused.delete(port);
    const stop = this.connection.subscribe(
      { stream: 'scope', port, rate: LEVEL_RATE },
      (frame) => {
        const range = this.levelRanges.get(port) ?? null;
        this.levels.set(port, { ...levelSpan(frame.values, range), text: scopeText(frame.values, range) });
        this.scheduleBars();
      },
      {
        onSubscribed: () => this.markLive(port),
        onError: (error) => {
          this.dropLevel(port);
          this.refused.set(port, performance.now());
          if (error.code === 'no-scope-slot' || error.code === 'busy') this.updateLevels();
        },
      },
    );
    this.levelSubs.set(port, stop);
  }

  markLive(port) {
    for (const entry of this.states?.values() ?? []) if (entry.port === port) entry.state = 'live';
    this.applyBars();
  }

  dropLevel(port) {
    this.levelSubs.get(port)?.();
    this.levelSubs.delete(port);
    this.levels.delete(port);
  }

  /**
   * The selected instance's parameters, as one `parameters` stream (no
   * scope slot): its effective values, each scaled to its range. A new
   * set of addresses starts it again; none, or inactive, stops it.
   */
  updateParameters(wanted) {
    const addresses = this.active ? [...new Set(wanted.map((c) => c.port))].slice(0, PARAMETER_STREAM_MAX) : [];
    const key = addresses.join('\t');
    if (this.paramStream?.key === key) return;
    this.paramStream?.stop?.();
    for (const port of this.paramStream?.addresses ?? []) this.levels.delete(`param ${port}`);
    this.paramStream = null;
    if (!addresses.length || !this.connection) return;
    const ranges = new Map(wanted.map((c) => [c.port, c.range]));
    const stream = { key, addresses, subscribed: false, stop: null };
    stream.stop = this.connection.subscribe(
      { stream: 'parameters', addresses, rate: 30 },
      (frame) => {
        for (const [i, port] of addresses.entries()) {
          const v = frame?.values?.[i];
          if (!v) continue;
          this.levels.set(`param ${port}`, { ...parameterSpan(v.effective, ranges.get(port) ?? null), text: valueText(v.effective) });
        }
        this.scheduleBars();
      },
      {
        onSubscribed: () => {
          stream.subscribed = true;
          for (const entry of this.states?.values() ?? []) if (entry.port.startsWith('param ')) entry.state = 'live';
          this.applyBars();
        },
        onError: () => {
          if (this.paramStream === stream) this.paramStream = null;
        },
      },
    );
    this.paramStream = stream;
  }

  releaseLevels() {
    clearTimeout(this.retryTimer);
    clearTimeout(this.levelTimer);
    for (const port of [...this.levelSubs.keys()]) this.dropLevel(port);
    this.paramStream?.stop?.();
    this.paramStream = null;
  }

  scheduleBars() {
    if (this.barsPending || !this.active) return;
    this.barsPending = true;
    requestAnimationFrame(() => {
      this.barsPending = false;
      this.applyBars();
    });
  }

  applyBars() {
    for (const [address, shown] of this.bars) {
      const entry = this.states?.get(address);
      const state = entry?.state ?? 'idle';
      const span = state === 'live' ? this.levels.get(entry.port) ?? NO_SPAN : NO_SPAN;
      const level = `${span.lo} ${span.hi}`;
      // Written only when changed: a pan frame touches no bar. A span at one value is a 1 px tick.
      if (shown.state !== state) shown.bar.setAttribute('data-state', state);
      if (shown.level !== level) {
        const width = Math.max(span.hi > span.lo || span.hi > 0 ? 1 : 0, shown.width * (span.hi - span.lo));
        shown.fill.setAttribute('x', String(shown.x + shown.width * span.lo));
        shown.fill.setAttribute('width', String(width));
      }
      shown.state = state;
      shown.level = level;
    }
    // A selected pin's level in the sidebar: its bar's, while one shows.
    const meter = this.sideMeter;
    if (meter) {
      const entry = this.states?.get(meter.address);
      const state = entry ? entry.state ?? 'idle' : 'off';
      const span = state === 'live' ? this.levels.get(entry.port) ?? NO_SPAN : NO_SPAN;
      const text = { live: span.text ?? '…', idle: 'stopped', pending: '…', 'no-slot': 'no slot', off: 'not on screen' }[state] ?? state;
      const key = `${state} ${span.lo} ${span.hi} ${text}`;
      if (meter.shown !== key) {
        meter.fill.style.transformOrigin = 'left';
        meter.fill.style.transform = `translateX(${100 * span.lo}%) scaleX(${span.hi - span.lo})`;
        meter.state.textContent = text;
        meter.shown = key;
      }
    }
  }
}

/** A meter with nothing to show. */
const NO_SPAN = Object.freeze({ lo: 0, hi: 0, text: null });

/** The parameters one schematic stream follows at most (the host's per-callback observation limit). */
const PARAMETER_STREAM_MAX = 64;

/** A scope block as the sidebar says it: its peak in dBFS without a range, else its lowest and highest value. */
function scopeText(points, range) {
  let lo = Infinity;
  let hi = -Infinity;
  let peak = 0;
  for (const v of points ?? []) {
    if (!Number.isFinite(v)) continue;
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
    peak = Math.max(peak, Math.abs(v));
  }
  if (!range) return peak > 0 ? `${(20 * Math.log10(peak)).toFixed(1)} dB` : '−∞ dB';
  return lo > hi ? '—' : lo === hi ? valueText(lo) : `${valueText(lo)} … ${valueText(hi)}`;
}

/** A value, 4 significant digits. */
function valueText(v) {
  return Number.isFinite(v) ? String(Number(v.toPrecision(4))) : '—';
}

customElements.get('signal-schematic') ?? customElements.define('signal-schematic', SignalSchematic);
