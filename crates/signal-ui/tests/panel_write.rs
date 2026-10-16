//! M2.9-2 (ADR 0026 §5, `docs/ui-protocol.md` "`panel.read`, `panel.write`"):
//! the panel editor's writes over both transports, on copies of the
//! committed projects (the example panels are never written): a write and
//! its push, a refused element or attribute, a conflict, a refused binding,
//! the text outside `<signal-panel>` kept byte for byte, the generated
//! panel's first write, and only the served project's `ui/`. (A `.signal`
//! project marked edited: `app_mode.rs`.)

mod common;

use std::fs;

use common::{Options, Rig, Transport, Ws, rig, root};
use serde_json::{Value, json};
use signal_ui::panel_write::sha256_hex;

fn kick(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            project: Some(root().join("projects/kick")),
            ..Options::default()
        },
    )
}

/// Kick's panel rearranged, as the editor writes it: the meter pair first.
const EDITED: &str = r#"
  <div class="signal-cards">
    <section class="signal-card">
      <h1>kick</h1>
      <p>edited</p>
    </section>
    <section class="signal-card wide">
      <h2>output</h2>
      <div class="row">
        <div class="signal-group">
          <signal-meter bind="/out.l" label="L"></signal-meter>
          <signal-meter bind="/out.r" label="R"></signal-meter>
        </div>
        <signal-fader bind="/out.gain" taper="db" label="gain"></signal-fader>
        <signal-knob bind="/drums.kick_decay" taper="log" min="100" max="1500" label="decay" skin="svg:assets/knob.svg"></signal-knob>
      </div>
    </section>
  </div>
"#;

/// The error reply to `id`.
fn refused(rig: &mut Rig, ws: &mut Ws, id: u64, message: Value) -> Value {
    rig.request(ws, id, message, "error")
}

fn read(rig: &mut Rig, ws: &mut Ws, id: u64, file: &str) -> Value {
    rig.request(
        ws,
        id,
        json!({"type": "panel.read", "file": file}),
        "panel.read",
    )
}

fn write(file: &str, html: &str, base: &Value) -> Value {
    json!({"type": "panel.write", "file": file, "html": html, "base": base})
}

