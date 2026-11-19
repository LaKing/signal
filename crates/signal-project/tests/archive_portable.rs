//! The review of 4d37bf8: an archive `pack --embed` writes renders from its
//! unpacked copy alone, and the staged (streamed) pack is the same archive.
use signal_project::{Project, archive};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn dir(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("archive-portable-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn write(p: &Path, include: Option<&str>) {
    fs::create_dir_all(p).unwrap();
    let inc = include
        .map(|i| format!("inc = {{ project = {i:?} }}\n"))
        .unwrap_or_default();
    fs::write(
        p.join("project.toml"),
        format!(
            "schema_version = 1\n[project]\nname = \"portable\"\nseed = 7\n[instances]\n{inc}out = {{ module = \"std.audio_out\" }}\n"
        ),
    )
    .unwrap();
}

/// A host's inside child includes a project from outside the host: the
/// embedded archive holds it, and loads with the original gone.
#[test]
fn an_inside_include_s_outside_include_is_embedded() {
    let base = dir("nested");
    let host = base.join("host");
    write(&host, Some("child"));
    write(&host.join("child"), Some("../../leaf"));
    write(&base.join("leaf"), None);
    Project::load(&host).unwrap();
    assert!(archive::pack(&host).is_err());
    let bytes = archive::pack_embedded(&host, None).unwrap();
    fs::remove_dir_all(base.join("leaf")).unwrap();
    let dest = base.join("unpacked");
    let unpacked = archive::unpack(&bytes, &dest).unwrap();
    assert!(
        unpacked
            .files
            .iter()
            .any(|f| f.starts_with("includes/") && f.ends_with("/project.toml")),
        "{:?}",
        unpacked.files
    );
    let child = fs::read_to_string(dest.join("child/project.toml")).unwrap();
    assert!(child.contains("project = \"../includes/inc\""), "{child}");
    Project::load(&dest).unwrap();
}

/// An absolute path to a project inside the folder: plain `pack` refuses
/// it, `--embed` writes it relative, and the copy reads its own.
#[test]
fn an_absolute_inside_include_is_written_relative() {
    let base = dir("absolute");
    let host = base.join("host");
    let child = fs::canonicalize(&base).unwrap().join("host/child");
    write(&host, Some(child.to_str().unwrap()));
    write(&child, None);
    Project::load(&host).unwrap();
    let refused = archive::pack(&host).unwrap_err().to_string();
    assert!(refused.contains("an absolute path"), "{refused}");
    let bytes = archive::pack_embedded(&host, None).unwrap();
    let dest = base.join("unpacked");
    archive::unpack(&bytes, &dest).unwrap();
    let text = fs::read_to_string(dest.join("project.toml")).unwrap();
    assert!(text.contains("project = \"child\""), "{text}");
    fs::remove_dir_all(&child).unwrap();
    Project::load(&dest).unwrap();
}

/// The staged pack streams the same archive [`archive::pack`] and
/// [`archive::pack_embedded`] give in memory.
#[test]
fn a_staged_pack_is_the_same_archive() {
    let base = dir("staged");
    let host = base.join("host");
    write(&host, Some("../outside"));
    write(&base.join("outside"), None);
    let file = base.join("host.signal");
    archive::pack_staged(&host, None, true, &file)
        .unwrap()
        .replace(&file)
        .unwrap();
    assert_eq!(
        fs::read(&file).unwrap(),
        archive::pack_embedded(&host, None).unwrap()
    );
    let plain = base.join("plain");
    write(&plain, None);
    let id = "0123456789abcdef0123456789abcdef";
    let staged = archive::pack_staged(&plain, Some(id), false, &file).unwrap();
    // Not yet renamed: the file is still the last one.
    assert_eq!(
        fs::read(&file).unwrap(),
        archive::pack_embedded(&host, None).unwrap()
    );
    staged.replace(&file).unwrap();
    assert_eq!(
        fs::read(&file).unwrap(),
        archive::pack_identified(&plain, Some(id)).unwrap()
    );
    assert_eq!(
        fs::read_dir(&base).unwrap().count(),
        4,
        "no staging file is left"
    );
}
