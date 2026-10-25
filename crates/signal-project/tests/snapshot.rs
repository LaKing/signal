//! ADR 0041: a project's snapshots in `snapshots.toml`, saved, overwritten
//! and deleted through the ADR 0040 writer, the file byte-stable whatever
//! order the snapshots were saved in, names with spaces and accents, a
//! change by another writer refused as a conflict with nothing lost, and the
//! file packed into a `.signal` project. In temporary folders.

use std::fs;
use std::path::PathBuf;

use signal_project::archive;
use signal_project::snapshot::{self, SNAPSHOTS_FILE, SnapshotError, Values};

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-snapshot-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n\n[project]\nname = \"s\"\nsample_rate = 48000\nseed = 1\n\n[instances]\nout = { module = \"std.audio_out\", gain = 0.5 }\n",
    )
    .unwrap();
    dir
}

fn values(pairs: &[(&str, f64)]) -> Values {
    pairs.iter().map(|(a, v)| ((*a).to_owned(), *v)).collect()
}

fn ok(_: Option<&str>) -> Result<(), SnapshotError> {
    Ok(())
}

#[test]
fn save_overwrite_and_delete() {
    let dir = folder("save");
    let path = dir.join(SNAPSHOTS_FILE);
    assert_eq!(
        snapshot::read(&dir).unwrap().0.snapshots.len(),
        0,
        "no file is no snapshots"
    );
    let (replaced, text, wrote) = snapshot::save(
        &dir,
        "  Verse A  ",
        values(&[("/out.gain", 0.25), ("/osc.freq", 440.0)]),
        false,
        ok,
    )
    .unwrap();
    assert!(!replaced);
    assert!(wrote);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert!(
        text.contains("[snapshots.\"Verse A\"]\n\"/osc.freq\" = 440\n\"/out.gain\" = 0.25\n"),
        "{text}"
    );

    // A name in use: refused unless overwriting.
    let refused = snapshot::save(&dir, "Verse A", values(&[("/out.gain", 1.0)]), false, ok);
    assert_eq!(refused, Err(SnapshotError::Exists("Verse A".to_owned())));
    assert_eq!(fs::read_to_string(&path).unwrap(), text, "nothing written");
    let (replaced, ..) =
        snapshot::save(&dir, "Verse A", values(&[("/out.gain", 1.0)]), true, ok).unwrap();
    assert!(replaced);
    let (all, _) = snapshot::read(&dir).unwrap();
    assert_eq!(all.snapshots["Verse A"], values(&[("/out.gain", 1.0)]));

    // Accents, spaces, quotes: any printable name reads back as given.
    for name in ["Árvíztűrő tükörfúrógép", "a \"quoted\" one", "チャンネル 1"] {
        snapshot::save(&dir, name, values(&[("/out.gain", 0.5)]), false, ok).unwrap();
    }
    let (all, _) = snapshot::read(&dir).unwrap();
    assert_eq!(
        all.snapshots.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "Verse A",
            "a \"quoted\" one",
            "Árvíztűrő tükörfúrógép",
            "チャンネル 1"
        ],
        "byte order"
    );

    let (deleted, ..) = snapshot::delete(&dir, "Árvíztűrő tükörfúrógép", ok).unwrap();
    assert_eq!(deleted, values(&[("/out.gain", 0.5)]));
    assert_eq!(
        snapshot::delete(&dir, "Árvíztűrő tükörfúrógép", ok),
        Err(SnapshotError::Unknown("Árvíztűrő tükörfúrógép".to_owned()))
    );
    for name in ["Verse A", "a \"quoted\" one", "チャンネル 1"] {
        snapshot::delete(&dir, name, ok).unwrap();
    }
    // The last one gone: the file stays, with no snapshots.
    let empty = fs::read_to_string(&path).unwrap();
    assert!(empty.ends_with("schema_version = 1\n"), "{empty:?}");
    assert!(snapshot::read(&dir).unwrap().0.snapshots.is_empty());
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "no staging file left: {leftovers:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// The same snapshots in the same order give the same bytes, however they
/// got there (saved in another order, then moved); saving the same values
/// again writes nothing.
#[test]
fn the_file_is_byte_stable() {
    let a = folder("stable-a");
    let b = folder("stable-b");
    let one = values(&[("/z.p", 1.5), ("/a.p", -2.0), ("/m.q", 1e-9)]);
    let two = values(&[("/out.gain", 0.1 + 0.2)]);
    snapshot::save(&a, "one", one.clone(), false, ok).unwrap();
    snapshot::save(&a, "two", two.clone(), false, ok).unwrap();
    snapshot::save(&b, "two", two.clone(), false, ok).unwrap();
    snapshot::save(&b, "one", one.clone(), false, ok).unwrap();
    assert!(
        fs::read_to_string(b.join(SNAPSHOTS_FILE))
            .unwrap()
            .contains("order = [\"two\", \"one\"]"),
        "a new one goes to the end"
    );
    let (to, _, wrote) = snapshot::move_to(&b, "one", 0, ok).unwrap();
    assert_eq!((to, wrote), (0, true));
    let text = fs::read(a.join(SNAPSHOTS_FILE)).unwrap();
    assert_eq!(text, fs::read(b.join(SNAPSHOTS_FILE)).unwrap());
    let (_, now, wrote) = snapshot::save(&a, "one", one, true, ok).unwrap();
    assert!(!wrote, "the same text: nothing written");
    assert_eq!(now.as_bytes(), text, "the text read under the lock");
    assert_eq!(fs::read(a.join(SNAPSHOTS_FILE)).unwrap(), text);
    let (all, _) = snapshot::read(&a).unwrap();
    assert_eq!(
        all.snapshots["two"]["/out.gain"].to_bits(),
        (0.1f64 + 0.2).to_bits(),
        "values read back bit for bit"
    );
    let _ = fs::remove_dir_all(&a);
    let _ = fs::remove_dir_all(&b);
}

