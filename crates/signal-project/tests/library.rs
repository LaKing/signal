//! M2.10-3 (ADR 0027 §4): the built-in library. `lib.<name>` resolves to a
//! library composite read under the loader's rules; a project composite
//! does not shadow one (AC3); library composites use std.* and lib.* only
//! and a cycle among them is refused (AC4); `signal vendor` copies one, with
//! the library composites it uses, and the project renders bit-identically
//! (AC6); the render manifest, archives and sessions record what was used
//! (AC7). Projects are written to temporary folders or held in memory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig, Provenance};
use signal_output::render_blocks;
use signal_project::archive::{library_version, pack, packs_to, unpack};
use signal_project::session::{Session, SessionRecorder};
use signal_project::vendor::{VendorError, vendor};
use signal_project::{Diagnostic, Library, Project, SourceFile};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

fn sources(files: &[(&str, &str)]) -> Vec<SourceFile> {
    files
        .iter()
        .map(|(path, text)| SourceFile {
            path: (*path).to_owned(),
            text: (*text).to_owned(),
        })
        .collect()
}

/// A project file with these instance and connection lines.
fn project_text(instances: &str, connections: &str) -> String {
    format!(
        "schema_version = 1\n\n[project]\nname = \"library\"\nsample_rate = 48000\nseed = 9\n\n\
         [instances]\n{instances}out = {{ module = \"std.audio_out\", gain = 0.25 }}\n\n\
         [wiring]\nconnections = [\n{connections}]\n"
    )
}

fn load_errors(files: &[(&str, &str)], library: &Library) -> Vec<Diagnostic> {
    match Project::from_sources_with(sources(files), library) {
        Ok(_) => panic!("loaded"),
        Err(diagnostics) => diagnostics,
    }
}

fn plan_errors(project: &Project) -> Vec<Diagnostic> {
    match project.plan(&registry()) {
        Ok(_) => panic!("planned"),
        Err(diagnostics) => diagnostics,
    }
}

fn messages(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A folder of its own for each test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-library-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
}

/// The audio and log hashes of 4 800 frames of `project`.
fn render(project: &Project) -> (u64, u64) {
    let registry = registry();
    let plan = project
        .plan(&registry)
        .unwrap_or_else(|d| panic!("{}", messages(&d)))
        .plan;
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let rendered = render_blocks(&mut engine, 4_800, 64, |_| Ok(None)).unwrap();
    (rendered.audio_fnv1a64(), rendered.log_fnv1a64())
}

const INNER: &str = r#"schema_version = 1

[composite]
name = "inner"
category = "core"
description = "Scales its input by amount."

[instances]
g = { module = "std.scale", mul = 0.5, add = 0 }

[exports]
inputs = { in = "g.in" }
outputs = { out = "g.out" }
parameters = { amount = "g.mul" }
"#;

const OUTER: &str = r#"schema_version = 1

# Noise through two inner composites.
[composite]
name = "outer"
category = "generator"
description = "Noise, twice scaled; level sets the second."

[instances]
noise = { module = "std.noise" }
a = { module = "lib.inner", amount = 0.25 }
b = { module = "lib.inner" }
sum = { module = "std.add" }

[wiring]
connections = [
  { from = "noise.out", to = "a.in" },
  { from = "noise.out", to = "b.in" },
  { from = "a.out", to = "sum.a" },
  { from = "b.out", to = "sum.b" },
]

[exports]
outputs = { out = "sum.out" }
parameters = { level = "b.amount" }
"#;

/// A library of `outer`, which uses `inner` twice.
fn nested() -> Library {
    Library::new([
        ("inner".to_owned(), INNER.to_owned()),
        ("outer".to_owned(), OUTER.to_owned()),
    ])
}

