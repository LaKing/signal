//! M2.8-12 project files (`.signal`, ADR 0025): `pack` and `unpack` on
//! copies of the committed projects in temporary folders. The round trip
//! keeps every file a project reads, the same folder always packs to the
//! same bytes, what a project never reads stays out, and every refused
//! archive names its reason and leaves nothing behind.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use signal_project::Project;
use signal_project::archive::{
    ArchiveError, Limits, TempProject, pack, unpack, unpack_with, write_atomic,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

fn projects() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects")
}

/// A folder of its own for each test.
fn base(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-archive-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every file under `dir`, by its `/`-separated path, with its bytes.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &format!("{name}/"), out);
            } else {
                out.push((name, fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

fn copy_tree(from: &Path, to: &Path) {
    for (name, bytes) in tree(from) {
        let path = to.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn names(bytes: &[u8]) -> Vec<String> {
    let archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    archive.file_names().map(str::to_owned).collect()
}

/// An archive with these entries, written as given (no checks of ours).
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, SimpleFileOptions::DEFAULT)
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

const PROJECT: &[u8] = b"schema_version = 1\n";

/// Replaces the first `from` in `bytes` (local and central headers both, by
/// replacing every occurrence) with `to`, of the same length.
fn patched(mut bytes: Vec<u8>, from: &[u8], to: &[u8]) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let mut at = 0;
    while let Some(i) = bytes[at..].windows(from.len()).position(|w| w == from) {
        bytes[at + i..at + i + from.len()].copy_from_slice(to);
        at += i + from.len();
    }
    bytes
}

/// Sets the Unix mode of the central directory entry named `name`.
fn with_mode(mut bytes: Vec<u8>, name: &str, mode: u32) -> Vec<u8> {
    let mut at = 0;
    while let Some(i) = bytes[at..].windows(4).position(|w| w == b"PK\x01\x02") {
        let header = at + i;
        let len = u16::from_le_bytes([bytes[header + 28], bytes[header + 29]]) as usize;
        if &bytes[header + 46..header + 46 + len] == name.as_bytes() {
            bytes[header + 5] = 3; // made by Unix
            bytes[header + 38..header + 42].copy_from_slice(&(mode << 16).to_le_bytes());
            return bytes;
        }
        at = header + 4;
    }
    panic!("{name} is not in the central directory");
}

/// The refusal of `bytes`, which must leave `into` unmade.
fn refused(name: &str, bytes: &[u8]) -> ArchiveError {
    refused_with(name, bytes, Limits::DEFAULT)
}

fn refused_with(name: &str, bytes: &[u8], limits: Limits) -> ArchiveError {
    let into = base(name).join("out");
    let error = unpack_with(bytes, &into, limits).unwrap_err();
    assert!(!into.exists(), "{name}: a refused archive opens nothing");
    assert!(!error.to_string().is_empty());
    error
}

/// AC1, AC9: every committed project round-trips: the unpacked folder has
/// exactly the files a project reads, byte for byte, and loads the same.
#[test]
fn every_project_round_trips() {
    for project in ["kick", "pulse", "physical", "physical-av"] {
        let dir = projects().join(project);
        let bytes = pack(&dir).unwrap();
        let into = base(&format!("round-{project}")).join(project);
        let unpacked = unpack(&bytes, &into).unwrap();
        assert_eq!(tree(&into), tree(&dir), "{project}");
        assert_eq!(unpacked.files.len(), tree(&dir).len());
        assert!(unpacked.files.contains(&"project.toml".to_owned()));
        let a = Project::load(&dir).unwrap();
        let b = Project::load(&into).unwrap();
        assert_eq!(a.files, b.files, "{project}");
        assert_eq!(
            pack(&into).unwrap(),
            bytes,
            "{project}: packs to the same bytes again"
        );
    }
}

/// AC2: deflate, sorted paths, a fixed time and fixed permissions: a copy
/// with other times and modes packs to the same bytes.
#[test]
fn the_same_folder_gives_the_same_bytes() {
    let kick = projects().join("kick");
    let first = pack(&kick).unwrap();
    assert_eq!(pack(&kick).unwrap(), first);

    let copy = base("determinism").join("elsewhere");
    copy_tree(&kick, &copy);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let toml = copy.join("project.toml");
    fs::write(&toml, fs::read(&toml).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&toml, fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(
        pack(&copy).unwrap(),
        first,
        "times, modes and the folder's place do not count"
    );

    let listed = names(&first);
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(listed, sorted, "sorted path order");
    let mut archive = ZipArchive::new(Cursor::new(first.as_slice())).unwrap();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).unwrap();
        assert_eq!(
            entry.compression(),
            CompressionMethod::Deflated,
            "{}",
            entry.name()
        );
        assert_eq!(entry.last_modified(), Some(DateTime::DEFAULT));
        assert_eq!(entry.unix_mode(), Some(0o100_644));
    }
}

/// AC3: hidden files, renders/, sessions/ and what a project never reads
/// stay out of the archive.
#[test]
fn what_a_project_never_reads_stays_out() {
    let dir = base("exclusions").join("p");
    copy_tree(&projects().join("kick"), &dir);
    for (path, text) in [
        (".DS_Store", "x"),
        (".git/HEAD", "ref"),
        ("renders/a/audio.wav", "x"),
        ("sessions/s/session.toml", "x"),
        ("notes.txt", "x"),
        ("composites/readme.md", "x"),
        ("composites/nested/x.toml", "x"),
        ("ui/.DS_Store", "x"),
        ("ui/assets/.hidden.png", "x"),
        ("kick.signal", "x"),
    ] {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let listed = names(&pack(&dir).unwrap());
    assert_eq!(listed, names(&pack(&projects().join("kick")).unwrap()));
    assert_eq!(
        listed,
        [
            "composites/drums.toml",
            "composites/kick.toml",
            "project.toml",
            "ui/assets/knob.svg",
            "ui/main.html"
        ]
    );
}

/// Packing refuses a folder without project.toml and a symbolic link.
#[test]
fn packing_needs_project_toml_and_refuses_links() {
    let empty = base("pack-empty");
    assert!(matches!(pack(&empty), Err(ArchiveError::NoProjectFile)));
    #[cfg(unix)]
    {
        let dir = base("pack-link").join("p");
        copy_tree(&projects().join("pulse"), &dir);
        std::os::unix::fs::symlink("/etc/hosts", dir.join("ui/assets/hosts.png")).unwrap();
        assert!(
            matches!(pack(&dir), Err(ArchiveError::Symlink(name)) if name == "ui/assets/hosts.png")
        );
    }
}

/// AC4: '..', absolute paths, symlinks and device files are refused.
#[test]
fn reading_accepts_only_relative_regular_files() {
    let traversal = patched(
        archive(&[("project.toml", PROJECT), ("zz/evil", b"x")]),
        b"zz/evil",
        b"../evil",
    );
    assert!(matches!(refused("dotdot", &traversal), ArchiveError::UnsafePath(n) if n == "../evil"));

    let absolute = patched(
        archive(&[("project.toml", PROJECT), ("xetc/passwd", b"x")]),
        b"xetc/passwd",
        b"/etc/passwd",
    );
    assert!(
        matches!(refused("absolute", &absolute), ArchiveError::UnsafePath(n) if n == "/etc/passwd")
    );

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("project.toml", SimpleFileOptions::DEFAULT)
        .unwrap();
    writer.write_all(PROJECT).unwrap();
    writer
        .add_symlink("ui/link", "/etc/passwd", SimpleFileOptions::DEFAULT)
        .unwrap();
    let link = writer.finish().unwrap().into_inner();
    assert!(matches!(refused("symlink", &link), ArchiveError::Symlink(n) if n == "ui/link"));

    let device = with_mode(
        archive(&[("project.toml", PROJECT), ("ui/tty", b"")]),
        "ui/tty",
        0o020_644,
    );
    assert!(matches!(refused("device", &device), ArchiveError::SpecialFile(n) if n == "ui/tty"));
    let fifo = with_mode(
        archive(&[("project.toml", PROJECT), ("ui/fifo", b"")]),
        "ui/fifo",
        0o010_644,
    );
    assert!(matches!(
        refused("fifo", &fifo),
        ArchiveError::SpecialFile(_)
    ));
}

/// AC4: two entries that APFS takes for one file are refused.
#[test]
fn a_name_twice_in_any_case_is_refused() {
    let twice = archive(&[
        ("project.toml", PROJECT),
        ("ui/A.html", b"a"),
        ("ui/a.html", b"b"),
    ]);
    assert!(matches!(refused("case", &twice), ArchiveError::Duplicate(n) if n == "ui/a.html"));
}

/// AC5: the bounds, 10 000 entries and 1 GiB, and each enforced.
#[test]
fn reading_enforces_the_entry_and_size_bounds() {
    assert_eq!(
        Limits::DEFAULT,
        Limits {
            entries: 10_000,
            bytes: 1 << 30
        }
    );
    let three = archive(&[("project.toml", PROJECT), ("ui/a", b"a"), ("ui/b", b"b")]);
    let small = Limits {
        entries: 2,
        bytes: 1 << 20,
    };
    assert!(matches!(
        refused_with("entries", &three, small),
        ArchiveError::TooManyEntries { count: 3, max: 2 }
    ));
    let big = archive(&[("project.toml", PROJECT), ("ui/big", &[0u8; 4096])]);
    let tight = Limits {
        entries: 10,
        bytes: 1024,
    };
    assert!(matches!(
        refused_with("bytes", &big, tight),
        ArchiveError::TooLarge { max: 1024 }
    ));
    // A declared size that lies is caught while writing: still nothing left.
    let lying = patched(
        archive(&[("project.toml", PROJECT), ("ui/big", &[7u8; 4096])]),
        &4096u32.to_le_bytes(),
        &16u32.to_le_bytes(),
    );
    let into = base("lying").join("out");
    let error = unpack_with(
        &lying,
        &into,
        Limits {
            entries: 10,
            bytes: 100,
        },
    )
    .unwrap_err();
    assert!(!into.exists(), "{error}");
}

/// AC6: project.toml at the archive's root, as a file.
#[test]
fn reading_requires_project_toml_at_the_root() {
    let nested = archive(&[("kick/project.toml", PROJECT)]);
    assert!(matches!(
        refused("nested", &nested),
        ArchiveError::NoProjectFile
    ));
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_directory("project.toml/", SimpleFileOptions::DEFAULT)
        .unwrap();
    let folder = writer.finish().unwrap().into_inner();
    assert!(matches!(
        refused("folder", &folder),
        ArchiveError::NoProjectFile
    ));
}

/// AC7: what is not an archive, and a folder that is not empty, are refused
/// with the reason named; a non-empty folder is left as it was.
#[test]
fn a_refusal_names_its_reason() {
    let garbage = refused("garbage", b"not a zip at all");
    assert!(
        garbage.to_string().starts_with("not a .signal archive"),
        "{garbage}"
    );
    let into = base("not-empty");
    fs::write(into.join("keep.txt"), "mine").unwrap();
    let bytes = pack(&projects().join("pulse")).unwrap();
    assert!(matches!(
        unpack(&bytes, &into),
        Err(ArchiveError::NotEmpty(_))
    ));
    assert_eq!(tree(&into), [("keep.txt".to_owned(), b"mine".to_vec())]);
    // An empty folder is unpacked into.
    let empty = base("empty-folder");
    unpack(&bytes, &empty).unwrap();
    assert_eq!(tree(&empty), tree(&projects().join("pulse")));
}

/// AC18's write: atomic, the old file replaced, no temporary left.
#[test]
fn writing_is_atomic_and_leaves_no_temporary() {
    let dir = base("atomic");
    let file = dir.join("kick.signal");
    fs::write(&file, "old").unwrap();
    let bytes = pack(&projects().join("kick")).unwrap();
    write_atomic(&file, &bytes).unwrap();
    assert_eq!(fs::read(&file).unwrap(), bytes);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "only the file");
}

/// auditor-1-atomic-temp-symlink (AC18): an entry already at the temporary
/// file's name (here a link to the file itself) is never followed or reused:
/// the file is written in full and stays a regular file, and the entry is
/// left as it was.
#[cfg(unix)]
#[test]
fn an_entry_at_the_temporary_name_is_left_alone() {
    let dir = base("atomic-link");
    let file = dir.join("kick.signal");
    fs::write(&file, "old").unwrap();
    let planted = dir.join(format!(".kick.signal.{}.tmp", std::process::id()));
    std::os::unix::fs::symlink(&file, &planted).unwrap();
    let bytes = pack(&projects().join("kick")).unwrap();
    write_atomic(&file, &bytes).unwrap();
    assert!(fs::symlink_metadata(&file).unwrap().file_type().is_file());
    assert_eq!(fs::read(&file).unwrap(), bytes);
    assert!(
        fs::symlink_metadata(&planted)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&planted).unwrap(), file);
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        2,
        "the file and the planted link"
    );
}

/// AC11: a temporary unpack for one run, removed when dropped.
#[test]
fn a_temporary_project_goes_with_its_run() {
    let file = base("temp").join("pulse.signal");
    write_atomic(&file, &pack(&projects().join("pulse")).unwrap()).unwrap();
    let temp = TempProject::unpack(&file).unwrap();
    let dir = temp.dir().to_owned();
    assert!(dir.join("project.toml").is_file());
    let mut text = String::new();
    fs::File::open(dir.join("project.toml"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(!text.is_empty());
    drop(temp);
    assert!(!dir.exists());
}
