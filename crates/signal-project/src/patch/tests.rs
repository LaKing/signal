//! The text edit of patches (M2.9-3): every op on every form the loader
//! accepts, its inverse and that inverse's inverse (undo, redo) byte for
//! byte, comments kept, and the refusals of untrusted edits. Committed
//! projects are read, never written.

use super::*;
use crate::{Project, SourceFile};

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap()
}

fn registry() -> signal_core::Registry {
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// The parameter order of std types and the kick project's composites.
fn modules(module: &str) -> Option<Vec<String>> {
    if ["kick", "drums"].contains(&module) {
        return Some(Vec::new());
    }
    let registry = registry();
    let t = registry.get(module)?;
    Some(
        t.descriptor
            .parameters
            .iter()
            .map(|p| p.name.to_owned())
            .collect(),
    )
}

static NO_TEMPLATES: BTreeMap<String, Interface> = BTreeMap::new();

fn no_content(_: &str) -> Option<ContentNames> {
    None
}

fn ctx(level: &str) -> Context<'_> {
    Context {
        level,
        module: &modules,
        templates: &NO_TEMPLATES,
        content: &no_content,
        file: &no_file,
        source: &no_source,
        bound: &super::no_bindings,
    }
}

fn no_file(_: &str) -> Option<String> {
    None
}

fn no_source(_: &str) -> Option<crate::inside::Source> {
    None
}

/// `ops` on `text`, then the inverse (through JSON, as the page sends it
/// back), then the inverse's inverse: both byte-exact. Returns the patched
/// text and positions.
fn round_trip(
    text: &str,
    positions: Option<&str>,
    level: &str,
    ops: &[Op],
) -> (String, Option<String>) {
    let patched =
        apply(text, positions, ops, &ctx(level)).unwrap_or_else(|e| panic!("{ops:?}: {e}"));
    let inverse: Vec<Op> =
        serde_json::from_str(&serde_json::to_string(&patched.inverse).unwrap()).unwrap();
    let undone = apply(
        &patched.text,
        patched.positions.as_deref(),
        &inverse,
        &ctx(level),
    )
    .unwrap_or_else(|e| panic!("undo of {ops:?}: {e}"));
    assert_eq!(undone.text, text, "undo of {ops:?} is not byte-exact");
    assert_eq!(
        undone.positions.as_deref(),
        positions,
        "undo of {ops:?}: positions"
    );
    let redone = apply(
        &undone.text,
        undone.positions.as_deref(),
        &undone.inverse,
        &ctx(level),
    )
    .unwrap_or_else(|e| panic!("redo of {ops:?}: {e}"));
    assert_eq!(
        redone.text, patched.text,
        "redo of {ops:?} is not byte-exact"
    );
    assert_eq!(
        redone.positions, patched.positions,
        "redo of {ops:?}: positions"
    );
    (patched.text, patched.positions)
}

fn plans(project: &str, composites: &[(&str, String)]) -> Result<(), String> {
    let mut files = vec![SourceFile {
        path: "project.toml".into(),
        text: project.to_owned(),
    }];
    for (path, text) in composites {
        files.push(SourceFile {
            path: (*path).to_owned(),
            text: text.clone(),
        });
    }
    let project = Project::from_sources(files).map_err(|d| format!("{d:?}"))?;
    project
        .plan(&registry())
        .map(|_| ())
        .map_err(|d| format!("{d:?}"))
}

fn add(name: &str, module: &str) -> Op {
    Op::InstanceAdd {
        name: name.into(),
        module: module.into(),
        params: BTreeMap::new(),
        x: None,
        y: None,
        edits: None,
        layout: None,
    }
}

fn remove(name: &str) -> Op {
    Op::InstanceRemove {
        name: name.into(),
        edits: None,
        layout: None,
    }
}

fn connect(from: &str, to: &str) -> Op {
    Op::ConnectionAdd {
        from: from.into(),
        to: to.into(),
        scale: None,
        edits: None,
    }
}

fn disconnect(from: &str, to: &str) -> Op {
    Op::ConnectionRemove {
        from: from.into(),
        to: to.into(),
        edits: None,
    }
}

fn scale(from: &str, to: &str, s: f64) -> Op {
    Op::ConnectionScale {
        from: from.into(),
        to: to.into(),
        scale: s,
        edits: None,
    }
}

