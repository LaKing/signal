//! ADR 0046: a part's names, its relative addresses, the panel tree read
//! and written as the page writes it, Group into slot's move of the
//! studio's controls, card ids and `ui/windows.toml`.

use super::*;
use std::path::PathBuf;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn studio_content() -> String {
    let text = std::fs::read_to_string(repository().join("projects/studio/ui/main.html")).unwrap();
    let (start, end) = panel_content(&text).unwrap();
    text[start..end].to_owned()
}

#[test]
fn a_parts_names() {
    assert_eq!(key_of("parts/synth.html"), Some("synth"));
    assert_eq!(key_of("parts/rack.fx.html"), Some("rack.fx"));
    for bad in [
        "main.html",
        "parts/Synth.html",
        "parts/a.b.c.html",
        "parts/.html",
        "parts/x/y.html",
    ] {
        assert_eq!(key_of(bad), None, "{bad}");
    }
    assert_eq!(marker("rack.fx"), "rack/fx");
    assert_eq!(marker("synth"), "synth");
    assert_eq!(slot_of("synth"), "/synth");
    assert_eq!(slot_of("rack.fx"), "rack/fx");
    assert_eq!(file_of("rack.fx"), "parts/rack.fx.html");
    let text = part_text("rack.fx", "\n");
    assert_eq!(marker_in(&text).map(|m| m.0), Some("rack/fx".to_owned()));
}

#[test]
fn relative_addresses_are_rooted_and_nothing_outside_the_slot() {
    assert_eq!(root_address(".cutoff", "/synth").unwrap(), "/synth.cutoff");
    assert_eq!(
        root_address("/vcf.cutoff", "/synth").unwrap(),
        "/synth/vcf.cutoff"
    );
    assert_eq!(
        root_address("/a/b.out", "/rack1/fx").unwrap(),
        "/rack1/fx/a/b.out"
    );
    for outside in [
        "echo.in",
        "../echo.in",
        "/..",
        "/vcf",
        ".Cut",
        ".a.b",
        "",
        "/a/.b",
    ] {
        let error = root_address(outside, "/synth").unwrap_err();
        assert!(
            error.contains("bind it from the host's panel"),
            "{outside}: {error}"
        );
    }
    assert_eq!(
        relative_address("/synth.cutoff", "/synth").as_deref(),
        Some(".cutoff")
    );
    assert_eq!(
        relative_address("/synth/vcf.cutoff", "/synth").as_deref(),
        Some("/vcf.cutoff")
    );
    assert_eq!(relative_address("/synthx.cutoff", "/synth"), None);
    assert_eq!(relative_address("/synth", "/synth"), None);
    assert_eq!(relative_address("/echo.in", "/synth"), None);
}

#[test]
fn the_tree_is_read_and_written_as_the_page_writes_it() {
    let content = studio_content();
    let tree = parse(&content).unwrap();
    let canonical = serialize(&tree);
    // The page's canonical text (ui/lib/fixtures/studio-canonical.html,
    // written by ui/lib/panel-tree.js and checked by ui/lib/parts.test.js).
    let page = std::fs::read_to_string(repository().join("ui/lib/fixtures/studio-canonical.html"))
        .unwrap();
    assert_eq!(canonical, page);
    assert_eq!(serialize(&parse(&canonical).unwrap()), canonical);
    assert_eq!(controls(&tree).len(), 65);
    assert!(parse("<div class=\"signal-cards\"><script></script></div>").is_err());
    assert!(parse("<div class=\"signal-cards\">text</div>").is_err());
    // ADR 0046 §5: the open slot card stands among the cards.
    let slot = "\n  <div class=\"signal-cards\">\n    <signal-slot bind=\"/synth\"></signal-slot>\n  </div>\n";
    assert_eq!(serialize(&parse(slot).unwrap()), slot);
    assert!(
        parse("<div class=\"signal-cards\"><signal-knob bind=\"/a.b\"></signal-knob></div>")
            .is_err()
    );
}

#[test]
fn grouping_the_studios_synth_moves_its_knobs_into_its_part() {
    let content = studio_content();
    let grouped = group_content(&content, "synth", None).unwrap();
    assert_eq!(grouped.moved, 27);
    let host = grouped.host;
    assert!(!host.contains("/synth."), "{host}");
    // The mono synth card keeps its piano roll; the slot card follows it.
    let synth = host.find("<h2>MONO SYNTH</h2>").unwrap();
    let roll = host.find("<signal-piano-roll").unwrap();
    let card = host
        .find("<signal-slot bind=\"/synth\"></signal-slot>")
        .unwrap();
    let rhythm = host.find("<h2>RHYTHM COMPOSER</h2>").unwrap();
    assert!(synth < roll && roll < card && card < rhythm, "{host}");
    let part = grouped.part.unwrap();
    assert!(part.contains("<h2>MONO SYNTH</h2>"), "{part}");
    assert!(part.contains(
        "<signal-knob bind=\".cutoff\" taper=\"log\" min=\"20\" max=\"10000\" label=\"cutoff\" skin=\"svg:assets/synth-knob.svg\"></signal-knob>"
    ), "{part}");
    assert!(
        part.contains("<div class=\"signal-group filter-contour\">"),
        "{part}"
    );
    assert!(!part.contains("piano-roll"), "{part}");
    // The card stays in the host: no id moved.
    assert!(grouped.cards.is_empty());
    // Every control of the host and the part, and no more.
    let host_controls = controls(&parse(&host).unwrap()).len();
    let part_controls = controls(&parse(&part).unwrap()).len();
    assert_eq!(host_controls + part_controls, 65 + 1);
}

