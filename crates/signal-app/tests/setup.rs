//! M2.5-1 AC1: the Setup file round-trips, falls back to defaults with a
//! reason for anything it cannot read, and is written atomically.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use signal_app::setup::{DeviceEntry, Setup, Window, setup_path};
use signal_engine::{Computation, RtPolicy};
use signal_gpu::Selection;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-setup-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn full() -> Setup {
    Setup {
        output: Some(DeviceEntry {
            name: "DELL U3219Q".to_owned(),
            id: Some("coreaudio:dell-2".to_owned()),
            signature: "2 ch, 48000 Hz".to_owned(),
        }),
        input: Some(DeviceEntry {
            name: "SSL 2+ MKII".to_owned(),
            id: None,
            signature: "2 ch, 44100-192000 Hz".to_owned(),
        }),
        buffer: 256,
        cpu_workers: Some(4),
        output_channels: Some(vec![3, 4]),
        last_project: Some(PathBuf::from("/Users/me/projects/kick")),
        window: Some(Window {
            width: 900.5,
            height: 640.0,
        }),
        rt_policy: RtPolicy::Semaphore,
        rt_computation: Computation::from_fraction(0.75).unwrap(),
        gpu_devices: Selection::Only(vec!["metal-apple-m5-max".to_owned(), "1".to_owned()]),
    }
}

/// M2.7-2 (AC7): `gpu_devices` is `"all"` or a list of adapter ids or
/// indices; anything else falls back to all with a reason; it is written
/// only when it limits the devices, and kept by a save.
#[test]
fn gpu_devices_limit_the_adapters() {
    let (setup, problems) = parse("gpu_devices = [\"metal-apple-m5-max\", 2]\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        setup.gpu_devices,
        Selection::Only(vec!["metal-apple-m5-max".to_owned(), "2".to_owned()])
    );
    assert_eq!(
        parse("gpu_devices = []\n").0.gpu_devices,
        Selection::Only(Vec::new())
    );
    assert_eq!(
        parse("gpu_devices = \"all\"\n").0.gpu_devices,
        Selection::All
    );
    let (setup, problems) = parse("gpu_devices = \"first\"\n");
    assert_eq!(setup.gpu_devices, Selection::All);
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0].starts_with("setup.toml: gpu_devices: must be \"all\""),
        "{problems:?}"
    );
    assert!(!Setup::default().to_toml().contains("gpu_devices"));
    let text = full().to_toml();
    assert!(
        text.contains("gpu_devices = [\"metal-apple-m5-max\", \"1\"]"),
        "{text}"
    );
}

