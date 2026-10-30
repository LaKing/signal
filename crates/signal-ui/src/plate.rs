//! The front plate's geometry (ADR 0047 §2, `docs/ui-controls.md`
//! "Panels"): `plate="W H"` on a panel file's `<signal-panel>` (its width
//! and height in plate units, 8 CSS px each) and `place="x y w h"` on each
//! card (`section.signal-card`, or the open slot card `<signal-slot>`).
//! Their check (`panel.check`, `signal ui check`): a malformed value or a
//! card beyond the plate is an error; a place without a plate, a card
//! without a place on a plate, and two cards overlapping are warnings. And
//! the `plate` attribute set or removed with every other byte of the file
//! kept (`panel.write`'s `plate`).

use signal_project::{Diagnostic, Severity};

use crate::bindings::{Finding, PANEL_ELEMENT, SLOT_ELEMENT, Tag, scan};

/// The largest plate side, in plate units (8000 CSS px).
pub const MAX_PLATE: u32 = 1000;

/// Whole numbers separated by spaces, exactly `n` of them.
fn numbers(value: &str, n: usize) -> Option<Vec<u32>> {
    let parts: Vec<&str> = value.split_ascii_whitespace().collect();
    if parts.len() != n {
        return None;
    }
    parts
        .iter()
        .map(|p| {
            (!p.is_empty() && p.len() <= 4 && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse().ok())
                .flatten()
        })
        .collect()
}

/// `plate="W H"`: two whole numbers of plate units, each 1 to [`MAX_PLATE`].
pub fn parse_plate(value: &str) -> Option<(u32, u32)> {
    let [w, h] = numbers(value, 2)?[..] else {
        return None;
    };
    let ok = |v: u32| (1..=MAX_PLATE).contains(&v);
    (ok(w) && ok(h)).then_some((w, h))
}

/// `place="x y w h"`: four whole numbers of plate units, `w` and `h` at least 1.
pub fn parse_place(value: &str) -> Option<(u32, u32, u32, u32)> {
    let [x, y, w, h] = numbers(value, 4)?[..] else {
        return None;
    };
    (w >= 1 && h >= 1 && [x, y, w, h].iter().all(|v| *v <= MAX_PLATE)).then_some((x, y, w, h))
}

/// The `plate` attribute of `text`'s `<signal-panel>`, as written.
pub fn panel_plate(text: &str) -> Option<String> {
    let mut found = None;
    scan(text, |tag| {
        if let Tag::Start {
            name, attributes, ..
        } = tag
            && name == PANEL_ELEMENT
            && found.is_none()
        {
            found = Some(
                attributes
                    .iter()
                    .find(|a| a.name == "plate")
                    .and_then(|a| a.value.as_ref().map(|(v, _)| v.clone())),
            );
        }
    });
    found.flatten()
}

/// The attributes of the start tag `tag` (`<name …>`), each as the byte
/// range from the space before it to its end, with its name.
fn attribute_spans(tag: &str) -> Vec<(usize, usize, String)> {
    let bytes = tag.as_bytes();
    let mut at = 1;
    while bytes
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
    {
        at += 1;
    }
    let mut spans = Vec::new();
    loop {
        let space = at;
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        match bytes.get(at) {
            None | Some(b'>') => return spans,
            Some(b'/') => {
                at += 1;
                continue;
            }
            Some(_) => {}
        }
        let begin = at;
        while bytes
            .get(at)
            .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/'))
        {
            at += 1;
        }
        let name = tag[begin..at].to_ascii_lowercase();
        let mut after = at;
        while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
            after += 1;
        }
        if bytes.get(after) == Some(&b'=') {
            after += 1;
            while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
                after += 1;
            }
            match bytes.get(after) {
                Some(q @ (b'"' | b'\'')) => {
                    let close = tag[after + 1..]
                        .find(*q as char)
                        .map_or(tag.len(), |i| after + 1 + i + 1);
                    at = close;
                }
                _ => {
                    while bytes
                        .get(after)
                        .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                    {
                        after += 1;
                    }
                    at = after;
                }
            }
        }
        if at == begin {
            at += 1;
            continue;
        }
        spans.push((space, at, name));
    }
}