const HEAD: &str =
    "# synthetic\nschema_version = 1\n\n[project]\nname = \"t\"\nsample_rate = 48000\n\n";
const INST: &str = "[instances]\na = { module = \"std.lfo\", freq_hz = 1 } # the lfo\n# the gain\nb = { module = \"std.gain\", db = -6 }\nc = { module = \"std.lag\" }\nout = { module = \"std.audio_out\", gain = 0.5 }\n";
const SECTIONS: &str = "[instances.a]\nmodule = \"std.lfo\"\nfreq_hz = 1\n\n# the gain\n[instances.b]\nmodule = \"std.gain\"\ndb = -6\n\n[instances.c]\nmodule = \"std.lag\"\n\n[instances.out]\nmodule = \"std.audio_out\"\ngain = 0.5\n";
const MULTI: &str = "\n[wiring]\nconnections = [\n  { from = \"a.out\", to = \"b.in\" }, # first\n  # the middle one\n  { from = \"b.out\", to = \"c.in\" },\n  { from = \"c.out\", to = \"out.l\" },\n]\n";
const MULTI_NO_COMMA: &str = "\n[wiring]\nconnections = [\n  { from = \"a.out\", to = \"b.in\" }, # first\n  { from = \"b.out\", to = \"c.in\" },\n  { from = \"c.out\", to = \"out.l\" } # last\n]\n";
const SINGLE: &str = "\n[wiring]\nconnections = [{ from = \"a.out\", to = \"b.in\" }, { from = \"b.out\", to = \"c.in\" }, { from = \"c.out\", to = \"out.l\" }]\n";
const EMPTY: &str = "\n[wiring]\nconnections = []\n";
const EMPTY_LINES: &str = "\n[wiring]\nconnections = [\n]\n";
const TABLES: &str = "\n[[wiring.connections]]\nfrom = \"a.out\"\nto = \"b.in\"\n\n# middle\n[[wiring.connections]]\nfrom = \"b.out\"\nto = \"c.in\"\n\n[[wiring.connections]]\nfrom = \"c.out\"\nto = \"out.l\"\nscale = 0.5\n";
const DOTTED: &str = "[instances]\na.module = \"std.lfo\"\na.freq_hz = 1\nb = { module = \"std.gain\", db = -6 }\nc.module = \"std.lag\"\nout = { module = \"std.audio_out\", gain = 0.5 }\n";
const INLINE_ROOT: &str = "instances = { a = { module = \"std.lfo\" }, b = { module = \"std.gain\" }, c = { module = \"std.lag\" }, out = { module = \"std.audio_out\" } }\nwiring = { connections = [{ from = \"a.out\", to = \"b.in\" }, { from = \"b.out\", to = \"c.in\" }, { from = \"c.out\", to = \"out.l\" }] }\n";

/// Every form × connection.add, every connection.remove (first, middle,
/// last), instance.add, every instance.remove: byte-exact undo and redo,
/// and every result loads and plans.
#[test]
fn every_op_on_every_form_round_trips_byte_for_byte() {
    let forms: Vec<(&str, String)> = vec![
        ("single-line array", format!("{HEAD}{INST}{SINGLE}")),
        ("multi-line, trailing comma", format!("{HEAD}{INST}{MULTI}")),
        (
            "multi-line, no trailing comma",
            format!("{HEAD}{INST}{MULTI_NO_COMMA}"),
        ),
        ("no [wiring]", format!("{HEAD}{INST}")),
        ("connections = []", format!("{HEAD}{INST}{EMPTY}")),
        ("connections = [\\n]", format!("{HEAD}{INST}{EMPTY_LINES}")),
        ("[[wiring.connections]]", format!("{HEAD}{INST}{TABLES}")),
        ("[instances.x]", format!("{HEAD}{SECTIONS}{MULTI}")),
        (
            "[instances.x] + [[wiring.connections]]",
            format!("{HEAD}{SECTIONS}{TABLES}"),
        ),
        ("dotted keys", format!("{HEAD}{DOTTED}{MULTI}")),
        // Root keys come before the first table.
        (
            "root inline tables",
            format!(
                "schema_version = 1\n{INLINE_ROOT}\n[project]\nname = \"t\"\nsample_rate = 48000\n"
            ),
        ),
    ];
    let mut count = 0;
    for (label, text) in &forms {
        let m = map(text, &Scheme::LEVEL).unwrap();
        let mut ops = vec![connect("a.out", "out.r"), add("svf", "std.svf")];
        ops.extend(m.conns.iter().map(|c| disconnect(&c.from, &c.to)));
        ops.extend(m.entries.iter().map(|e| remove(&e.name)));
        for op in ops {
            let (after, _) = round_trip(text, None, "", std::slice::from_ref(&op));
            plans(&after, &[])
                .unwrap_or_else(|e| panic!("{label} / {op:?} does not plan: {e}\n{after}"));
            count += 1;
        }
    }
    assert!(count >= 90, "{count}");
}

