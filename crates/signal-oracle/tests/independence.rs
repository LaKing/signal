//! AC-OR (M2.7-6): the oracle shares no code with the engine beyond
//! signal-core's types and the primitives. Its normal dependencies are
//! signal-core alone; the engine, the graph crate and the primitives are
//! dev-dependencies, used only by the comparison tests.

#[test]
fn the_oracle_depends_on_signal_core_alone() {
    let manifest = include_str!("../Cargo.toml");
    let mut section = "";
    let mut dependencies = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if section == "[dependencies]"
            && let Some((name, _)) = line.split_once('=')
            && !line.starts_with('#')
        {
            dependencies.push(name.trim().to_owned());
        }
        assert!(
            !(line.starts_with("[target") && line.contains("dependencies")),
            "a target-specific dependency section: {line}"
        );
    }
    assert_eq!(
        dependencies,
        ["signal-core"],
        "[dependencies] of signal-oracle"
    );
}
