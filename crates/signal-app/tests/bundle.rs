//! M2.5-3 AC2: `scripts/bundle-app.sh` builds Signal.app, offline and signed
//! ad hoc, into a directory of its own. Skipped (passing) without codesign or
//! plutil. The app is never opened, so no permission prompt appears.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether `tool` can be started at all.
fn available(tool: &str) -> bool {
    !matches!(Command::new(tool).output(), Err(e) if e.kind() == ErrorKind::NotFound)
}

fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn plist_value(plist: &Path, key: &str) -> String {
    let output = run(Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn bundle_app_builds_a_signed_bundle() {
    for tool in ["codesign", "plutil"] {
        if !available(tool) {
            eprintln!("skipped: {tool} is not available");
            return;
        }
    }
    let out = std::env::temp_dir().join(format!("signal-bundle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    // A target directory of its own, inside the one this test was built in
    // (M2.7-7): the script builds `signal` without features, and in the shared
    // directory that replaced the `CARGO_BIN_EXE_signal` other tests run (a
    // `--features signal-cli/lab` workspace run, or one running concurrently).
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bundle-app-target");
    run(Command::new("bash")
        .arg(root().join("scripts/bundle-app.sh"))
        .arg("--target-dir")
        .arg(&target)
        .arg("--out")
        .arg(&out));
    let app = out.join("Signal.app");
    let gui = app.join("Contents/MacOS/Signal");
    let cli = app.join("Contents/Helpers/signal");
    assert!(gui.is_file() && cli.is_file());
    assert_ne!(
        fs::read(&gui).unwrap(),
        fs::read(&cli).unwrap(),
        "the GUI and the CLI are different binaries"
    );

    let plist = app.join("Contents/Info.plist");
    run(Command::new("plutil").arg("-lint").arg(&plist));
    assert_eq!(plist_value(&plist, "CFBundleName"), "Signal");
    assert_eq!(plist_value(&plist, "CFBundleIdentifier"), "hu.d250.signal");
    assert_eq!(plist_value(&plist, "CFBundleExecutable"), "Signal");
    assert_eq!(
        plist_value(&plist, "CFBundleShortVersionString"),
        env!("CARGO_PKG_VERSION")
    );
    assert!(!plist_value(&plist, "LSMinimumSystemVersion").is_empty());
    assert!(
        plist_value(&plist, "NSMicrophoneUsageDescription").contains("audio input"),
        "the microphone usage is described"
    );
    assert_eq!(plist_value(&plist, "NSHighResolutionCapable"), "true");
    // M2.8-4: the window loads its pages from 127.0.0.1.
    assert_eq!(
        plist_value(&plist, "NSAppTransportSecurity.NSAllowsLocalNetworking"),
        "true"
    );

    // M2.8-4: the UI library inside the bundle (sealed by the signature
    // below), without its node tests.
    let ui = app.join("Contents/Resources/ui");
    for page in [
        "signal-ui.js",
        "app/index.html",
        "app/setup.html",
        "app/panel.html",
        "app/app.js",
        "controls/index.js",
        "default-panel.html",
    ] {
        assert!(ui.join(page).is_file(), "Resources/ui/{page}");
    }
    assert!(!ui.join("index.js").exists(), "the node test entry");
    assert!(!ui.join("app/lib/menu.test.js").exists(), "a node test");

    run(Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&app));
    for code in [&app, &cli] {
        let output = run(Command::new("codesign")
            .args(["-d", "--entitlements", "-", "--verbose=2"])
            .arg(code));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("Signature=adhoc"), "{code:?}: {text}");
        assert!(text.contains("runtime"), "{code:?}: hardened runtime");
        assert!(
            text.contains("com.apple.security.device.audio-input"),
            "{code:?}: {text}"
        );
    }

    let output = run(Command::new(&cli).arg("modules"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("std.audio_out"));

    // The helper is built with `--features gpu` (M2.7-2 answer A): it lists
    // the GPUs. The exit status is not asserted: a machine (or sandbox)
    // without a GPU may refuse the listing, which is not what is checked.
    let output = Command::new(&cli)
        .args(["devices", "--compute"])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !text.contains("this build has no GPU support"),
        "the helper is built with --features gpu: {text}"
    );
    fs::remove_dir_all(&out).unwrap();
}