/// The committed projects and composites, read (never written): every
/// instance and every connection removed in turn, an add of each kind, and
/// a scale, each undone and redone byte for byte.
#[test]
fn kick_pulse_physical_av_and_a_composite_round_trip() {
    let composites = [
        (
            "composites/drums.toml",
            read("projects/kick/composites/drums.toml"),
        ),
        (
            "composites/kick.toml",
            read("projects/kick/composites/kick.toml"),
        ),
    ];
    let cases = [
        ("projects/kick/project.toml", "", ("lfo.out", "out.l"), true),
        (
            "projects/pulse/project.toml",
            "",
            ("sweep.out", "out.l"),
            false,
        ),
        (
            "projects/physical-av/project.toml",
            "",
            ("membrane_a.out", "out.r"),
            false,
        ),
        (
            "projects/kick/composites/kick.toml",
            "kick",
            ("amp_env.out", "shaper.in"),
            false,
        ),
    ];
    for (path, level, (from, to), kick) in cases {
        let text = read(path);
        let m = map(&text, &Scheme::LEVEL).unwrap();
        let first = &m.conns[0];
        let mut ops = vec![
            connect(from, to),
            add("svf", "std.svf"),
            scale(&first.from, &first.to, 0.25),
            scale(&first.from, &first.to, 1.0),
        ];
        ops.extend(m.conns.iter().map(|c| disconnect(&c.from, &c.to)));
        ops.extend(m.entries.iter().map(|e| remove(&e.name)));
        for op in ops {
            let (after, _) = round_trip(&text, None, level, std::slice::from_ref(&op));
            if kick && !matches!(op, Op::InstanceRemove { .. }) {
                plans(&after, &composites).unwrap_or_else(|e| panic!("{path} / {op:?}: {e}"));
            }
        }
    }
    // A composite without [wiring]: one is created, and the undo removes it.
    let drums = read("projects/kick/composites/drums.toml");
    let (after, _) = round_trip(
        &drums,
        None,
        "drums",
        &[connect("kick.out", "kick.trigger")],
    );
    assert!(after.contains("\n\n[wiring]\nconnections = [\n  { from = \"kick.out\", to = \"kick.trigger\" },\n]\n\n[exports]"), "{after}");
}

#[test]
fn appended_entries_are_written_in_the_files_style() {
    let kick = read("projects/kick/project.toml");
    let patched = apply(
        &kick,
        None,
        &[
            Op::InstanceAdd {
                name: "svf".into(),
                module: "std.svf".into(),
                params: [("q".to_owned(), 2.0), ("cutoff_hz".to_owned(), 500.0)].into(),
                x: None,
                y: None,
                edits: None,
                layout: None,
            },
            connect("svf.lp", "out.l"),
        ],
        &ctx(""),
    )
    .unwrap();
    // The last key of [instances], parameters in declared order; the last
    // element of connections, on its own line.
    assert!(patched.text.contains(
        "out = { module = \"std.audio_out\", gain = 0.75 }\nsvf = { module = \"std.svf\", cutoff_hz = 500, q = 2 }\n\n[wiring]"
    ), "{}", patched.text);
    assert!(patched.text.contains(
        "  { from = \"drums.out\", to = \"out.r\" },\n  { from = \"svf.lp\", to = \"out.l\" },\n]\n"
    ));
    // Everything else byte for byte: removing the two additions gives the file.
    let without = patched
        .text
        .replace(
            "svf = { module = \"std.svf\", cutoff_hz = 500, q = 2 }\n",
            "",
        )
        .replace("  { from = \"svf.lp\", to = \"out.l\" },\n", "");
    assert_eq!(without, kick);
}