/// ADR 0040: another writer that changes the file after this one read it
/// (an editor, which takes no lock) makes the save a conflict; its edit is
/// kept and nothing of this save is written. A `base` check refuses too.
#[test]
fn a_change_meanwhile_is_a_conflict() {
    let dir = folder("conflict");
    let path = dir.join(SNAPSHOTS_FILE);
    snapshot::save(&dir, "first", values(&[("/out.gain", 0.5)]), false, ok).unwrap();
    let theirs = fs::read_to_string(&path).unwrap().replace("0.5", "0.125");
    let refused = snapshot::save(
        &dir,
        "second",
        values(&[("/out.gain", 0.25)]),
        false,
        |_| {
            fs::write(&path, &theirs).unwrap();
            Ok(())
        },
    );
    assert!(
        matches!(refused, Err(SnapshotError::Changed(_))),
        "{refused:?}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        theirs,
        "their edit kept"
    );
    let refused = snapshot::delete(&dir, "first", |_| {
        Err(SnapshotError::Changed("base".to_owned()))
    });
    assert_eq!(refused.unwrap_err().code(), "conflict");
    assert!(
        snapshot::read(&dir)
            .unwrap()
            .0
            .snapshots
            .contains_key("first")
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A file that does not read is never overwritten; a link is not written
/// through.
#[test]
fn a_bad_file_or_a_link_is_refused() {
    let dir = folder("bad");
    let path = dir.join(SNAPSHOTS_FILE);
    fs::write(
        &path,
        "schema_version = 1\n[snapshots.a]\n\"/out.gain\" = \"x\"\n",
    )
    .unwrap();
    let refused = snapshot::save(&dir, "b", values(&[("/out.gain", 0.5)]), false, ok);
    assert_eq!(refused.unwrap_err().code(), "does-not-load");
    assert!(fs::read_to_string(&path).unwrap().contains("\"x\""));
    fs::remove_file(&path).unwrap();
    let outside = dir.join("elsewhere.toml");
    fs::write(&outside, "").unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let refused = snapshot::save(&dir, "b", values(&[("/out.gain", 0.5)]), false, ok);
    assert_eq!(refused.unwrap_err().code(), "write-failed");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "");
    // Never read through either: the target's text is not quoted anywhere.
    fs::write(&outside, "secret = \"do not show\"\n").unwrap();
    let read = snapshot::read(&dir).unwrap_err();
    assert_eq!(read.code(), "bad-value");
    assert!(!read.to_string().contains("do not show"), "{read}");
    let _ = fs::remove_dir_all(&dir);
}

/// The snapshots are part of the project: a `.signal` file packs them.
#[test]
fn a_signal_file_packs_the_snapshots() {
    let dir = folder("pack");
    snapshot::save(&dir, "Verse A", values(&[("/out.gain", 0.5)]), false, ok).unwrap();
    let packed = archive::pack(&dir).unwrap();
    let into =
        std::env::temp_dir().join(format!("signal-snapshot-unpacked-{}", std::process::id()));
    let _ = fs::remove_dir_all(&into);
    archive::unpack(&packed, &into).unwrap();
    assert_eq!(
        fs::read(into.join(SNAPSHOTS_FILE)).unwrap(),
        fs::read(dir.join(SNAPSHOTS_FILE)).unwrap()
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&into);
}

/// ADR 0041 §9 (the owner's request of 2026-10-25): the person's order of
/// the snapshots, as text: a new one at the end, an overwrite keeping its
/// place, a move, a delete leaving the order, the undo snapshot outside it,
/// a stale name dropped on the next write, a stale base a conflict.
#[test]
fn the_order_is_kept_in_the_file() {
    let dir = folder("order");
    let path = dir.join(SNAPSHOTS_FILE);
    for name in ["Verse A", "chorus", "Bridge"] {
        snapshot::save(&dir, name, values(&[("/out.gain", 0.5)]), false, ok).unwrap();
    }
    let names = |dir: &std::path::Path| snapshot::read(dir).unwrap().0.names();
    assert_eq!(names(&dir), ["Verse A", "chorus", "Bridge"]);
    snapshot::save(&dir, "chorus", values(&[("/out.gain", 1.0)]), true, ok).unwrap();
    assert_eq!(
        names(&dir),
        ["Verse A", "chorus", "Bridge"],
        "an overwrite keeps its place"
    );
    snapshot::move_to(&dir, "Bridge", 0, ok).unwrap();
    assert_eq!(names(&dir), ["Bridge", "Verse A", "chorus"]);
    // A stale base: a conflict, nothing moved.
    let stale = snapshot::move_to(&dir, "chorus", 0, |_| {
        Err(SnapshotError::Changed("base".to_owned()))
    });
    assert_eq!(stale.unwrap_err().code(), "conflict");
    assert_eq!(names(&dir), ["Bridge", "Verse A", "chorus"]);
    snapshot::delete(&dir, "Verse A", ok).unwrap();
    assert_eq!(names(&dir), ["Bridge", "chorus"]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("order"),
        "byte order again: the key goes: {text}"
    );
    // The undo snapshot is no part of the order.
    snapshot::edit(&dir, ok, |all| {
        all.undo = Some(values(&[("/out.gain", 0.25)]));
        Ok(())
    })
    .unwrap();
    assert_eq!(names(&dir), ["Bridge", "chorus"]);
    // A hand edit naming a snapshot that is not there: read without it,
    // dropped on the next write.
    let edited = fs::read_to_string(&path).unwrap().replace(
        "schema_version = 1\n",
        "schema_version = 1\norder = [\"gone\", \"chorus\", \"Bridge\"]\n",
    );
    fs::write(&path, edited).unwrap();
    assert_eq!(names(&dir), ["chorus", "Bridge"]);
    snapshot::save(&dir, "Coda", values(&[("/out.gain", 0.5)]), false, ok).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("order = [\"chorus\", \"Bridge\", \"Coda\"]\n"),
        "{text}"
    );
    assert!(!text.contains("gone"), "{text}");
    // A duplicate is refused, and the file is never overwritten.
    let duplicate = text.replace(
        "order = [\"chorus\", \"Bridge\", \"Coda\"]",
        "order = [\"Bridge\", \"Bridge\"]",
    );
    fs::write(&path, &duplicate).unwrap();
    assert_eq!(
        snapshot::move_to(&dir, "Coda", 0, ok).unwrap_err().code(),
        "does-not-load"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), duplicate);
    let _ = fs::remove_dir_all(&dir);
}