/// AC2: `lib.<name>` resolves to the library composite, transitively; the
/// library's files are neither project files nor project composites.
#[test]
fn a_lib_reference_resolves_to_the_library_composite() {
    let text = project_text(
        "x = { module = \"lib.outer\", level = 0.1 }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let project =
        Project::from_sources_with(sources(&[("project.toml", &text)]), &nested()).unwrap();
    let names: Vec<&str> = project.library.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["lib.inner", "lib.outer"]);
    assert_eq!(project.files.len(), 1);
    assert!(project.composites.is_empty());
    assert_eq!(project.library[1].source.path, "signal-lib:lib/outer.toml");
    let plan = project.plan(&registry()).unwrap().plan;
    let addresses: Vec<&str> = plan.instances.iter().map(|i| i.address.as_str()).collect();
    assert!(addresses.contains(&"/x/a/g") && addresses.contains(&"/x/noise"));

    // The built-in library: every `signal-lib` composite by `lib.<name>`.
    let text = project_text(
        "x = { module = \"lib.crossfade\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let project = Project::from_sources(sources(&[("project.toml", &text)])).unwrap();
    assert_eq!(project.library[0].name, "lib.crossfade");
    assert_eq!(
        project.library[0].hash,
        signal_lib::get("crossfade").unwrap().hash()
    );
    project.plan(&registry()).unwrap();
}

/// A reference to no library composite is an unknown module.
#[test]
fn an_unknown_lib_reference_is_an_unknown_module() {
    let text = project_text("x = { module = \"lib.nothing\" }\n", "");
    let project = Project::from_sources(sources(&[("project.toml", &text)])).unwrap();
    let errors = messages(&plan_errors(&project));
    assert!(errors.contains("lib.nothing"), "{errors}");
}

/// AC3: a project composite named like a library composite the project
/// uses is an error naming both; without such a reference it is the
/// project's own composite.
#[test]
fn a_project_composite_does_not_shadow_a_library_one() {
    let own = r#"schema_version = 1

[composite]
name = "tape_echo"

[instances]
g = { module = "std.scale", mul = 1, add = 0 }

[exports]
inputs = { in = "g.in" }
outputs = { out = "g.out" }
"#;
    let uses_library = project_text(
        "x = { module = \"lib.tape_echo\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let errors = load_errors(
        &[
            ("project.toml", &uses_library),
            ("composites/tape_echo.toml", own),
        ],
        Library::builtin(),
    );
    let text = messages(&errors);
    assert_eq!(errors.len(), 1, "{text}");
    assert_eq!(errors[0].file, "project.toml");
    assert!(
        text.contains("composites/tape_echo.toml") && text.contains("lib.tape_echo"),
        "{text}"
    );

    // Control: the same composite, used by its own name, is the project's.
    let uses_own = project_text(
        "x = { module = \"tape_echo\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let project = Project::from_sources(sources(&[
        ("project.toml", &uses_own),
        ("composites/tape_echo.toml", own),
    ]))
    .unwrap();
    assert!(project.library.is_empty());
    project.plan(&registry()).unwrap();
}

/// AC3: a project composite cannot carry a library composite's reference
/// as its name; a dotted name that is no library composite's gets the
/// generic invalid-name error.
#[test]
fn a_project_composite_named_lib_dot_is_refused() {
    let text = project_text("", "");
    let dotted = "schema_version = 1\n\n[composite]\nname = \"lib.tape_echo\"\n";
    let errors = load_errors(
        &[
            ("project.toml", &text),
            ("composites/lib.tape_echo.toml", dotted),
        ],
        Library::builtin(),
    );
    let message = messages(&errors);
    assert_eq!(errors.len(), 1, "{message}");
    assert!(
        message.contains("composites/lib.tape_echo.toml")
            && message.contains("library composite `lib.tape_echo`"),
        "{message}"
    );

    let other = "schema_version = 1\n\n[composite]\nname = \"lib.nothing\"\n";
    let errors = load_errors(
        &[
            ("project.toml", &text),
            ("composites/lib.nothing.toml", other),
        ],
        Library::builtin(),
    );
    let message = messages(&errors);
    assert!(
        message.contains("invalid composite name `lib.nothing`"),
        "{message}"
    );
}