/// A write replaces the content and nothing else, its reply carries the new
/// base, every other client gets the panel, and the watch takes it as seen.
fn a_write_replaces_the_panel_and_is_pushed(transport: Transport) {
    let mut rig = kick("panel-write", transport);
    let (mut ws, _) = rig.client();
    let (mut other, _) = rig.client();
    let file = rig.dir.join("ui/main.html");
    let before = fs::read_to_string(&file).unwrap();
    let read = read(&mut rig, &mut ws, 1, "main.html");
    assert_eq!(read["exists"], true);
    assert_eq!(read["editable"], true);
    assert_eq!(read["planned"], true);
    assert_eq!(read["base"], sha256_hex(before.as_bytes()));
    assert!(
        read["html"]
            .as_str()
            .unwrap()
            .contains("<signal-knob bind=\"/drums.kick_decay\"")
    );

    let written = rig.request(
        &mut ws,
        2,
        write("main.html", EDITED, &read["base"]),
        "panel.written",
    );
    let after = fs::read_to_string(&file).unwrap();
    assert_eq!(written["base"], sha256_hex(after.as_bytes()));
    assert_eq!(written["created"], false);
    let start = before.find("<signal-panel>").unwrap() + "<signal-panel>".len();
    let end = before.find("</signal-panel>").unwrap();
    assert_eq!(
        after,
        format!("{}{EDITED}{}", &before[..start], &before[end..])
    );
    let leftovers: Vec<_> = fs::read_dir(rig.dir.join("ui"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "atomic: no temporary file left");

    let pushed = rig.wait(&mut other, "panel.written", |got| {
        got.of("panel.written").cloned()
    });
    assert_eq!(pushed["file"], "main.html");
    assert_eq!(pushed["base"], written["base"]);
    assert_eq!(pushed["html"], EDITED);
    // The written panel checks as panel.check checks it.
    let check = rig.request(
        &mut ws,
        3,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(check["controls"], 4);
    rig.no_reload(&mut ws, 300);
    rig.finish();
}

/// Never trusting the page: what is outside the whitelist is refused and
/// nothing is written.
fn a_refused_element_or_attribute_writes_nothing(transport: Transport) {
    let mut rig = kick("panel-write-refused", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("ui/main.html");
    let before = fs::read_to_string(&file).unwrap();
    let base = read(&mut rig, &mut ws, 1, "main.html")["base"].clone();
    for (id, (html, why)) in [
        (
            EDITED.replace("<p>edited</p>", "<script>alert(1)</script>"),
            "<script>",
        ),
        (
            EDITED.replace("<signal-fader bind", "<signal-fader onclick=\"x()\" bind"),
            "event handlers",
        ),
        (
            EDITED.replace("<h2>output</h2>", "<h2 style=\"color: red\">output</h2>"),
            "style",
        ),
        (
            EDITED.replace("<p>edited</p>", "<img src=\"x.png\">"),
            "<img>",
        ),
        (
            EDITED.replace("label=\"gain\"", "label=\"gain\" foo=\"1\""),
            "foo",
        ),
        (
            EDITED.replace("<p>edited</p>", "<!-- a comment -->"),
            "comment",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let error = refused(
            &mut rig,
            &mut ws,
            10 + id as u64,
            write("main.html", &html, &base),
        );
        assert_eq!(error["code"], "bad-html", "{error}");
        assert!(error["message"].as_str().unwrap().contains(why), "{error}");
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), before);
    rig.finish();
}

/// A file changed since the editor read it is refused with its text, so
/// the page can read it again.
fn a_changed_file_is_a_conflict(transport: Transport) {
    let mut rig = kick("panel-write-conflict", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("ui/main.html");
    let base = read(&mut rig, &mut ws, 1, "main.html")["base"].clone();
    let changed = fs::read_to_string(&file)
        .unwrap()
        .replace("<p>A techno", "<p>By hand: a techno");
    fs::write(&file, &changed).unwrap();
    let error = refused(&mut rig, &mut ws, 2, write("main.html", EDITED, &base));
    assert_eq!(error["code"], "conflict", "{error}");
    assert_eq!(error["text"], changed);
    assert_eq!(error["base"], sha256_hex(changed.as_bytes()));
    assert!(error["html"].as_str().unwrap().contains("By hand"));
    assert_eq!(fs::read_to_string(&file).unwrap(), changed);
    // A null base is the missing file's: a conflict too while it exists.
    let error = refused(
        &mut rig,
        &mut ws,
        3,
        write("main.html", EDITED, &Value::Null),
    );
    assert_eq!(error["code"], "conflict");
    // With the current base it is written.
    rig.request(
        &mut ws,
        4,
        write("main.html", EDITED, &error["base"]),
        "panel.written",
    );
    rig.finish();
}

/// The binding check refuses what `panel.check` refuses, and nothing is written.
fn a_refused_binding_writes_nothing(transport: Transport) {
    let mut rig = kick("panel-write-binding", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("ui/main.html");
    let before = fs::read_to_string(&file).unwrap();
    let base = read(&mut rig, &mut ws, 1, "main.html")["base"].clone();
    let html = EDITED.replace(
        "<signal-fader bind=\"/out.gain\" taper=\"db\" label=\"gain\"></signal-fader>",
        "<signal-scope bind=\"/out.gain\"></signal-scope>",
    );
    let error = refused(&mut rig, &mut ws, 2, write("main.html", &html, &base));
    assert_eq!(error["code"], "bad-binding", "{error}");
    assert!(
        error["message"].as_str().unwrap().contains("wrong kind"),
        "{error}"
    );
    assert_eq!(error["diagnostics"][0]["severity"], "error");
    assert_eq!(fs::read_to_string(&file).unwrap(), before);
    rig.finish();
}

/// Everything outside `<signal-panel>` stays byte for byte, a
/// `</signal-panel>` in a comment or script inside it included (the end is
/// found as the start tags are, auditor-2-end-tag-scan); what was inside
/// (its comments, its script) is replaced.
fn the_text_outside_the_panel_is_kept_byte_for_byte(transport: Transport) {
    let mut rig = kick("panel-write-outside", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("ui/main.html");
    let original = fs::read_to_string(&file).unwrap();
    let start = original.find("<signal-panel>").unwrap() + "<signal-panel>".len();
    let end = original.find("</signal-panel>").unwrap();
    let head = format!(
        "{}<!-- a comment before -->\r\n",
        &original[..start - "<signal-panel>".len()]
    );
    let tail = "</signal-panel>\n<!-- after: </signal-panel> -->\n<script>let s = \"</signal-panel>\";</script>\n<style>p { color: red }</style>\n</body>\n</html>\n";
    let inside = format!(
        "{}<!-- in it: </signal-panel> --><script>let t = \"</signal-panel>\";</script>\n",
        &original[start..end]
    );
    let text = format!("{head}<signal-panel>{inside}{tail}");
    fs::write(&file, &text).unwrap();
    let read = read(&mut rig, &mut ws, 1, "main.html");
    assert!(
        read["html"].as_str().unwrap().ends_with("</script>\n"),
        "{read}"
    );
    rig.request(
        &mut ws,
        2,
        write("main.html", EDITED, &read["base"]),
        "panel.written",
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{head}<signal-panel>{EDITED}{tail}")
    );
    rig.finish();
}

/// A file with no one panel element is not editable, and says why.
fn a_file_without_one_panel_is_not_editable(transport: Transport) {
    let mut rig = kick("panel-write-not-editable", transport);
    let (mut ws, _) = rig.client();
    let ui = rig.dir.join("ui");
    let none = "<!doctype html><p>no panel</p>";
    let two = "<signal-panel></signal-panel>\n<signal-panel></signal-panel>\n";
    fs::write(ui.join("none.html"), none).unwrap();
    fs::write(ui.join("two.html"), two).unwrap();
    for (id, (name, text, why)) in [
        ("none.html", none, "no <signal-panel>"),
        ("two.html", two, "2 <signal-panel> elements"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = 10 * (id as u64 + 1);
        let read = read(&mut rig, &mut ws, id, name);
        assert_eq!(read["editable"], false);
        assert!(read["reason"].as_str().unwrap().contains(why), "{read}");
        let error = refused(
            &mut rig,
            &mut ws,
            id + 1,
            write(name, EDITED, &read["base"]),
        );
        assert_eq!(error["code"], "not-editable", "{error}");
        assert!(error["message"].as_str().unwrap().contains(why), "{error}");
        assert_eq!(fs::read_to_string(ui.join(name)).unwrap(), text);
    }
    rig.finish();
}

/// A project without `ui/main.html` (the generated panel): the first write
/// creates it with the standard head and the edited panel.
fn the_generated_panel_s_first_write_creates_main_html(transport: Transport) {
    let mut rig = rig(
        "panel-write-first",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("ui/main.html");
    assert!(!file.exists());
    let read = read(&mut rig, &mut ws, 1, "main.html");
    assert_eq!(read["exists"], false);
    assert_eq!(read["base"], Value::Null);
    assert_eq!(read["editable"], true);
    // The generated panel's layout of the tone project, as the editor writes it.
    let html = r#"
  <div class="signal-cards">
    <section class="signal-card">
      <h2>a</h2>
      <p>std.sine_osc</p>
      <div class="row">
        <signal-knob bind="/a.freq" label="freq" taper="log"></signal-knob>
      </div>
    </section>
    <section class="signal-card wide">
      <h2>output</h2>
      <div class="row">
        <div class="signal-group">
          <signal-meter bind="/out.l" label="l"></signal-meter>
          <signal-meter bind="/out.r" label="r"></signal-meter>
        </div>
        <signal-fader bind="/out.gain" taper="db" label="gain"></signal-fader>
      </div>
    </section>
  </div>
"#;
    let written = rig.request(
        &mut ws,
        2,
        write("main.html", html, &Value::Null),
        "panel.written",
    );
    assert_eq!(written["created"], true);
    let text = fs::read_to_string(&file).unwrap();
    assert_eq!(written["base"], sha256_hex(text.as_bytes()));
    assert!(text.starts_with("<!doctype html>"));
    // The title is the project's name as the app shows it (the folder's).
    assert!(text.contains("<title>project</title>"), "{text}");
    assert!(text.contains("<link rel=\"stylesheet\" href=\"/ui/controls/theme.css\">"));
    assert!(text.contains(&format!("<signal-panel>{html}</signal-panel>")));
    assert!(text.contains("import '/ui/controls/index.js';"));
    let check = rig.request(
        &mut ws,
        3,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(check["ok"], true, "{check}");
    // Now it exists: a second null base is a conflict.
    let error = refused(&mut rig, &mut ws, 4, write("main.html", html, &Value::Null));
    assert_eq!(error["code"], "conflict");
    rig.finish();
}

/// Only an `.html` file in the served project's `ui/` (existing security
/// rule): no other folder, no hidden file, no other name for a new file.
fn only_the_served_project_s_panels_are_written(transport: Transport) {
    let mut rig = kick("panel-write-paths", transport);
    let (mut ws, _) = rig.client();
    let project = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    for (id, name) in [
        "../project.toml",
        "../main.html",
        "a/b.html",
        ".x.html",
        "main.txt",
        "new.html",
        "/etc/x.html",
        "",
    ]
    .into_iter()
    .enumerate()
    {
        let error = refused(
            &mut rig,
            &mut ws,
            10 + id as u64,
            write(name, EDITED, &Value::Null),
        );
        assert_eq!(error["code"], "unknown-panel", "{name}: {error}");
    }
    assert_eq!(
        fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
        project
    );
    assert!(!rig.dir.join("main.html").exists() && !rig.dir.join("ui/new.html").exists());
    rig.finish();
}

/// auditor-1-003 (AC15): a panel reached through a link (a linked `ui/`, or
/// a linked file in it) resolves outside the project: it is not written,
/// and the file outside is unchanged.
fn a_panel_reached_through_a_link_is_not_written(transport: Transport) {
    let mut rig = kick("panel-write-link", transport);
    let (mut ws, _) = rig.client();
    let outside = rig.dir.parent().unwrap().join("outside-ui");
    fs::rename(rig.dir.join("ui"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, rig.dir.join("ui")).unwrap();
    let file = outside.join("main.html");
    let before = fs::read_to_string(&file).unwrap();
    let base = json!(sha256_hex(before.as_bytes()));
    let error = refused(&mut rig, &mut ws, 1, write("main.html", EDITED, &base));
    assert_eq!(error["code"], "unknown-panel", "{error}");
    assert!(
        error["message"].as_str().unwrap().contains("link"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), before);
    // A linked file in a real ui/ folder.
    fs::remove_file(rig.dir.join("ui")).unwrap();
    fs::create_dir(rig.dir.join("ui")).unwrap();
    std::os::unix::fs::symlink(&file, rig.dir.join("ui/linked.html")).unwrap();
    let error = refused(&mut rig, &mut ws, 2, write("linked.html", EDITED, &base));
    assert_eq!(error["code"], "unknown-panel", "{error}");
    assert_eq!(fs::read_to_string(&file).unwrap(), before);
    rig.finish();
}

both!(
    a_write_replaces_the_panel_and_is_pushed,
    a_refused_element_or_attribute_writes_nothing,
    a_changed_file_is_a_conflict,
    a_refused_binding_writes_nothing,
    the_text_outside_the_panel_is_kept_byte_for_byte,
    a_file_without_one_panel_is_not_editable,
    the_generated_panel_s_first_write_creates_main_html,
    only_the_served_project_s_panels_are_written,
    a_panel_reached_through_a_link_is_not_written,
);