#[test]
fn a_removed_line_takes_its_own_trailing_comment_never_a_neighbours() {
    let text = format!("{HEAD}{INST}{MULTI}");
    // The first connection carries `# first`; the middle one a comment above.
    let patched = apply(&text, None, &[disconnect("a.out", "b.in")], &ctx("")).unwrap();
    assert!(!patched.text.contains("# first"));
    assert!(
        patched
            .text
            .contains("  # the middle one\n  { from = \"b.out\", to = \"c.in\" },\n")
    );
    let patched = apply(&text, None, &[disconnect("b.out", "c.in")], &ctx("")).unwrap();
    assert!(
        patched
            .text
            .contains("{ from = \"a.out\", to = \"b.in\" }, # first\n  { from = \"c.out\"")
    );
    assert!(!patched.text.contains("the middle one"));
    // An instance: its trailing comment goes with it, the next one's comment stays.
    let patched = apply(&text, None, &[remove("a")], &ctx("")).unwrap();
    assert!(!patched.text.contains("# the lfo"));
    assert!(patched.text.contains("[instances]\n# the gain\nb = "));
}

/// Entries sharing lines with comments between them: a removal never takes
/// a neighbour's comment, and its undo puts back every byte.
#[test]
fn a_shared_line_keeps_a_neighbours_comment() {
    let (ab, bc, co) = (
        "{ from = \"a.out\", to = \"b.in\" }",
        "{ from = \"b.out\", to = \"c.in\" }",
        "{ from = \"c.out\", to = \"out.l\" }",
    );
    let wiring =
        |connections: &str| format!("{HEAD}{INST}\n[wiring]\nconnections = {connections}\n");
    // The last entry, after a comment: the comment and its line stay.
    let text = wiring(&format!("[{ab}, {bc}, # first\n  {co}]"));
    let (patched, _) = round_trip(&text, None, "", &[disconnect("c.out", "out.l")]);
    assert!(
        patched.contains(&format!("[{ab}, {bc}, # first\n]")),
        "{patched}"
    );
    plans(&patched, &[]).unwrap();
    // An entry before a comment: its comma goes, the comment stays.
    let (patched, _) = round_trip(&text, None, "", &[disconnect("b.out", "c.in")]);
    assert!(
        patched.contains(&format!("[{ab}, # first\n  {co}]")),
        "{patched}"
    );
    plans(&patched, &[]).unwrap();
    let (patched, _) = round_trip(&text, None, "", &[disconnect("a.out", "b.in")]);
    assert!(
        patched.contains(&format!("[{bc}, # first\n  {co}]")),
        "{patched}"
    );
    // Two entries, the first one's comment between them.
    let text = wiring(&format!("[{ab}, # first\n  {bc}]"));
    let (patched, _) = round_trip(&text, None, "", &[disconnect("b.out", "c.in")]);
    assert!(patched.contains(&format!("[{ab}, # first\n]")), "{patched}");
    plans(&patched, &[]).unwrap();
    let (patched, _) = round_trip(&text, None, "", &[disconnect("a.out", "b.in")]);
    assert!(patched.contains(&format!("[# first\n  {bc}]")), "{patched}");
    plans(&patched, &[]).unwrap();
}

/// An unreadable `ui/schematic.toml` holds no position: a removal is not
/// blocked by it, and only an edit of it fails.
#[test]
fn an_unreadable_positions_file_blocks_only_its_own_edits() {
    let text = format!("{HEAD}{INST}{MULTI}");
    let broken = "[top\na = { x = 1 }\n";
    let patched = apply(&text, Some(broken), &[remove("c")], &ctx("")).unwrap();
    assert!(!patched.positions_changed);
    assert_eq!(patched.positions.as_deref(), Some(broken));
    let placed = Op::InstanceAdd {
        name: "svf".into(),
        module: "std.svf".into(),
        params: BTreeMap::new(),
        x: Some(1.0),
        y: Some(2.0),
        edits: None,
        layout: None,
    };
    let error = apply(&text, Some(broken), &[placed], &ctx("")).unwrap_err();
    assert_eq!(error.code, "write-failed");
}