/// `text` with its `<signal-panel>`'s `plate` set to `plate` (inserted
/// after its last attribute) or removed (`None`, with the space before
/// it), every other byte kept: setting and then removing a plate gives the
/// file back byte for byte. Refused for a malformed plate or a file
/// without one `<signal-panel>`.
pub fn with_plate(text: &str, plate: Option<&str>) -> Result<String, String> {
    if let Some(value) = plate
        && parse_plate(value).is_none()
    {
        return Err(format!(
            "plate=\"{value}\" is not the plate's width and height in plate units (two whole numbers, 1 to {MAX_PLATE})"
        ));
    }
    let mut tags = Vec::new();
    scan(text, |tag| {
        if let Tag::Start {
            name, offset, end, ..
        } = tag
            && name == PANEL_ELEMENT
        {
            tags.push((offset, end));
        }
    });
    let [(start, end)] = tags.as_slice() else {
        return Err(format!(
            "the file has {} <{PANEL_ELEMENT}> elements; its plate is set on one",
            tags.len()
        ));
    };
    let tag = &text[*start..*end];
    let spans = attribute_spans(tag);
    let written = plate.map(|v| {
        format!(
            " plate=\"{}\"",
            v.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
        )
    });
    let (from, to) = match spans.iter().find(|(_, _, name)| name == "plate") {
        Some((space, stop, _)) => (*space, *stop),
        None => {
            let last = spans
                .last()
                .map_or_else(|| 1 + PANEL_ELEMENT.len(), |(_, stop, _)| *stop);
            (last, last)
        }
    };
    let mut out = String::with_capacity(text.len() + 24);
    out.push_str(&text[..start + from]);
    out.push_str(written.as_deref().unwrap_or(""));
    out.push_str(&text[start + to..]);
    Ok(out)
}

/// A card's place as written: its value, offset, the card's offset, and the place.
struct Placed {
    value: String,
    at: usize,
    line_of: usize,
    place: Option<(u32, u32, u32, u32)>,
}

