// Test entry for `node --test ui/` only: Node runs a directory argument as
// a module, which resolves to this file. It is NOT the UI library's entry
// (the library is ui/lib/), and no page loads it. Every `*.test.js` under
// ui/ must be imported here; crates/signal-ui/tests/js.rs checks that.
import './lib/protocol.test.js';