/// ADR 0041 §8: `snapshot_exclude` in an instance entry: written and
/// removed with only its own text changing, read back by the project,
/// checked against the instance's parameters, refused twice, and no change
/// to what plays (the M2-2 diff is empty).
#[test]
fn snapshot_exclude_is_text_next_to_the_control() {
    use signal_project::{Project, ProjectEdit, SourceFile, write_snapshot_exclude};
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    let text = "schema_version = 1\n\n[project]\nname = \"s\"\nsample_rate = 48000\nseed = 1\n\n[instances]\nosc = { module = \"std.sine_osc\", freq = 440 } # the tone\nout = { module = \"std.audio_out\", gain = 0.5 }\n\n[wiring]\nconnections = [{ from = \"osc.out\", to = \"out.l\" }]\n";
    let excluded = write_snapshot_exclude(text, "osc", "freq", true)
        .unwrap()
        .unwrap();
    assert_eq!(
        excluded,
        text.replace(
            "freq = 440 }",
            "freq = 440, snapshot_exclude = [\"freq\"] }"
        )
    );
    assert_eq!(
        write_snapshot_exclude(&excluded, "osc", "freq", true).unwrap(),
        None,
        "already so"
    );
    let load = |text: &str| {
        Project::from_sources(vec![SourceFile {
            path: "project.toml".to_owned(),
            text: text.to_owned(),
        }])
    };
    let before = load(text).unwrap();
    let after = load(&excluded).unwrap();
    assert!(after.plan(&registry).is_ok());
    assert_eq!(
        after
            .snapshot_excluded("osc")
            .into_iter()
            .collect::<Vec<_>>(),
        ["freq"]
    );
    assert!(before.snapshot_excluded("osc").is_empty());
    assert!(
        matches!(
            signal_project::diff(&before, &after, &registry).unwrap(),
            ProjectEdit::Parameters(changes) if changes.is_empty()
        ),
        "nothing that plays changes"
    );
    let back = write_snapshot_exclude(&excluded, "osc", "freq", false)
        .unwrap()
        .unwrap();
    assert_eq!(back, text, "back to the text it was");

    let wrong = text.replace(
        "freq = 440 }",
        "freq = 440, snapshot_exclude = [\"nope\"] }",
    );
    let refused = load(&wrong).unwrap().plan(&registry).err().unwrap();
    assert!(
        refused[0]
            .message
            .contains("no parameter of `std.sine_osc`"),
        "{refused:?}"
    );
    let twice = text.replace(
        "freq = 440 }",
        "freq = 440, snapshot_exclude = [\"freq\", \"freq\"] }",
    );
    assert!(load(&twice).is_err());
}