/// AC4: a library composite uses std.* and lib.* only, takes no pattern or
/// gpu pin, and declares its category and description.
#[test]
fn library_composites_use_std_and_lib_only() {
    let bad = r#"schema_version = 1

[composite]
name = "bad"

[instances]
v = { module = "voice" }
s = { module = "std.step_seq8", pattern = "drums" }
"#;
    let library = Library::new([("bad".to_owned(), bad.to_owned())]);
    let text = project_text("x = { module = \"lib.bad\" }\n", "");
    let errors = load_errors(&[("project.toml", &text)], &library);
    let message = messages(&errors);
    assert!(
        errors.iter().all(|d| d.file == "signal-lib:lib/bad.toml"),
        "{message}"
    );
    assert!(
        message.contains(
            "`voice`: the library composite `lib.bad` may use std.* and lib.* modules only"
        ),
        "{message}"
    );
    assert!(message.contains("takes no `pattern`"), "{message}");
    assert!(message.contains("declares no `category`"), "{message}");
    assert!(message.contains("gives no `description`"), "{message}");
}

/// AC4: a cycle among library composites is refused when the plan is
/// built, naming the chain.
#[test]
fn a_cycle_among_library_composites_is_refused() {
    let cyclic = |name: &str, other: &str| {
        format!(
            "schema_version = 1\n\n[composite]\nname = \"{name}\"\ncategory = \"core\"\n\
             description = \"One half of a cycle.\"\n\n\
             [instances]\ninner = {{ module = \"lib.{other}\" }}\n"
        )
    };
    let library = Library::new([
        ("a".to_owned(), cyclic("a", "b")),
        ("b".to_owned(), cyclic("b", "a")),
    ]);
    let text = project_text("x = { module = \"lib.a\" }\n", "");
    let project = Project::from_sources_with(sources(&[("project.toml", &text)]), &library)
        .expect("a cycle loads; the plan refuses it");
    let errors = messages(&plan_errors(&project));
    assert!(
        errors.contains("lib.a") && errors.contains("lib.b") && errors.contains("recursive"),
        "{errors}"
    );
}

/// AC5: a composite's category is a palette group of the registry.
#[test]
fn an_unknown_category_is_refused() {
    let own = "schema_version = 1\n\n[composite]\nname = \"voice\"\ncategory = \"drums\"\n\n\
               [instances]\ng = { module = \"std.scale\" }\n\n[exports]\noutputs = { out = \"g.out\" }\n";
    let text = project_text("x = { module = \"voice\" }\n", "");
    let project = Project::from_sources(sources(&[
        ("project.toml", &text),
        ("composites/voice.toml", own),
    ]))
    .unwrap();
    let errors = messages(&plan_errors(&project));
    assert!(
        errors.contains("unknown category `drums`") && errors.contains("generator"),
        "{errors}"
    );
}

/// An error inside a library composite names its file in the library.
#[test]
fn errors_in_a_library_composite_name_its_file() {
    let broken = OUTER.replace("\"b.out\", to = \"sum.b\"", "\"b.nope\", to = \"sum.b\"");
    let library = Library::new([
        ("inner".to_owned(), INNER.to_owned()),
        ("outer".to_owned(), broken),
    ]);
    let text = project_text("x = { module = \"lib.outer\" }\n", "");
    let project =
        Project::from_sources_with(sources(&[("project.toml", &text)]), &library).unwrap();
    let errors = plan_errors(&project);
    assert!(
        errors.iter().any(|d| d.file == "signal-lib:lib/outer.toml"),
        "{}",
        messages(&errors)
    );
}

