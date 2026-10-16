// Test entry for `node --test ui/` only: Node runs a directory argument as
// a module, which resolves to this file. It is NOT the UI library's entry
// (the library is ui/signal-ui.js, ui/controls/ and ui/lib/), and no page
// loads it. Every `*.test.js` under ui/ must be imported here;
// crates/signal-ui/tests/js.rs checks that.
import './lib/protocol.test.js';
import './lib/taper.test.js';
import './lib/gesture.test.js';
import './lib/ballistics.test.js';
import './lib/trigger.test.js';
import './lib/skin.test.js';
import './lib/format.test.js';
import './lib/entry.test.js';
import './lib/offline.test.js';
import './lib/panel.test.js';
import './lib/binding.test.js';
import './lib/unsaved.test.js';
import './lib/canvas.test.js';
import './signal-ui.test.js';
import './fake-connection.test.js';
import './app/lib/menu.test.js';
import './app/lib/draft.test.js';
import './app/lib/parameters.test.js';
import './app/lib/dom.test.js';
import './app/lib/tabs.test.js';
import './app/lib/power.test.js';
import './app/lib/status.test.js';
import './app/lib/save.test.js';
import './app/lib/document.test.js';
import './app/lib/layout.test.js';
import './app/lib/context.test.js';
import './lib/cards.test.js';
import './controls/panel.test.js';
import './controls/entry-field.test.js';
import './lib/schematic-view.test.js';
import './lib/schematic-layout.test.js';
import './lib/schematic-scene.test.js';
import './lib/schematic-inspector.test.js';
import './lib/schematic-levels.test.js';
