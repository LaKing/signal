//! The panel editor's writes (`panel.write`, ADR 0026 §5,
//! `docs/ui-protocol.md`): the content of a panel file's one
//! `<signal-panel>` element replaced by what the editor sent, checked again
//! against the whitelist the page uses (`ui/lib/panel-whitelist.json`,
//! never trusting the page), with everything outside the element kept byte
//! for byte; and the SHA-256 that names the version a write is based on.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;

/// The whitelist of what the editor writes, shared with the page.
const WHITELIST: &str = include_str!("../../../ui/lib/panel-whitelist.json");

/// The containers, by key (`div.row`): the attributes they take and the
/// keys of their children (`control`, `text`, or a container's).
struct Whitelist {
    containers: BTreeMap<String, (Vec<String>, Vec<String>)>,
    controls: BTreeMap<String, Vec<String>>,
    boolean: Vec<String>,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn whitelist() -> &'static Whitelist {
    static PARSED: OnceLock<Whitelist> = OnceLock::new();
    PARSED.get_or_init(|| {
        // A malformed file accepts nothing (the tests read it).
        let value: Value = serde_json::from_str(WHITELIST).unwrap_or(Value::Null);
        let field = |name: &str| value.get(name).cloned().unwrap_or(Value::Null);
        let containers = field("containers")
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            (
                                strings(v.get("attributes").unwrap_or(&Value::Null)),
                                strings(v.get("children").unwrap_or(&Value::Null)),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let controls = field("controls")
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), strings(v))).collect())
            .unwrap_or_default();
        Whitelist {
            containers,
            controls,
            boolean: strings(&field("boolean")),
        }
    })
}

/// The containers' elements and required classes, by key.
const CONTAINERS: [(&str, &str, &str); 4] = [
    ("div.signal-cards", "div", "signal-cards"),
    ("section.signal-card", "section", "signal-card"),
    ("div.row", "div", "row"),
    ("div.signal-group", "div", "signal-group"),
];