#[test]
fn a_card_of_nothing_but_moved_controls_becomes_the_open_slot_card() {
    let content = studio_content();
    let grouped = group_content(&content, "echo", None).unwrap();
    assert_eq!(grouped.moved, 6);
    assert!(!grouped.host.contains("TAPE ECHO"));
    let card = grouped
        .host
        .find("<signal-slot bind=\"/echo\"></signal-slot>")
        .unwrap();
    let master = grouped.host.find("<h2>master</h2>").unwrap();
    assert!(card < master);
    let part = grouped.part.unwrap();
    assert!(
        part.contains("<section class=\"signal-card wide tape-echo\">"),
        "{part}"
    );
    assert_eq!(
        grouped.cards,
        vec![("tape-echo".to_owned(), "echo:tape-echo".to_owned())]
    );
    // Only the chosen controls (by their index among the panel's).
    let all = controls(&parse(&content).unwrap())
        .iter()
        .position(|c| c.attribute("bind") == Some("/echo.mix"))
        .unwrap();
    let one = group_content(&content, "echo", Some(&[all])).unwrap();
    assert_eq!(one.moved, 1);
    assert!(one.host.contains("TAPE ECHO"));
    assert!(one.part.unwrap().contains("bind=\".mix\""));
    // Nothing on it: the slot card at the end of the first grid.
    let none = group_content(&content, "dc", None).unwrap();
    assert_eq!(none.moved, 0);
    assert!(none.part.is_none());
    assert!(
        none.host
            .ends_with("    <signal-slot bind=\"/dc\"></signal-slot>\n  </div>\n")
    );
}

#[test]
fn card_ids_are_the_pages() {
    assert_eq!(slug("MONO SYNTH"), "mono-synth");
    assert_eq!(slug("  Tape  echo! "), "tape-echo");
    assert_eq!(slug("Dob és ütő"), "dob-es-uto");
    assert_eq!(slug("…"), "card");
    assert_eq!(
        card_ids(&["kick".into(), "kick".into(), "".into()]),
        vec!["kick", "kick-2", "card"]
    );
}

#[test]
fn windows_toml_follows_a_moved_card() {
    let text = "# the windows\n[[layout.\"k\".windows]]\nview = \"main\"\nscreen = 0\nx = 0\ny = 0\nwidth = 10\nheight = 10\ncards = [\"tape-echo\", \"master\"]   # kept\n\n[[layout.\"k\".windows]]\nview = \"panel\"\nscreen = 0\nx = 0\ny = 0\nwidth = 10\nheight = 10\ncards = [ \"tape-echo\" ]\n";
    let renamed = rename_cards(
        text,
        &[("tape-echo".to_owned(), "echo:tape-echo".to_owned())],
    )
    .unwrap();
    assert_eq!(renamed, text.replace("\"tape-echo\"", "\"echo:tape-echo\""));
    assert_eq!(rename_cards(text, &[("x".into(), "y".into())]), None);
    assert_eq!(
        rename_cards("not toml [", &[("x".into(), "y".into())]),
        None
    );
}

#[test]
fn a_parts_controls_on_removed_ports() {
    let text = part_text(
        "synth",
        "\n  <div class=\"signal-cards\">\n    <section class=\"signal-card\">\n      <h2>s</h2>\n      <div class=\"row\">\n        <div class=\"signal-group\">\n          <signal-knob bind=\".cutoff\" label=\"cutoff\"></signal-knob>\n        </div>\n        <signal-knob bind=\".drive\"></signal-knob>\n      </div>\n    </section>\n  </div>\n",
    );
    assert_eq!(controls_on(&text, &["cutoff".into()]), vec!["cutoff"]);
    assert_eq!(
        controls_on(&text, &["drive".into()]),
        vec!["<signal-knob> .drive"]
    );
    let without = without_controls(&text, &["cutoff".into()])
        .unwrap()
        .unwrap();
    assert!(!without.contains("cutoff"));
    assert!(!without.contains("signal-group"), "{without}");
    assert!(without.contains(".drive"));
    assert_eq!(without_controls(&text, &["x".into()]).unwrap(), None);
    let rooted = rooted_controls(&text, "/synth");
    assert_eq!(rooted.len(), 2);
    assert_eq!(rooted[0]["binds"], serde_json::json!(["/synth.cutoff"]));
    assert_eq!(rooted[0]["bind"], ".cutoff");
}