/// The plate's findings in `text` (ADR 0047 §2): what `panel.check` and
/// `signal ui check` say of `plate` and `place`.
pub fn findings(file: &str, text: &str) -> Vec<Finding> {
    let mut plate: Option<(String, usize)> = None;
    let mut panels = 0;
    let mut cards: Vec<(usize, Option<(String, usize)>)> = Vec::new();
    scan(text, |tag| {
        let Tag::Start {
            name,
            offset,
            attributes,
            ..
        } = tag
        else {
            return;
        };
        let attribute = |n: &str| {
            attributes
                .iter()
                .find(|a| a.name == n)
                .and_then(|a| a.value.clone())
        };
        if name == PANEL_ELEMENT {
            panels += 1;
            if panels == 1 {
                plate = attribute("plate");
            }
            return;
        }
        let card = name == SLOT_ELEMENT
            || (name == "section"
                && attribute("class")
                    .is_some_and(|(c, _)| c.split_ascii_whitespace().any(|t| t == "signal-card")));
        if card {
            cards.push((offset, attribute("place")));
        }
    });
    let mut found = Vec::new();
    let mut say = |offset: usize, severity, message: String| {
        found.push(Finding {
            control: None,
            diagnostic: Diagnostic::at(severity, file, text, offset, message),
        });
    };
    let size = match &plate {
        Some((value, at)) => {
            let size = parse_plate(value);
            if size.is_none() {
                say(
                    *at,
                    Severity::Error,
                    format!(
                        "plate=\"{value}\" is not the plate's width and height in plate units (8 px each; two whole numbers, 1 to {MAX_PLATE}: plate=\"160 90\")"
                    ),
                );
            }
            size
        }
        None => None,
    };
    let placed: Vec<Placed> = cards
        .iter()
        .filter_map(|(card, place)| {
            place.as_ref().map(|(value, at)| Placed {
                value: value.clone(),
                at: *at,
                line_of: *card,
                place: parse_place(value),
            })
        })
        .collect();
    for p in &placed {
        if p.place.is_none() {
            say(
                p.at,
                Severity::Error,
                format!(
                    "place=\"{}\" is not a card's place in plate units (x y width height, whole numbers, the size at least 1: place=\"0 0 40 20\")",
                    p.value
                ),
            );
        }
    }
    if plate.is_none() {
        if let Some(first) = placed.first() {
            say(
                first.at,
                Severity::Warning,
                format!(
                    "the cards' places are read on a plate only: <{PANEL_ELEMENT}> has no plate=\"W H\", so the panel is laid out at 1280 px instead"
                ),
            );
        }
        return found;
    }
    let Some((width, height)) = size else {
        return found;
    };
    for (card, place) in &cards {
        if place.is_none() {
            say(
                *card,
                Severity::Warning,
                "this card has no place on the plate (place=\"x y w h\"): it is put below the placed cards".to_owned(),
            );
        }
    }
    for p in &placed {
        if let Some((x, y, w, h)) = p.place
            && (x + w > width || y + h > height)
        {
            say(
                p.at,
                Severity::Error,
                format!(
                    "place=\"{}\" reaches beyond the plate ({width} × {height} plate units)",
                    p.value
                ),
            );
        }
    }
    for (i, a) in placed.iter().enumerate() {
        for b in placed.iter().skip(i + 1) {
            let (Some(pa), Some(pb)) = (a.place, b.place) else {
                continue;
            };
            if pa.0 < pb.0 + pb.2 && pb.0 < pa.0 + pa.2 && pa.1 < pb.1 + pb.3 && pb.1 < pa.1 + pa.3
            {
                let line = text[..b.line_of.min(text.len())].matches('\n').count() + 1;
                let line_a = text[..a.line_of.min(text.len())].matches('\n').count() + 1;
                say(
                    b.at,
                    Severity::Warning,
                    format!(
                        "this card (line {line}, place=\"{}\") overlaps the card at line {line_a} (place=\"{}\")",
                        b.value, a.value
                    ),
                );
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(text: &str) -> Vec<(Severity, String)> {
        findings("ui/main.html", text)
            .into_iter()
            .map(|f| (f.diagnostic.severity, f.diagnostic.message))
            .collect()
    }

    #[test]
    fn plates_and_places_parse_as_whole_plate_units() {
        assert_eq!(parse_plate("160 90"), Some((160, 90)));
        assert_eq!(parse_plate(" 160   90 "), Some((160, 90)));
        for bad in [
            "160", "160 90 1", "0 90", "160 -1", "1.5 2", "a b", "1001 2", "",
        ] {
            assert_eq!(parse_plate(bad), None, "{bad}");
        }
        assert_eq!(parse_place("0 12 160 38"), Some((0, 12, 160, 38)));
        for bad in ["0 0 0 4", "0 0 4", "0 0 4 4 4", "-1 0 4 4", "0 0 4 x"] {
            assert_eq!(parse_place(bad), None, "{bad}");
        }
    }

    /// ADR 0047 test 3: outside the plate an error, overlaps a warning, a malformed value an error.
    #[test]
    fn the_check_judges_the_places() {
        let ok = r#"<signal-panel plate="160 40"><div class="signal-cards">
<section class="signal-card" place="0 0 80 20"></section>
<section class="signal-card wide" place="80 0 80 20"></section>
<signal-slot bind="/a" place="0 20 160 20"></signal-slot>
</div></signal-panel>"#;
        assert_eq!(messages(ok), []);
        let beyond = ok.replace("80 0 80 20", "79 0 82 20");
        let found = messages(&beyond);
        assert!(
            found
                .iter()
                .any(|(s, m)| *s == Severity::Error && m.contains("beyond the plate")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|(s, m)| *s == Severity::Warning && m.contains("overlaps")),
            "{found:?}"
        );
        let found = messages(&ok.replace("0 20 160 20", "0 20 160"));
        assert!(
            found
                .iter()
                .any(|(s, m)| *s == Severity::Error && m.contains("is not a card's place")),
            "{found:?}"
        );
        let found = messages(&ok.replace("160 40", "160"));
        assert!(
            found
                .iter()
                .any(|(s, m)| *s == Severity::Error && m.contains("plate=\"160\"")),
            "{found:?}"
        );
        let found = messages(&ok.replace(" plate=\"160 40\"", ""));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, Severity::Warning);
        let found = messages(&ok.replace(" place=\"0 20 160 20\"", ""));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.contains("no place"));
        // A panel without either: nothing to say.
        assert_eq!(
            messages(
                "<signal-panel><div class=\"signal-cards\"><section class=\"signal-card\"></section></div></signal-panel>"
            ),
            []
        );
    }

    /// Set, changed and removed, every other byte kept; removing restores the file byte for byte.
    #[test]
    fn the_plate_is_set_and_removed_in_place() {
        let text = "<!doctype html>\n<!-- <signal-panel plate=\"1 1\"> -->\n<body>\n<signal-panel part=\"synth\">\n  <div class=\"signal-cards\"></div>\n</signal-panel>\n";
        let set = with_plate(text, Some("160 40")).unwrap();
        assert!(
            set.contains("<signal-panel part=\"synth\" plate=\"160 40\">"),
            "{set}"
        );
        assert_eq!(panel_plate(&set).as_deref(), Some("160 40"));
        let changed = with_plate(&set, Some("160  48")).unwrap();
        assert!(
            changed.contains("<signal-panel part=\"synth\" plate=\"160 48\">"),
            "{changed}"
        );
        assert_eq!(with_plate(&changed, None).unwrap(), text);
        assert_eq!(with_plate(text, None).unwrap(), text);
        let bare = "<signal-panel>\n</signal-panel>";
        let set = with_plate(bare, Some("160 90")).unwrap();
        assert_eq!(set, "<signal-panel plate=\"160 90\">\n</signal-panel>");
        assert_eq!(with_plate(&set, None).unwrap(), bare);
        assert!(with_plate(bare, Some("0 1")).is_err());
        assert!(with_plate("<p></p>", Some("1 1")).is_err());
        assert_eq!(panel_plate(text), None);
    }
}
