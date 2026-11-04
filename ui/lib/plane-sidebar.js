// The Deck's sidebar (the owner's, 2026-11-04): what is selected on the
// plane, at any layer and depth, and its whole net (ui/lib/plane-net.js):
// where its signal comes from, where it goes, the chips' pins it passes
// through, each a link that selects that port. Shown while something is
// selected; Escape lets the selection, and the sidebar, go. No DOM.

import { netRoles } from './plane-net.js';

/** Most ports a net's list shows; the rest are counted. */
export const NET_SHOWN = 60;

/**
 * The sidebar for `selection` (ui/controls/plane.js `selectionOf`'s) with
 * its `net` (`selectionNet`'s, or null) over `graph`: `{title, subtitle,
 * rows: [{label, value}], sections: [{title, items: [{label, address}],
 * more}], copy}` (`copy` the addresses Copy writes), or null when nothing
 * is selected.
 */
export function planeSidebar(selection, { graph = null, net = null } = {}) {
  if (!selection) return null;
  const head = headOf(selection, graph);
  const sections = [];
  if (net) {
    const roles = netRoles(graph, net);
    const section = (title, list) => {
      if (!list.length) return;
      sections.push({ title, items: list.slice(0, NET_SHOWN).map((address) => ({ label: address, address })), more: Math.max(0, list.length - NET_SHOWN) });
    };
    section('Comes from', roles.sources);
    section('Goes to', roles.targets);
    section('Through the chips’ pins', roles.through);
    head.rows.push({ label: 'Net', value: `${net.addresses.size} port${net.addresses.size === 1 ? '' : 's'}, ${net.wires.size} wire${net.wires.size === 1 ? '' : 's'}` });
  }
  return { ...head, sections, copy: net ? [...net.addresses].sort() : head.copy };
}

/** The title, subtitle and rows of what is selected. */
function headOf(selection, graph) {
  const item = selection.item ?? {};
  const connection = (from, to, what) => ({ title: what, subtitle: `${from} → ${to}`, rows: [{ label: 'From', value: from }, { label: 'To', value: to }], copy: [from, to] });
  const port = (address, what, rows = []) => ({ title: address, subtitle: what, rows, copy: [address] });
  if (selection.kind === 'engine') {
    if (item.kind === 'e-wire' || item.kind === 'e-edge') return connection(item.from, item.to, item.edge ? 'Connection through the edge' : 'Connection');
    if (item.kind === 'e-bind') return port(item.address, 'Control signal into a parameter', [{ label: 'Set by', value: 'its control at the Control layer' }]);
    if (item.kind === 'e-pin') {
      const what = item.parameter ? 'Parameter' : item.side === 'out' ? 'Output' : 'Input';
      return port(item.address, `${what} of ${item.card}${item.module ? ` (${item.module})` : ''}`);
    }
    if (item.kind === 'e-card') {
      const count = item.composite ? (graph?.composites?.[item.module]?.instances?.length ?? 0) : 0;
      return { title: item.address, subtitle: `${item.module}${item.composite ? `, a chip of ${count} instance${count === 1 ? '' : 's'}` : ''}`, rows: [], copy: [item.address] };
    }
  }
  if (selection.kind === 'cable') return connection(selection.from, selection.to, 'Cable');
  if (selection.kind === 'auto') return selection.to ? connection(selection.from, selection.to, 'Automation') : port(selection.from, 'Automation source');
  if (selection.kind === 'leg') return port(selection.address, 'Control leg: the parameter its control sets');
  if (selection.kind === 'port') return port(selection.address, 'Port of the instrument');
  if (selection.kind === 'address') return port(selection.address, 'Port');
  if (selection.kind === 'instrument') return { title: selection.entry, subtitle: 'Instrument', rows: [], copy: [] };
  return { title: selection.kind, subtitle: '', rows: [], copy: [] };
}