#[test]
fn ops_refuse_what_the_issue_lists() {
    let text = format!("{HEAD}{INST}{MULTI}");
    let code = |ops: &[Op]| apply(&text, None, ops, &ctx("")).unwrap_err().code;
    assert_eq!(code(&[add("Svf", "std.svf")]), "bad-value");
    assert_eq!(code(&[add("a", "std.svf")]), "bad-value");
    assert_eq!(code(&[add("svf", "std.nothing")]), "unknown-module");
    assert_eq!(code(&[remove("nobody")]), "unknown-instance");
    assert_eq!(code(&[connect("nobody.out", "b.in")]), "unknown-instance");
    assert_eq!(code(&[connect("a.out", "b.in")]), "bad-value");
    assert_eq!(code(&[connect("a", "b.in")]), "bad-value");
    assert_eq!(code(&[disconnect("a.out", "c.in")]), "bad-value");
    assert_eq!(code(&[scale("a.out", "b.in", f64::INFINITY)]), "bad-value");
    // The whole patch fails together.
    assert_eq!(
        code(&[add("svf", "std.svf"), remove("nobody")]),
        "unknown-instance"
    );
}

#[test]
fn a_scale_of_one_removes_the_key() {
    let kick = read("projects/kick/project.toml");
    let patched = apply(
        &kick,
        None,
        &[scale("lfo.out", "drums.kick_decay", 1.0)],
        &ctx(""),
    )
    .unwrap();
    assert!(
        patched
            .text
            .contains("  { from = \"lfo.out\", to = \"drums.kick_decay\" },\n")
    );
    let patched = apply(&kick, None, &[scale("drums.out", "out.l", 0.5)], &ctx("")).unwrap();
    assert!(
        patched
            .text
            .contains("  { from = \"drums.out\", to = \"out.l\", scale = 0.5 },\n")
    );
}

#[test]
fn positions_are_written_as_schematic_move_writes_them_and_undone() {
    let text = format!("{HEAD}{INST}{MULTI}");
    let at = |name: &str, x: f64, y: f64| Op::InstanceAdd {
        name: name.into(),
        module: "std.svf".into(),
        params: BTreeMap::new(),
        x: Some(x),
        y: Some(y),
        edits: None,
        layout: None,
    };
    // No ui/schematic.toml: created, and the undo removes it again.
    let (_, positions) = round_trip(&text, None, "", &[at("svf", 12.4, -3.6)]);
    assert_eq!(
        positions.as_deref(),
        Some("[top]\nsvf = { x = 12, y = -4 }\n")
    );
    // One with comments and other levels: kept byte for byte.
    let file = "# moved by hand\n[top]\na = { x = 1, y = 2 } # here\n\n[composite.kick]\nosc = { x = 0, y = 30 }\n";
    let (_, positions) = round_trip(&text, Some(file), "", &[at("svf", 5.0, 6.0)]);
    assert_eq!(
        positions.as_deref(),
        Some(
            "# moved by hand\n[top]\na = { x = 1, y = 2 } # here\nsvf = { x = 5, y = 6 }\n\n[composite.kick]\nosc = { x = 0, y = 30 }\n"
        )
    );
    // A removal takes the instance's position, and its undo puts it back in place.
    let (_, positions) = round_trip(&text, Some(file), "", &[remove("a")]);
    assert_eq!(
        positions.as_deref(),
        Some("# moved by hand\n[top]\n\n[composite.kick]\nosc = { x = 0, y = 30 }\n")
    );
}

/// A commit (a parameter written into an instance entry) between a patch
/// and its undo: the commit is kept, the undo still exact.
#[test]
fn undo_holds_across_a_commit() {
    let text = format!("{HEAD}{INST}{MULTI}");
    let commit = |t: &str| {
        crate::write_parameter(t, "a", "freq_hz", Some(0.034364261168384876), &["freq_hz"]).unwrap()
    };
    let patched = apply(&text, None, &[remove("b")], &ctx("")).unwrap();
    let committed = commit(&patched.text);
    let undone = apply(&committed, None, &patched.inverse, &ctx("")).unwrap();
    assert_eq!(undone.text, commit(&text));
    // The added instance committed, then undone; its redo keeps the commit.
    let patched = apply(&text, None, &[add("svf", "std.svf")], &ctx("")).unwrap();
    let committed =
        crate::write_parameter(&patched.text, "svf", "q", Some(2.0), &["cutoff_hz", "q"]).unwrap();
    let undone = apply(&committed, None, &patched.inverse, &ctx("")).unwrap();
    assert_eq!(undone.text, text);
    let redone = apply(&undone.text, None, &undone.inverse, &ctx("")).unwrap();
    assert_eq!(redone.text, committed);
}