/// AC7: the manifest lists the library composites the plan instantiates,
/// transitively, with their hashes; one only an unused project composite
/// references is validated but not listed. A project with none (kick) lists
/// nothing, so its manifest has no such field.
#[test]
fn the_manifest_lists_the_library_composites_used() {
    let unused = "schema_version = 1\n\n[composite]\nname = \"unused\"\n\n\
                  [instances]\ne = { module = \"lib.ar_env\" }\n\n[exports]\noutputs = { out = \"e.out\" }\n";
    let text = project_text(
        "x = { module = \"lib.crossfade\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let project = Project::from_sources(sources(&[
        ("project.toml", &text),
        ("composites/unused.toml", unused),
    ]))
    .unwrap();
    project.plan(&registry()).unwrap();
    assert_eq!(project.library.len(), 2);
    let used = project.library_hashes();
    assert_eq!(
        used,
        BTreeMap::from([(
            "lib.crossfade".to_owned(),
            signal_lib::get("crossfade").unwrap().hash()
        )])
    );
    let provenance = Provenance {
        library_composite_hashes: used,
        ..Provenance::default()
    };
    let json = serde_json::to_value(&provenance).unwrap();
    assert!(json["library_composite_hashes"]["lib.crossfade"].is_string());

    let kick =
        Project::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick")).unwrap();
    assert!(kick.library_hashes().is_empty());
    let provenance = Provenance {
        library_composite_hashes: kick.library_hashes(),
        ..Provenance::default()
    };
    let json = serde_json::to_value(&provenance).unwrap();
    assert!(json.get("library_composite_hashes").is_none(), "{json}");
}

/// AC7: a `.signal` archive records the library version in its comment;
/// unpacking reports it, and the archive still packs to the folder.
#[test]
fn archives_record_the_library_version() {
    let dir = folder("archive");
    let text = project_text("x = { module = \"lib.crossfade\" }\n", "");
    write(&dir, &[("project.toml", &text)]);
    let bytes = pack(&dir).unwrap();
    assert_eq!(
        library_version(&bytes).unwrap().as_deref(),
        Some(Library::VERSION)
    );
    assert!(packs_to(&dir, &bytes).unwrap());
    let into = dir.with_extension("unpacked");
    let _ = fs::remove_dir_all(&into);
    let unpacked = unpack(&bytes, &into).unwrap();
    assert_eq!(unpacked.files, ["project.toml"]);
    assert_eq!(unpacked.library_version.as_deref(), Some(Library::VERSION));
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&into).unwrap();
}

