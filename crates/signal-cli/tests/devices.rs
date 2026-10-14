//! M2.7-2, AC3: `signal devices --compute` lists the CPU's performance
//! levels and the GPU adapters, as text and as the JSON documented in
//! `docs/benchmarks.md` ("`signal devices --compute`"). The default build
//! links no wgpu (AC2b, decision of 2026-10-14): it says the GPU part is not
//! built; `lab.rs` checks a GPU build.

use std::process::{Command, Stdio};

use serde_json::Value;

fn signal(args: &[&str]) -> (String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs");
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    assert!(output.status.success(), "{}", text(&output.stderr));
    (text(&output.stdout), text(&output.stderr))
}

#[test]
fn compute_devices_as_json() {
    let (stdout, _) = signal(&["--json", "devices", "--compute"]);
    let devices: Value = serde_json::from_str(&stdout).unwrap();
    let cpu = &devices["cpu"];
    assert!(
        ["sysctl", "fallback"].contains(&cpu["source"].as_str().unwrap()),
        "{cpu}"
    );
    assert!(cpu["performance_cores"].as_u64().is_some_and(|n| n >= 1));
    let levels = cpu["levels"].as_array().unwrap();
    assert!(!levels.is_empty());
    for level in levels {
        for key in ["index", "physical", "logical"] {
            assert!(level[key].is_u64(), "{key}: {level}");
        }
        assert!(level["name"].is_string());
    }
    let gpu = &devices["gpu"];
    for key in ["built", "available"] {
        assert!(gpu[key].is_boolean(), "{key}: {gpu}");
    }
    assert!(gpu["adapters"].is_array(), "{gpu}");
    if cfg!(not(feature = "gpu")) {
        assert_eq!(gpu["built"], false);
        assert_eq!(gpu["available"], false);
        assert!(
            gpu["reason"]
                .as_str()
                .is_some_and(|r| r.contains("--features gpu")),
            "{gpu}"
        );
    }
}

#[test]
fn compute_devices_as_text() {
    let (stdout, _) = signal(&["devices", "--compute"]);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].starts_with("CPU ("), "{stdout}");
    assert!(lines[1].starts_with("  level 0 "), "{stdout}");
    assert!(lines.contains(&"GPU:"), "{stdout}");
    if cfg!(not(feature = "gpu")) {
        assert!(
            stdout.contains("not listed: this build has no GPU support"),
            "{stdout}"
        );
    }
}