/// A hand-written file with a same-endpoint duplicate: removing either
/// one and its undo and redo are exact (the entry is the edit's index).
#[test]
fn duplicates_in_a_file_undo_and_redo_by_index() {
    let text = format!(
        "{HEAD}{INST}\n[wiring]\nconnections = [\n  {{ from = \"a.out\", to = \"b.in\" }},\n  {{ from = \"b.out\", to = \"c.in\" }},\n  {{ from = \"a.out\", to = \"b.in\", scale = 2 }},\n]\n"
    );
    round_trip(&text, None, "", &[disconnect("a.out", "b.in")]);
    // The second, by its own inverse: remove the first, then the second.
    let patched = apply(
        &text,
        None,
        &[disconnect("a.out", "b.in"), disconnect("a.out", "b.in")],
        &ctx(""),
    )
    .unwrap();
    let undone = apply(&patched.text, None, &patched.inverse, &ctx("")).unwrap();
    assert_eq!(undone.text, text);
    let redone = apply(&undone.text, None, &undone.inverse, &ctx("")).unwrap();
    assert_eq!(redone.text, patched.text);
    round_trip(&text, None, "", &[remove("a")]);
}

fn forged_with(edits: &mut Vec<Edit>, t: &str, text: &str) {
    let p = t.find(text).unwrap();
    let at = anchor(t, &map(t, &Scheme::LEVEL).unwrap(), p).unwrap();
    edits.push(Edit::Delete {
        at,
        text: text.to_owned(),
    });
}

/// auditor-1-004: an op that carries edits may change nothing but what a
/// forward op changes; the kick lfo comment deleted beside a genuine edit
/// passes the gap checks and is refused by the forward-pair rule.
#[test]
fn forged_edits_are_refused() {
    let kick = read("projects/kick/project.toml");
    let lfo = "# 1/29.1 Hz: one cycle every 29.1 s.\n";
    let refused = |text: &str, op: Op| {
        let error = apply(text, None, &[op], &ctx("")).unwrap_err();
        assert_eq!(error.code, "bad-value", "{error}");
        error.message
    };
    // instance.add svf, sent as edits, with the deletion.
    let added = apply(&kick, None, &[add("svf", "std.svf")], &ctx("")).unwrap();
    let undone = apply(&added.text, None, &added.inverse, &ctx("")).unwrap();
    let Op::InstanceAdd {
        edits: Some(mut edits),
        ..
    } = undone.inverse[0].clone()
    else {
        panic!("{:?}", undone.inverse);
    };
    let mut probe = kick.clone();
    run_checked(&mut probe, &Scheme::LEVEL, &edits).unwrap();
    forged_with(&mut edits, &probe, lfo);
    let mut forged = add("svf", "std.svf");
    if let Op::InstanceAdd { edits: e, .. } = &mut forged {
        *e = Some(edits);
    }
    assert!(refused(&kick, forged).contains("not instance.add"));
    // The undo of instance.remove accent, with the deletion.
    let removed = apply(&kick, None, &[remove("accent")], &ctx("")).unwrap();
    let Op::InstanceAdd {
        edits: Some(mut edits),
        ..
    } = removed.inverse[0].clone()
    else {
        panic!();
    };
    let mut probe = removed.text.clone();
    run_checked(&mut probe, &Scheme::LEVEL, &edits).unwrap();
    forged_with(&mut edits, &probe, lfo);
    let mut forged = removed.inverse[0].clone();
    if let Op::InstanceAdd { edits: e, .. } = &mut forged {
        *e = Some(edits);
    }
    assert!(refused(&removed.text, forged).contains("not instance.add"));
    // A delete crossing a connection; a cut reaching into the next instance;
    // an insert inside an entry; bytes that differ.
    let m = map(&kick, &Scheme::LEVEL).unwrap();
    let crossing = kick[m.conns[0].range.end..m.conns[1].range.start + 5].to_owned();
    let with = |edits: Vec<Edit>| Op::ConnectionRemove {
        from: "transport.beat".into(),
        to: "drums.trigger".into(),
        edits: Some(edits),
    };
    let delete = Edit::Delete {
        at: Pos {
            item: Item::Connection(0),
            offset: 0,
        },
        text: crossing,
    };
    assert!(refused(&kick, with(vec![delete])).contains("crosses an entry"));
    let lfo_end = m
        .entries
        .iter()
        .find(|e| e.name == "lfo")
        .unwrap()
        .range
        .end;
    let accent = m
        .entries
        .iter()
        .find(|e| e.name == "accent")
        .unwrap()
        .range
        .start;
    let cut = Edit::Cut {
        item: Item::Instance("lfo".into(), 0),
        before: String::new(),
        after: kick[lfo_end..accent + 3].to_owned(),
    };
    let op = Op::InstanceRemove {
        name: "lfo".into(),
        edits: Some(vec![cut]),
        layout: None,
    };
    assert!(refused(&kick, op).contains("another entry"));
    let inside = Edit::Insert {
        at: Pos {
            item: Item::Connection(0),
            offset: 0,
        },
        text: "x".into(),
    };
    let op = Op::InstanceAdd {
        name: "svf".into(),
        module: "std.svf".into(),
        params: BTreeMap::new(),
        x: None,
        y: None,
        edits: Some(vec![
            Edit::Insert {
                at: Pos {
                    item: Item::Instance("lfo".into(), 0),
                    offset: 0,
                },
                text: "!".into(),
            },
            inside,
        ]),
        layout: None,
    };
    refused(&kick, op);
    let differ = Edit::Delete {
        at: Pos {
            item: Item::File,
            offset: 0,
        },
        text: "# not the first line\n".into(),
    };
    assert!(refused(&kick, with(vec![differ])).contains("differ"));
}