/// AC7: a session records the library version and the library composites
/// its versions use (here version 0's; a swap's are added, as
/// `signal-cli/tests/session.rs` checks); both read back.
#[test]
fn sessions_record_the_library() {
    let text = project_text(
        "x = { module = \"lib.crossfade\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let project = Project::from_sources(sources(&[("project.toml", &text)])).unwrap();
    let registry = registry();
    let recorder =
        SessionRecorder::new(&project, &registry, 48_000, 9, Provenance::default()).unwrap();
    let dir = folder("session");
    let _ = fs::remove_dir_all(&dir);
    recorder.write(&dir).unwrap();
    let toml = fs::read_to_string(dir.join("session.toml")).unwrap();
    assert!(
        toml.contains(&format!("library_version = \"{}\"", Library::VERSION)),
        "{toml}"
    );
    assert!(
        toml.contains("[provenance.library_composite_hashes]"),
        "{toml}"
    );
    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(session.library_version.as_deref(), Some(Library::VERSION));
    assert_eq!(
        session.provenance.library_composite_hashes,
        project.library_hashes()
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// AC6: vendoring copies a composite and those it uses, renamed, rewrites
/// the project's references, and the project renders bit-identically.
#[test]
fn vendoring_renders_bit_identically() {
    let library = nested();
    let text = project_text(
        "noise = { module = \"std.noise\" }\n\
         # The voice.\n\
         x = { module = \"lib.outer\", level = 0.1 }\n\
         y = { module = \"lib.inner\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n\
         { from = \"noise.out\", to = \"y.in\" },\n\
         { from = \"y.out\", to = \"out.r\" },\n",
    );
    let dir = folder("vendor");
    write(&dir, &[("project.toml", &text)]);
    let load = |dir: &Path| {
        Project::from_sources_with(signal_project::read_sources(dir).unwrap(), &library).unwrap()
    };
    let before = render(&load(&dir));

    let vendored = vendor(&dir, "lib.outer", Some("voice"), &library).unwrap();
    assert_eq!(
        vendored.copied,
        ["composites/inner.toml", "composites/voice.toml"]
    );
    assert_eq!(vendored.rewritten, ["project.toml"]);
    let voice = fs::read_to_string(dir.join("composites/voice.toml")).unwrap();
    assert!(voice.contains("name = \"voice\"") && voice.contains("module = \"inner\""));
    assert!(voice.contains("# Noise through two inner composites."));
    let rewritten = fs::read_to_string(dir.join("project.toml")).unwrap();
    assert!(rewritten.contains("x = { module = \"voice\", level = 0.1 }"));
    assert!(rewritten.contains("y = { module = \"inner\" }") && rewritten.contains("# The voice."));

    // The vendored project loads cleanly, uses no library composite, and
    // renders bit-identically (audio and log).
    let after = Project::load(&dir).unwrap();
    assert!(after.library.is_empty());
    assert!(after.files.iter().all(|f| !f.text.contains("\"lib.")));
    after.plan(&registry()).unwrap();
    assert_eq!(render(&after), before);

    // Vendoring again would overwrite the copy (its identical dependency
    // would be kept): refused, nothing written.
    match vendor(&dir, "lib.outer", Some("voice"), &library) {
        Err(VendorError::Exists(path)) => assert_eq!(path, "composites/voice.toml"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fs::read_to_string(dir.join("project.toml")).unwrap(),
        rewritten
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// AC6: vendoring one dependency alone leaves the library composite that
/// uses it in the library, and the project loads and renders as before.
#[test]
fn vendoring_a_dependency_alone_keeps_the_project_loading() {
    let library = nested();
    let text = project_text(
        "x = { module = \"lib.outer\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n",
    );
    let dir = folder("partial");
    write(&dir, &[("project.toml", &text)]);
    let load = |dir: &Path| {
        Project::from_sources_with(signal_project::read_sources(dir).unwrap(), &library).unwrap()
    };
    let before = render(&load(&dir));
    let vendored = vendor(&dir, "lib.inner", None, &library).unwrap();
    assert_eq!(vendored.copied, ["composites/inner.toml"]);
    assert!(vendored.rewritten.is_empty());
    let after = load(&dir);
    assert_eq!(after.composites.len(), 1);
    assert_eq!(render(&after), before);
    fs::remove_dir_all(&dir).unwrap();
}

/// AC6: a dependency whose name the project already has, with other text,
/// refuses the whole operation, and nothing is written.
#[test]
fn vendoring_refuses_a_taken_dependency_name() {
    let library = nested();
    let text = project_text("x = { module = \"lib.outer\" }\n", "");
    let dir = folder("taken");
    let own = INNER.replace("mul = 0.5", "mul = 0.4");
    write(
        &dir,
        &[("project.toml", &text), ("composites/inner.toml", &own)],
    );
    match vendor(&dir, "lib.outer", None, &library) {
        Err(VendorError::Exists(path)) => assert_eq!(path, "composites/inner.toml"),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join("composites/outer.toml").exists());
    assert_eq!(fs::read_to_string(dir.join("project.toml")).unwrap(), text);
    assert!(matches!(
        vendor(&dir, "lib.nothing", None, &library),
        Err(VendorError::NotInLibrary(_))
    ));
    assert!(matches!(
        vendor(&dir, "outer", None, &library),
        Err(VendorError::NotInLibrary(_))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

/// Review M2.10-3 (low): a dependency vendored before, its file unchanged,
/// is kept rather than refused, and the project renders as before; nothing
/// temporary is left behind.
#[test]
fn vendoring_keeps_an_identical_dependency() {
    let library = nested();
    let text = project_text(
        "x = { module = \"lib.outer\" }\ny = { module = \"lib.inner\" }\n",
        "{ from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"y.in\" },\n\
         { from = \"y.out\", to = \"out.r\" },\n",
    );
    let dir = folder("reuse");
    write(&dir, &[("project.toml", &text)]);
    let load = |dir: &Path| {
        Project::from_sources_with(signal_project::read_sources(dir).unwrap(), &library).unwrap()
    };
    let before = render(&load(&dir));
    let first = vendor(&dir, "lib.inner", None, &library).unwrap();
    assert_eq!(first.copied, ["composites/inner.toml"]);
    assert_eq!(first.rewritten, ["project.toml"]);
    let second = vendor(&dir, "lib.outer", None, &library).unwrap();
    assert_eq!(second.copied, ["composites/outer.toml"]);
    assert_eq!(second.reused, ["composites/inner.toml"]);
    assert_eq!(second.rewritten, ["project.toml"]);
    let after = load(&dir);
    assert!(after.library.is_empty());
    assert_eq!(render(&after), before);
    let mut names: Vec<String> = fs::read_dir(dir.join("composites"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["inner.toml", "outer.toml"]);
    // The top composite itself is never kept: vendoring it again is refused.
    assert!(matches!(
        vendor(&dir, "lib.inner", None, &library),
        Err(VendorError::Exists(_))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

/// AC6 (auditor-1-002): a copy named like another library composite the
/// project still uses directly would leave a project that does not load
/// (AC3's collision), so vendoring is refused and nothing is written.
#[test]
fn vendoring_refuses_a_name_that_would_collide_with_a_used_library_composite() {
    let text = project_text(
        "echo = { module = \"lib.tape_echo\" }\n\
         fade = { module = \"lib.crossfade\" }\n",
        "{ from = \"echo.out\", to = \"fade.a\" },\n\
         { from = \"fade.out\", to = \"out.l\" },\n",
    );
    let dir = folder("collide");
    write(&dir, &[("project.toml", &text)]);
    Project::load(&dir)
        .unwrap()
        .plan(&registry())
        .expect("the project loads and plans before vendoring");

    match vendor(&dir, "lib.tape_echo", Some("crossfade"), Library::builtin()) {
        Err(VendorError::Invalid(diagnostics)) => {
            let message = messages(&diagnostics);
            assert!(
                message.contains("lib.crossfade") && message.contains("composites/crossfade.toml"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(dir.join("project.toml")).unwrap(), text);
    assert!(!dir.join("composites").exists());
    Project::load(&dir).unwrap().plan(&registry()).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

/// AC6 with the built-in library: the M2.11-derived composites vendored
/// into a project render bit-identically.
#[test]
fn vendoring_builtin_composites_renders_bit_identically() {
    let text = project_text(
        "clock = { module = \"std.phasor\", freq = 8 }\n\
         kick = { module = \"lib.analog_kick\" }\n\
         echo = { module = \"lib.tape_echo\", mix = 0.5 }\n",
        "{ from = \"clock.wrap\", to = \"kick.trigger\" },\n\
         { from = \"kick.out\", to = \"echo.in\" },\n\
         { from = \"echo.out\", to = \"out.l\" },\n\
         { from = \"echo.out\", to = \"out.r\" },\n",
    );
    let dir = folder("builtin");
    write(&dir, &[("project.toml", &text)]);
    let before = render(&Project::load(&dir).unwrap());
    let library = Library::builtin();
    vendor(&dir, "lib.tape_echo", None, library).unwrap();
    vendor(&dir, "lib.analog_kick", Some("kick_drum"), library).unwrap();
    let after = Project::load(&dir).unwrap();
    assert!(after.library.is_empty() && after.library_hashes().is_empty());
    let names: Vec<&str> = after.composites.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["kick_drum", "tape_echo"]);
    assert_eq!(render(&after), before);
    fs::remove_dir_all(&dir).unwrap();
}