/// M2.7-1 (C1): the advanced keys `rt_policy` and `rt_computation` read,
/// fall back with a reason, and are written only when not the default.
#[test]
fn the_wake_policy_and_computation_are_advanced_keys() {
    let (setup, problems) = parse("rt_policy = \"fanout\"\nrt_computation = 0.5\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(setup.rt_policy, RtPolicy::Fanout);
    assert_eq!(setup.rt_computation.fraction(), 0.5);
    assert_eq!(
        parse("rt_computation = 1\n").0.rt_computation.fraction(),
        1.0
    );
    for name in ["park50", "budget-spin", "semaphore", "fanout"] {
        let (setup, problems) = parse(&format!("rt_policy = \"{name}\"\n"));
        assert!(problems.is_empty());
        assert_eq!(setup.rt_policy.name(), name);
    }
    let (setup, mut problems) = parse("rt_policy = \"spin\"\nrt_computation = 1.5\n");
    assert_eq!(setup, Setup::default());
    problems.sort_unstable();
    assert_eq!(
        problems,
        [
            "setup.toml: rt_computation: must be a fraction of the period above 0 and at most 1; using the default",
            "setup.toml: rt_policy: must be one of park50, budget-spin, semaphore, fanout; using the default"
        ]
    );
    // The default Setup writes neither key.
    let text = Setup::default().to_toml();
    assert!(!text.contains("rt_"), "{text}");
    assert!(full().to_toml().contains("rt_policy = \"semaphore\""));
}

fn parse(text: &str) -> (Setup, Vec<String>) {
    Setup::parse(text, Path::new("setup.toml"))
}

#[test]
fn default_and_full_setups_round_trip() {
    let dir = scratch("round-trip");
    let path = dir.join("Signal/setup.toml");
    for setup in [Setup::default(), full()] {
        setup.save(&path).unwrap();
        assert_eq!(Setup::load(&path), (setup, Vec::new()));
    }
    // A missing file is the default, with nothing to report.
    assert_eq!(
        Setup::load(&dir.join("absent.toml")),
        (Setup::default(), Vec::new())
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn what_does_not_read_falls_back_with_a_reason() {
    let (setup, problems) = parse("buffer = [");
    assert_eq!(setup, Setup::default());
    assert!(
        problems[0].starts_with("setup.toml: not valid TOML ("),
        "{problems:?}"
    );

    let (setup, problems) = parse("version = 2\nbuffer = 64\n");
    assert_eq!(setup, Setup::default());
    assert_eq!(
        problems,
        ["setup.toml: version 2 is not 1; using the default Setup"]
    );

    // Each bad key falls back alone; the good ones are kept.
    let (setup, problems) = parse(
        "version = 1\nbuffer = \"lots\"\noutput_channels = [0, 1]\ncolour = \"red\"\n\
         last_project = \"/p\"\n[window]\nwidth = -1.0\nheight = 10.0\n\
         [output]\nname = \"Speakers\"\nchannels = 2\n[input]\nname = \"Mic\"\n",
    );
    assert_eq!(
        setup,
        Setup {
            last_project: Some(PathBuf::from("/p")),
            input: Some(DeviceEntry {
                name: "Mic".to_owned(),
                id: None,
                signature: String::new(),
            }),
            ..Setup::default()
        }
    );
    let mut keys: Vec<&str> = problems
        .iter()
        .map(|p| p.split(':').nth(1).unwrap().trim())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "buffer",
            "output",
            "output_channels",
            "unknown key 'colour'; ignored",
            "window"
        ],
        "{problems:#?}"
    );
    assert!(problems.iter().all(|p| p.starts_with("setup.toml: ")));

    // CPU workers: "auto" or a number of threads, at least 1.
    assert_eq!(parse("cpu_workers = \"auto\"\n").0.cpu_workers, None);
    assert_eq!(parse("cpu_workers = 3\n").0.cpu_workers, Some(3));
    for bad in ["cpu_workers = 0\n", "cpu_workers = \"many\"\n"] {
        let (setup, problems) = parse(bad);
        assert_eq!(setup.cpu_workers, None);
        assert_eq!(
            problems,
            [
                "setup.toml: cpu_workers: must be \"auto\" or a number of at least 1; using the default"
            ]
        );
    }
}

#[test]
fn the_write_is_atomic() {
    let dir = scratch("atomic");
    let path = dir.join("setup.toml");
    full().save(&path).unwrap();
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        [OsString::from("setup.toml")],
        "no temporary file is left"
    );

    // A write that cannot complete leaves the old file as it was.
    let before = fs::read(&path).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let failed = Setup::default().save(&path);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(failed.is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_flag_then_the_environment_then_application_support() {
    let home = Some(OsString::from("/Users/me"));
    let env = Some(OsString::from("/tmp/env.toml"));
    assert_eq!(
        setup_path(Some(Path::new("flag.toml")), env.clone(), home.clone()),
        Ok(PathBuf::from("flag.toml"))
    );
    assert_eq!(
        setup_path(None, env, home.clone()),
        Ok(PathBuf::from("/tmp/env.toml"))
    );
    assert_eq!(
        setup_path(None, None, home),
        Ok(PathBuf::from(
            "/Users/me/Library/Application Support/Signal/setup.toml"
        ))
    );
    assert!(setup_path(None, None, None).is_err());
}