/// Layout edits are bound the same way: exactly the op's instance's
/// position at the op's level.
#[test]
fn layout_edits_touching_another_position_are_refused() {
    let text = format!("{HEAD}{INST}{MULTI}");
    let file = "[top]\na = { x = 1, y = 2 }\nb = { x = 3, y = 4 }\n";
    let removed = apply(&text, Some(file), &[remove("a")], &ctx("")).unwrap();
    let Op::InstanceAdd {
        layout: Some(layout),
        edits,
        ..
    } = removed.inverse[0].clone()
    else {
        panic!();
    };
    // The genuine undo, plus b's position deleted.
    let mut probe = removed.positions.clone().unwrap();
    let mut forged = layout.clone();
    run_checked(&mut probe, &positions_scheme(""), &forged.edits).unwrap();
    let p = probe.find("b = { x = 3, y = 4 }\n").unwrap();
    let at = anchor(&probe, &map(&probe, &positions_scheme("")).unwrap(), p).unwrap_or(Pos {
        item: Item::Instance("a".into(), 0),
        offset: 1,
    });
    forged.edits.push(Edit::Delete {
        at,
        text: "b = { x = 3, y = 4 }\n".into(),
    });
    let op = Op::InstanceAdd {
        name: "a".into(),
        module: "std.lfo".into(),
        params: BTreeMap::new(),
        x: Some(1.0),
        y: Some(2.0),
        edits,
        layout: Some(forged),
    };
    let error = apply(&removed.text, removed.positions.as_deref(), &[op], &ctx("")).unwrap_err();
    assert_eq!(error.code, "bad-value", "{error}");
    // `created` keeps a file that still holds text.
    let added = apply(
        &text,
        Some(file),
        &[Op::InstanceAdd {
            name: "svf".into(),
            module: "std.svf".into(),
            params: BTreeMap::new(),
            x: Some(0.0),
            y: Some(0.0),
            edits: None,
            layout: None,
        }],
        &ctx(""),
    )
    .unwrap();
    let mut inverse = added.inverse.clone();
    if let Op::InstanceRemove {
        layout: Some(l), ..
    } = &mut inverse[0]
    {
        l.created = true;
    }
    let undone = apply(&added.text, added.positions.as_deref(), &inverse, &ctx("")).unwrap();
    assert_eq!(undone.positions.as_deref(), Some(file));
}

