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
import './lib/offline.test.js';
import './lib/panel.test.js';
import './signal-ui.test.js';
import './fake-connection.test.js';