/// A class token the whitelist's `classToken` allows: `[a-z][a-z0-9-]*`.
fn class_token(token: &str) -> bool {
    let mut bytes = token.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `text` with its character references decoded; only `&amp; &lt; &gt;
/// &quot; &#N; &#xN;`, as the page reads them.
fn decode(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let body_len = after
            .find(|c: char| c == ';' || c.is_whitespace() || c == '&' || c == '<')
            .unwrap_or(after.len());
        let body = &after[..body_len];
        let closed = after[body_len..].starts_with(';');
        let decoded = closed
            .then(|| match body {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                _ => {
                    // The page's spellings only: a lower-case `x` and digits, no sign.
                    let number = match body.strip_prefix("#x") {
                        Some(hex)
                            if (1..=6).contains(&hex.len())
                                && hex.bytes().all(|b| b.is_ascii_hexdigit()) =>
                        {
                            u32::from_str_radix(hex, 16).ok()
                        }
                        Some(_) => None,
                        _ => body
                            .strip_prefix('#')
                            .filter(|d| {
                                (1..=7).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit())
                            })
                            .and_then(|d| d.parse().ok()),
                    };
                    number.filter(|n| *n > 0).and_then(char::from_u32)
                }
            })
            .flatten();
        let Some(c) = decoded else {
            let shown = &rest[amp..amp + 1 + body_len + usize::from(closed)];
            return Err(format!(
                "the character reference {shown} is not one of &amp; &lt; &gt; &quot; &#…;"
            ));
        };
        out.push(c);
        rest = &after[body_len + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `{line}, column {column}` of `offset` in `text`, 1-based.
fn at(text: &str, offset: usize) -> String {
    let before = text.get(..offset).unwrap_or("");
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    format!("line {line}, column {column}")
}

/// Checks `html`, the content the editor sent for a `<signal-panel>`,
/// against the whitelist as the page does before sending (`parseContent` in
/// submit mode, `ui/lib/panel-tree.js`): the elements, attributes, classes,
/// text and nesting of `ui/lib/panel-whitelist.json`, nothing else (no
/// comment, script, style, event handler or unknown element or attribute).
pub fn check_content(html: &str) -> Result<(), String> {
    let list = whitelist();
    let mut stack: Vec<(String, String)> = vec![("panel".to_owned(), String::new())];
    let mut i = 0;
    let fail = |offset: usize, message: String| format!("{}: {message}", at(html, offset));
    while i < html.len() {
        let lt = html[i..].find('<').map_or(html.len(), |p| i + p);
        if lt > i {
            let text = decode(&html[i..lt]).map_err(|m| fail(i, m))?;
            let (key, _) = stack.last().cloned().unwrap_or_default();
            if !matches!(key.as_str(), "h1" | "h2" | "p") && !text.trim().is_empty() {
                return Err(fail(
                    i,
                    format!(
                        "text \"{}\" is outside a heading or description",
                        text.trim()
                    ),
                ));
            }
        }
        if lt >= html.len() {
            break;
        }
        i = lt;
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            return Err(fail(i, "a comment is not written by the editor".to_owned()));
        }
        let name_of = |s: &str| {
            let len = s
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
                .count();
            (len > 0 && s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic))
                .then(|| s[..len].to_ascii_lowercase())
        };
        if let Some(after) = rest.strip_prefix("</") {
            let Some(name) = name_of(after) else {
                return Err(fail(i, "a malformed end tag".to_owned()));
            };
            let close = after[name.len()..].trim_start();
            if !close.starts_with('>') {
                return Err(fail(i, format!("</{name}> is not closed")));
            }
            match stack.last() {
                Some((_, tag)) if stack.len() > 1 && *tag == name => {
                    stack.pop();
                }
                _ => {
                    return Err(fail(
                        i,
                        format!("</{name}> does not close the open element"),
                    ));
                }
            }
            i = html.len() - close.len() + 1;
            continue;
        }
        let Some(tag) = name_of(&rest[1..]) else {
            return Err(fail(
                i,
                format!(
                    "\"{}\" is not a tag the editor writes",
                    rest.chars().take(2).collect::<String>()
                ),
            ));
        };
        let begin = i;
        i += 1 + tag.len();
        let mut attributes: Vec<(String, Option<String>, usize)> = Vec::new();
        loop {
            let spaced = html[i..].len() - html[i..].trim_start().len();
            i += spaced;
            match html.as_bytes().get(i) {
                None => return Err(fail(begin, format!("<{tag}> is not closed"))),
                Some(b'>') => {
                    i += 1;
                    break;
                }
                _ => {}
            }
            if spaced == 0 {
                return Err(fail(
                    i,
                    format!("<{tag}>: attributes are separated by spaces"),
                ));
            }
            let len = html[i..]
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | '>' | '/' | '=' | '<')
                })
                .unwrap_or(html.len() - i);
            if len == 0 {
                return Err(fail(
                    i,
                    format!("<{tag}>: a stray character where an attribute belongs"),
                ));
            }
            let name = html[i..i + len].to_ascii_lowercase();
            let offset = i;
            i += len;
            let eq = html[i..].trim_start();
            let value = if let Some(after) = eq.strip_prefix('=') {
                let quoted = after.trim_start();
                let Some(inner) = quoted.strip_prefix('"') else {
                    return Err(fail(
                        offset,
                        format!("<{tag} {name}>: the value is not in double quotes"),
                    ));
                };
                let Some(close) = inner.find('"') else {
                    return Err(fail(
                        offset,
                        format!("<{tag} {name}>: the value is not closed"),
                    ));
                };
                let start = html.len() - inner.len();
                i = start + close + 1;
                Some(decode(&inner[..close]).map_err(|m| fail(start, m))?)
            } else {
                None
            };
            attributes.push((name, value, offset));
        }
        let classes: Vec<&str> = attributes
            .iter()
            .find(|a| a.0 == "class")
            .and_then(|a| a.1.as_deref())
            .unwrap_or("")
            .split_whitespace()
            .collect();
        let key = if list.controls.contains_key(&tag) {
            Some("control".to_owned())
        } else if matches!(tag.as_str(), "h1" | "h2" | "p") {
            Some(tag.clone())
        } else {
            let keys: Vec<&str> = CONTAINERS
                .iter()
                .filter(|(_, element, class)| *element == tag && classes.contains(class))
                .map(|(key, _, _)| *key)
                .collect();
            keys.first()
                .filter(|_| keys.len() == 1)
                .map(|k| (*k).to_owned())
        };
        let Some(key) = key else {
            return Err(fail(
                begin,
                format!("<{tag}> is not an element the editor writes"),
            ));
        };
        let parent = stack.last().map(|(k, _)| k.clone()).unwrap_or_default();
        let fits = if parent == "panel" {
            key == "div.signal-cards"
        } else {
            list.containers
                .get(&parent)
                .is_some_and(|(_, children)| children.contains(&key))
        };
        if !fits {
            return Err(fail(begin, format!("<{tag}> is not allowed in {parent}")));
        }
        let empty = Vec::new();
        let permitted = match key.as_str() {
            "control" => list.controls.get(&tag).unwrap_or(&empty),
            _ => list.containers.get(&key).map_or(&empty, |(a, _)| a),
        };
        for (n, (name, value, offset)) in attributes.iter().enumerate() {
            if attributes.iter().take(n).any(|a| a.0 == *name) {
                return Err(fail(*offset, format!("<{tag}>: {name} is given twice")));
            }
            if !permitted.contains(name) {
                let why = if name == "style" {
                    "style is not allowed"
                } else if name.starts_with("on") {
                    "event handlers are not allowed"
                } else {
                    "not an attribute it takes"
                };
                return Err(fail(*offset, format!("<{tag} {name}>: {why}")));
            }
            if value.is_none() && !list.boolean.contains(name) {
                return Err(fail(*offset, format!("<{tag} {name}>: needs a value")));
            }
        }
        for (n, class) in classes.iter().enumerate() {
            if !class_token(class) {
                return Err(fail(
                    begin,
                    format!("<{tag}>: the class \"{class}\" is not a lowercase name"),
                ));
            }
            if classes.iter().take(n).any(|c| c == class) {
                return Err(fail(
                    begin,
                    format!("<{tag}>: the class \"{class}\" is given twice"),
                ));
            }
        }
        // A control holds nothing but space; a heading or description text.
        if key == "control" && !html[i..].trim_start().starts_with(&format!("</{tag}")) {
            return Err(fail(i, format!("<{tag}> holds nothing")));
        }
        stack.push((key, tag));
    }
    if let Some((_, tag)) = stack.last().filter(|_| stack.len() > 1) {
        return Err(format!("<{tag}> is not closed"));
    }
    Ok(())
}

