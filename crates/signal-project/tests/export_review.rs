//! The outside review of 99b764b (slot export, WAV loading): an exported
//! slot plays what it played in the host, or the export is refused.
use signal_project::{Project, export};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn dir(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("export-review-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn registry() -> signal_core::Registry {
    let mut r = signal_core::Registry::new();
    signal_std::register(&mut r).unwrap();
    r
}

const HEAD: &str =
    "schema_version = 1\n[project]\nname = \"host\"\nsample_rate = 48000\nseed = 1\n";

/// The exported files, loaded and planned from a folder of their own.
fn exported(host: &Path, slot: &str) -> (PathBuf, signal_graph::ExecutionPlan) {
    let project = Project::load(host).unwrap();
    let files = export::export_project(host, &project, slot, &registry()).unwrap();
    let out = host.with_file_name(format!(
        "{}-out",
        host.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&out);
    for (path, bytes) in &files {
        let p = out.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    let plan = Project::load(&out).unwrap().plan(&registry()).unwrap().plan;
    (out, plan)
}

fn value(plan: &signal_graph::ExecutionPlan, address: &str, parameter: &str) -> f64 {
    plan.instances
        .iter()
        .find(|i| i.address.as_str() == address)
        .unwrap_or_else(|| panic!("no {address}"))
        .parameters[parameter]
}

/// A slot's value reaches an instance written as a `[instances.c]` table.
#[test]
fn a_slot_value_reaches_a_standard_table() {
    let host = dir("standard");
    write(
        &host,
        "project.toml",
        &format!(
            "{HEAD}[instances]\ns = {{ slot = {{ outputs = {{ out = {{}} }}, parameters = {{ level = {{}} }} }}, load = \"voice\", level = 0.75 }}\n\
             out = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [{{ from = \"s.out\", to = \"out.l\" }}]\n"
        ),
    );
    write(
        &host,
        "composites/voice.toml",
        "schema_version = 1\n[composite]\nname = \"voice\"\n\n[instances.c]\nmodule = \"std.const\"\nvalue = 0.125\n\n\
         [exports]\noutputs = { out = \"c.out\" }\nparameters = { level = \"c.value\" }\n",
    );
    let (_, plan) = exported(&host, "s");
    assert_eq!(value(&plan, "/c", "value"), 0.75);
}

/// A `set` key of a module slot (a parameter outside its interface)
/// travels too.
#[test]
fn a_module_slot_s_set_travels() {
    let host = dir("set");
    write(
        &host,
        "project.toml",
        &format!(
            "{HEAD}[instances]\ns = {{ slot = {{ outputs = {{ out = {{}} }} }}, load = \"std.const\", set = {{ value = 0.625 }} }}\n\
             out = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [{{ from = \"s.out\", to = \"out.l\" }}]\n"
        ),
    );
    let (_, plan) = exported(&host, "s");
    assert_eq!(value(&plan, "/s", "value"), 0.625);
}

/// A slot port the content does not match is not exported.
#[test]
fn an_unmatched_slot_port_is_not_exported() {
    let host = dir("spare");
    write(
        &host,
        "project.toml",
        &format!(
            "{HEAD}[instances]\ns = {{ slot = {{ outputs = {{ out = {{}}, spare = {{}} }} }}, load = \"std.const\" }}\n\
             out = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [{{ from = \"s.out\", to = \"out.l\" }}]\n"
        ),
    );
    Project::load(&host).unwrap().plan(&registry()).unwrap();
    let (out, _) = exported(&host, "s");
    let text = fs::read_to_string(out.join("project.toml")).unwrap();
    assert!(!text.contains("spare"), "{text}");
}

/// Content that includes a project is refused, not exported half.
#[test]
fn content_including_a_project_is_refused() {
    let host = dir("include");
    write(
        &host,
        "project.toml",
        &format!(
            "{HEAD}[instances]\ns = {{ slot = {{ outputs = {{ out = {{}} }} }}, load = \"voice\" }}\n\
             out = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [{{ from = \"s.out\", to = \"out.l\" }}]\n"
        ),
    );
    write(
        &host,
        "composites/voice.toml",
        "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\nsub = { project = \"./sub\" }\n\
         [exports]\noutputs = { out = \"sub.out\" }\n",
    );
    write(
        &host,
        "sub/project.toml",
        "schema_version = 1\n[project]\nname = \"sub\"\n[instances]\nc = { module = \"std.const\", value = 0.5 }\n\
         [exports]\noutputs = { out = \"c.out\" }\n",
    );
    let project = Project::load(&host).unwrap();
    project.plan(&registry()).unwrap();
    let refused = export::export_project(&host, &project, "s", &registry()).unwrap_err();
    assert!(refused.contains("./sub"), "{refused}");
}

/// The part's bindings are rooted in either quote, through an export
/// written as a table, and the assets it names are carried.
#[test]
fn the_part_is_rooted_and_carries_its_assets() {
    let host = dir("part");
    write(
        &host,
        "project.toml",
        &format!(
            "{HEAD}[instances]\ns = {{ slot = {{ outputs = {{ out = {{}} }}, parameters = {{ level = {{}}, y = {{}} }} }}, load = \"voice\" }}\n\
             out = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [{{ from = \"s.out\", to = \"out.l\" }}]\n"
        ),
    );
    write(
        &host,
        "composites/voice.toml",
        "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\nc = { module = \"std.const\", value = 0.125 }\n\
         d = { module = \"std.const\", value = 0.25 }\n\
         [exports]\noutputs = { out = \"c.out\" }\n\
         parameters = { level = { to = \"c.value\", min = 0, max = 1 }, y = \"d.value\" }\n",
    );
    write(
        &host,
        "ui/parts/s.html",
        "<signal-card part=\"s\"><signal-knob bind=\".level\"></signal-knob>\
         <signal-knob bind='.y'></signal-knob><img src=\"assets/dial.svg\"></signal-card>\n",
    );
    write(&host, "ui/assets/dial.svg", "<svg/>");
    let (out, _) = exported(&host, "s");
    let main = fs::read_to_string(out.join("ui/main.html")).unwrap();
    assert!(main.contains("bind=\"/c.value\""), "{main}");
    assert!(main.contains("bind='/d.value'"), "{main}");
    assert_eq!(fs::read(out.join("ui/assets/dial.svg")).unwrap(), b"<svg/>");
}

/// ADR 0056 §1: a WAV longer than a table holds is refused from its header.
#[test]
fn a_long_wav_is_refused_from_its_header() {
    let host = dir("long-wav");
    let samples = signal_core::MAX_TABLE_VALUES + 1;
    let data = (samples * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    bytes.resize(bytes.len() + samples * 2, 0);
    write(&host, "samples/long.wav", "");
    fs::write(host.join("samples/long.wav"), &bytes).unwrap();
    write(
        &host,
        "patterns/long.toml",
        "schema_version = 1\nkind = \"sample\"\nfile = \"samples/long.wav\"\n",
    );
    write(
        &host,
        "project.toml",
        &format!(
            "{HEAD}[instances]\nhit = {{ module = \"std.sampler\", pattern = \"long\" }}\n\
             out = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [{{ from = \"hit.l\", to = \"out.l\" }}]\n"
        ),
    );
    let Err(refused) = Project::load(&host) else {
        panic!("the long WAV loaded");
    };
    let refused = refused.to_string();
    assert!(
        refused.contains(&format!("{samples} samples; a sample holds at most")),
        "{refused}"
    );
}