/// auditor-1-002: an instance written as dotted keys among other entries
/// (its pieces apart) is removed piece by piece, and its undo and redo are
/// byte-exact; every result loads and plans.
#[test]
fn an_interleaved_dotted_instance_round_trips() {
    let instances = "[instances]\na.module = \"std.lfo\" # its module\nb = { module = \"std.gain\", db = -6 }\n# a's rate\na.freq_hz = 1\nc = { module = \"std.lag\" }\nout = { module = \"std.audio_out\", gain = 0.5 }\n";
    let text = format!("{HEAD}{instances}{MULTI}");
    plans(&text, &[]).unwrap();
    let m = map(&text, &Scheme::LEVEL).unwrap();
    assert_eq!(
        m.entries.iter().filter(|e| e.name == "a").count(),
        2,
        "two pieces"
    );
    let (after, _) = round_trip(&text, None, "", &[remove("a")]);
    assert!(
        !after.contains("a.module") && !after.contains("a.freq_hz") && !after.contains("a's rate")
    );
    assert!(
        after.contains("b = { module = \"std.gain\", db = -6 }\nc = "),
        "{after}"
    );
    plans(&after, &[]).unwrap();
    for name in ["b", "c"] {
        let (after, _) = round_trip(&text, None, "", &[remove(name)]);
        plans(&after, &[]).unwrap();
    }
    round_trip(
        &text,
        None,
        "",
        &[add("svf", "std.svf"), connect("a.out", "out.r")],
    );
}

/// auditor-1-006 / auditor-2: malformed untrusted offsets and ranges (a
/// position inside a multi-byte character, an offset that overflows, a range
/// past either end) are refused with bad-value, never a panic.
#[test]
fn malformed_offsets_are_refused_never_a_panic() {
    let kick = read("projects/kick/project.toml");
    let text = format!("# é\n{kick}");
    let refuse = |op: Op| {
        let error = apply(&text, None, &[op], &ctx("")).unwrap_err();
        assert_eq!(error.code, "bad-value", "{error}");
    };
    let add_with = |edits: Vec<Edit>| Op::InstanceAdd {
        name: "svf".into(),
        module: "std.svf".into(),
        params: BTreeMap::new(),
        x: None,
        y: None,
        edits: Some(edits),
        layout: None,
    };
    let lfo = || Item::Instance("lfo".into(), 0);
    let at = |item: Item, offset: usize| Pos { item, offset };
    // An insert at byte 3 splits é.
    refuse(add_with(vec![Edit::Insert {
        at: at(Item::File, 3),
        text: "x".into(),
    }]));
    // Offsets that overflow.
    refuse(add_with(vec![Edit::Insert {
        at: at(lfo(), usize::MAX),
        text: "x".into(),
    }]));
    refuse(add_with(vec![Edit::Delete {
        at: at(lfo(), usize::MAX),
        text: "x".into(),
    }]));
    // A delete starting on ± (kick's "decay 420 ms ± 120 ms") whose one byte ends inside it.
    let p = text.find('±').unwrap();
    let m = map(&text, &Scheme::LEVEL).unwrap();
    let on = anchor(&text, &m, p).unwrap();
    refuse(add_with(vec![Edit::Delete {
        at: on.clone(),
        text: "x".into(),
    }]));
    // A delete starting inside ±.
    refuse(add_with(vec![Edit::Delete {
        at: Pos {
            offset: on.offset + 1,
            ..on
        },
        text: "x".into(),
    }]));
    // A delete past the end; a cut whose before reaches before the file.
    refuse(add_with(vec![Edit::Delete {
        at: at(Item::File, text.len() - 1),
        text: "xx".into(),
    }]));
    refuse(Op::InstanceRemove {
        name: "lfo".into(),
        edits: Some(vec![Edit::Cut {
            item: lfo(),
            before: "x".repeat(text.len() + 1),
            after: String::new(),
        }]),
        layout: None,
    });
    refuse(Op::InstanceRemove {
        name: "lfo".into(),
        edits: Some(vec![Edit::Cut {
            item: lfo(),
            before: String::new(),
            after: "x".repeat(text.len()),
        }]),
        layout: None,
    });
    // Items: a piece number reads back, a bad one is refused.
    assert!("instance:a#x".parse::<Item>().is_err());
    assert_eq!(
        "instance:a#2".parse::<Item>().unwrap(),
        Item::Instance("a".into(), 2)
    );
    assert_eq!(Item::Instance("a".into(), 0).to_string(), "instance:a");
}