/// The file `text` with the content of its `<signal-panel>` (`range`, from
/// [`crate::bindings::panel_content`]) replaced by `html`; every byte
/// outside it is kept.
pub fn replace(text: &str, range: (usize, usize), html: &str) -> String {
    let mut out = String::with_capacity(text.len() + html.len());
    out.push_str(&text[..range.0]);
    out.push_str(html);
    out.push_str(&text[range.1..]);
    out
}

/// A new `ui/main.html` for the generated default panel's first write:
/// the standard head (the theme, the project's name as its title), the
/// edited `<signal-panel>`, and the controls module.
pub fn first_file(name: &str, html: &str) -> String {
    let title = name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html>\n\
<!-- The project's panel (docs/ui-controls.md \"Panels\"), written by the panel editor. -->\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
<link rel=\"stylesheet\" href=\"/ui/controls/theme.css\">\n\
<style>\n\
  :root {{ color-scheme: dark; }}\n\
  body {{ margin: 0; padding: 0.75rem; background: var(--signal-bg); color: var(--signal-fg); font: 13px/1.45 -apple-system, BlinkMacSystemFont, \"Helvetica Neue\", sans-serif; }}\n\
</style>\n\
</head>\n\
<body>\n\
<signal-panel>{html}</signal-panel>\n\
<script type=\"module\">\n\
import '/ui/controls/index.js';\n\
</script>\n\
</body>\n\
</html>\n"
    )
}

