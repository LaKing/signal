//! Review 2026-11-01 (special-file-project-toml): Signal.app opening a
//! project whose `project.toml` or `ui/windows.toml` is a FIFO or a link to
//! `/dev/zero` never blocks or reads without end: the open completes with
//! the load's diagnostic, the window layout's write is
//! refused, and the control thread goes on stepping, all within a bound.
//! (`file.read`, `pattern.read` and the panel reads use the same rule,
//! `signal_project::read_regular`.)

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use common::{devices, root};
use signal_core::Registry;
use signal_ui::player::Backend;
use signal_ui::{AppHostConfig, Edits, Host, ServerOptions};

/// How long the whole scenario may take.
const BOUND: Duration = Duration::from_secs(20);

/// Makes `path` the special file of `case`; false where it cannot be made.
fn special(case: &str, path: &Path) -> bool {
    match case {
        "zero" => std::os::unix::fs::symlink("/dev/zero", path).is_ok(),
        _ => Command::new("mkfifo")
            .arg(path)
            .status()
            .is_ok_and(|s| s.success()),
    }
}

/// The scenario on the control thread's own thread, for `case`; what it
/// saw, or a timeout.
fn scenario(case: &'static str, base: PathBuf) -> Result<(), String> {
    let tone = base.join("tone");
    fs::create_dir_all(&tone).unwrap();
    fs::write(tone.join("project.toml"), common::TONE).unwrap();
    // The project.toml case: the whole project's file is special.
    let broken = base.join("broken");
    fs::create_dir_all(&broken).unwrap();
    if !special(case, &broken.join("project.toml")) {
        eprintln!("{case}: the special file cannot be made here; skipped");
        return Ok(());
    }
    // The windows case: a project that loads, its ui/windows.toml special.
    let windows = base.join("windows");
    fs::create_dir_all(windows.join("ui")).unwrap();
    fs::write(windows.join("project.toml"), common::TONE).unwrap();
    assert!(special(case, &windows.join("ui/windows.toml")));
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let mut host = Host::new_app_local(AppHostConfig {
        setup_path: base.join("setup.toml"),
        project: Some(tone),
        open_root: base.join("Open"),
        registry,
        backend: Backend::Null(devices()),
        compute: vec!["CPU: test".to_owned()],
        server: ServerOptions {
            port: 0,
            remote: false,
            ui_dir: root().join("ui"),
            project_ui: PathBuf::new(),
            client_queue: 64,
        },
        plays: false,
    })?;
    // Opening it completes; it does not load, and the window says why.
    let _ = host.open_project(&broken, Edits::Discard);
    for _ in 0..20 {
        host.step();
    }
    let status = host.project_status().ok_or("no project")?;
    assert_eq!(status.dir, broken);
    // The layout of a project that loads, its ui/windows.toml special: refused.
    host.open_project(&windows, Edits::Discard)?;
    let (done, answer) = mpsc::channel();
    host.write_windows_then(
        &windows,
        Box::new(|text: &str| Ok(format!("{text}# edited\n"))),
        Box::new(move |result| {
            let _ = done.send(result);
        }),
    );
    let mut written = None;
    for _ in 0..200 {
        host.step();
        if let Ok(result) = answer.try_recv() {
            written = Some(result);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    match written {
        Some(Err(message)) => assert!(message.contains("windows.toml"), "{message}"),
        Some(Ok(())) => return Err(format!("{case}: a special ui/windows.toml was written")),
        None => return Err(format!("{case}: the layout write was never answered")),
    }
    for _ in 0..20 {
        host.step();
    }
    drop(host);
    Ok(())
}

#[test]
fn special_project_files_never_block_the_app() {
    for case in ["zero", "fifo"] {
        let base =
            std::env::temp_dir().join(format!("signal-ui-special-{case}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let (done, answer) = mpsc::channel();
        let at = base.clone();
        std::thread::spawn(move || {
            let _ = done.send(scenario(case, at));
        });
        match answer.recv_timeout(BOUND) {
            Ok(result) => result.unwrap(),
            Err(_) => panic!("{case}: the app blocked on a special project file"),
        }
        let _ = fs::remove_dir_all(&base);
    }
}