/// The SHA-256 of `bytes` (FIPS 180-4), as lower-case hex: the `base` of
/// `panel.read` and `panel.write`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = bytes.to_vec();
    let length = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&length.to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        // The message schedule as a window of its next 16 words.
        let mut window = [0u32; 16];
        for (slot, word) in window.iter_mut().zip(block.as_chunks::<4>().0) {
            *slot = u32::from_be_bytes(*word);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for k in K {
            let [
                w0,
                w1,
                w2,
                w3,
                w4,
                w5,
                w6,
                w7,
                w8,
                w9,
                w10,
                w11,
                w12,
                w13,
                w14,
                w15,
            ] = window;
            let s0 = w1.rotate_right(7) ^ w1.rotate_right(18) ^ (w1 >> 3);
            let s1 = w14.rotate_right(17) ^ w14.rotate_right(19) ^ (w14 >> 10);
            let next = w0.wrapping_add(s0).wrapping_add(w9).wrapping_add(s1);
            window = [
                w1, w2, w3, w4, w5, w6, w7, w8, w9, w10, w11, w12, w13, w14, w15, next,
            ];
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(k)
                .wrapping_add(w0);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (state, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(add);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::panel_content;

    /// FIPS 180-4's examples (and the empty message).
    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            sha256_hex(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// The whitelist file parses, and its class grammar is the one checked here.
    #[test]
    fn the_whitelist_file_is_read() {
        let value: Value = serde_json::from_str(WHITELIST).unwrap();
        assert_eq!(value["classToken"], "^[a-z][a-z0-9-]*$");
        assert_eq!(whitelist().controls.len(), 9);
        assert_eq!(whitelist().containers.len(), 7);
        assert!(
            class_token("wide")
                && class_token("a-1")
                && !class_token("A")
                && !class_token("1a")
                && !class_token("a_b")
        );
    }

    /// ADR 0026 §5 and operator decision 2026-10-16: h1 headings, lowercase
    /// class tokens; nothing else outside the whitelist, no comment.
    #[test]
    fn content_is_checked_against_the_whitelist() {
        let card = |inner: &str, class: &str| {
            format!(
                "\n  <div class=\"signal-cards\">\n    <section class=\"{class}\">{inner}</section>\n  </div>\n"
            )
        };
        for ok in [
            card(
                "<h1>kick</h1><p>a &amp; b &#x41;&#65;&#x4a;</p>",
                "signal-card wide steps",
            ),
            card(
                "<h2 class=\"x\">a</h2><div class=\"row tight\"><signal-knob bind=\"/a.b\" min=\"0\" max=\"1\" bipolar label=\"&quot;q&quot;\"></signal-knob><div class=\"signal-group\"><signal-meter bind=\"/out.l\"></signal-meter></div></div>",
                "signal-card view",
            ),
            card(
                "<signal-scope bind=\"/a.out\"></signal-scope>",
                "signal-card",
            ),
            "\n".to_owned(),
        ] {
            assert_eq!(check_content(&ok), Ok(()), "{ok}");
        }
        for (bad, why) in [
            (card("<script>x()</script>", "signal-card"), "<script>"),
            (card("<style>p{}</style>", "signal-card"), "<style>"),
            (
                card("<h2 style=\"color:red\">a</h2>", "signal-card"),
                "style is not allowed",
            ),
            (
                card(
                    "<div class=\"row\"><signal-knob bind=\"/a.b\" onclick=\"x()\"></signal-knob></div>",
                    "signal-card",
                ),
                "event handlers",
            ),
            (card("<img src=\"x\">", "signal-card"), "<img>"),
            (
                card(
                    "<div class=\"row\"><signal-knob bind=\"/a.b\" foo=\"1\"></signal-knob></div>",
                    "signal-card",
                ),
                "foo",
            ),
            (card("<h1 class=\"x\">a</h1>", "signal-card"), "class"),
            (card("", "signal-card Wide"), "lowercase"),
            (card("<!-- a note -->", "signal-card"), "comment"),
            (
                card("<div class=\"row\">text</div>", "signal-card"),
                "outside a heading",
            ),
            (
                card(
                    "<div class=\"row\"><signal-knob bind=/a.b></signal-knob></div>",
                    "signal-card",
                ),
                "double quotes",
            ),
            (card("<h2>a &mdash; b</h2>", "signal-card"), "reference"),
            // The page's spellings only (auditor-2 parity note).
            (card("<h2>&#X41;</h2>", "signal-card"), "reference"),
            (card("<h2>&#x+41;</h2>", "signal-card"), "reference"),
            (card("<h2>&#+65;</h2>", "signal-card"), "reference"),
            (
                card(
                    "<div class=\"row\"><signal-knob bind=\"/a\"><b></b></signal-knob></div>",
                    "signal-card",
                ),
                "holds nothing",
            ),
            (
                "<section class=\"signal-card\"></section>".to_owned(),
                "not allowed in panel",
            ),
            (card("<div class=\"row\">", "signal-card"), "does not close"),
        ] {
            let error = check_content(&bad).expect_err(&bad);
            assert!(error.contains(why), "{bad}: {error}");
        }
    }

    /// auditor-2-end-tag-scan (AC11): the end tag is found outside comments
    /// and scripts, so a `</signal-panel>` in either is content, not its end.
    #[test]
    fn the_panel_end_is_found_outside_comments_and_scripts() {
        let text = "<p>x</p>\n<signal-panel file=\"main.html\">\n<!-- </signal-panel> -->\n<script>let s = \"</signal-panel>\";</script>\n</signal-panel>\n<!-- after -->";
        let (start, end) = panel_content(text).unwrap();
        assert_eq!(
            &text[..start],
            "<p>x</p>\n<signal-panel file=\"main.html\">"
        );
        assert_eq!(&text[end..], "</signal-panel>\n<!-- after -->");
        assert_eq!(
            replace(text, (start, end), "\n"),
            "<p>x</p>\n<signal-panel file=\"main.html\">\n</signal-panel>\n<!-- after -->"
        );
        let only_in_comment = "<signal-panel>\n<!-- </signal-panel> -->\n";
        assert!(
            panel_content(only_in_comment)
                .unwrap_err()
                .contains("no end tag")
        );
        assert!(
            panel_content("<p>no panel</p><!-- <signal-panel> -->")
                .unwrap_err()
                .contains("no <signal-panel>")
        );
        let two = "<signal-panel></signal-panel>\n<signal-panel></signal-panel>";
        assert!(
            panel_content(two)
                .unwrap_err()
                .contains("2 <signal-panel> elements (lines 1, 2)")
        );
    }

    #[test]
    fn a_first_file_has_the_standard_head() {
        let text = first_file("a <b>", "\n");
        assert!(text.contains("<title>a &lt;b&gt;</title>"));
        assert!(text.contains("<link rel=\"stylesheet\" href=\"/ui/controls/theme.css\">"));
        assert!(text.contains("import '/ui/controls/index.js';"));
        assert_eq!(panel_content(&text).map(|(s, e)| &text[s..e]), Ok("\n"));
    }
}
