//! The GPU devices of a run (M2.7-2): in a `--features gpu` build (which
//! `lab` includes), the machine's adapters that the Setup's `gpu_devices`
//! allows, opened as a device pool, and GPU-backed instances placed on them
//! by the deterministic rule of `signal_gpu::GpuHost` and the project's `gpu`
//! pins. The default build links no GPU (AC2b): it lends nothing.

use std::collections::BTreeMap;
use std::path::Path;
#[cfg(feature = "gpu")]
use std::sync::Arc;

use serde_json::Value;
use signal_core::Registry;
use signal_engine::{ComputeDevice, Placement};
use signal_graph::ExecutionPlan;
use signal_project::session::Device;

/// The module types that run on a GPU device: `lab.*` only.
pub(crate) fn gpu_types() -> &'static [&'static str] {
    #[cfg(feature = "lab")]
    return &signal_lab::GPU_TYPES;
    #[cfg(not(feature = "lab"))]
    return &[];
}

/// Whether `plan` has a GPU-backed instance.
pub(crate) fn uses_gpu(plan: &ExecutionPlan) -> bool {
    plan.instances
        .iter()
        .any(|i| gpu_types().contains(&i.type_name))
}

/// A run's registry, whose GPU-backed instances are lent their device.
/// Keep it until the engine is dropped: it owns the pool, which must never be
/// dropped on the audio thread.
pub(crate) struct Gpu {
    registry: Registry,
    #[cfg(feature = "gpu")]
    host: Option<Arc<signal_gpu::GpuHost>>,
    /// The Setup that narrowed the devices: its path and `gpu_devices`.
    setup: Option<(String, String)>,
    /// What opening the devices reported.
    reports: Vec<String>,
    /// Whether the Setup that narrowed the devices was told.
    told: bool,
}

impl Gpu {
    /// `registry`, its GPU-backed instances placed on this machine's devices
    /// when `wanted` (the run has or may get one), by the project's `pins`.
    #[cfg_attr(not(feature = "gpu"), allow(unused_variables))]
    pub(crate) fn new(
        registry: &Registry,
        wanted: bool,
        pins: BTreeMap<String, String>,
        setup: Option<&Path>,
    ) -> Gpu {
        #[cfg_attr(not(feature = "gpu"), allow(unused_mut))]
        let mut gpu = Gpu {
            registry: registry.clone(),
            #[cfg(feature = "gpu")]
            host: None,
            setup: None,
            reports: Vec::new(),
            told: false,
        };
        #[cfg(feature = "gpu")]
        if wanted {
            let (selection, narrowed, problems) = setup::selection(setup);
            gpu.setup = narrowed;
            gpu.reports = problems;
            let pool = signal_gpu::DevicePool::open(&selection);
            gpu.reports.extend(pool.reports().iter().cloned());
            let host = Arc::new(signal_gpu::GpuHost::new(Arc::new(pool), gpu_types(), pins));
            gpu.registry
                .set_host(Arc::clone(&host) as Arc<dyn signal_core::Host>);
            gpu.host = Some(host);
        }
        gpu
    }

    /// The registry to build the engine with.
    pub(crate) fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The devices, for a session's machine (M2.7-8): every device of the
    /// pool, in id order; none without one.
    pub(crate) fn devices(&self) -> Vec<Device> {
        #[cfg(feature = "gpu")]
        if let Some(host) = &self.host {
            return host
                .pool()
                .devices()
                .iter()
                .map(|d| Device {
                    backend: d.backend.clone(),
                    name: d.name.clone(),
                    vendor: d.vendor,
                    device_id: d.device_id,
                    driver: d.driver.clone(),
                    driver_info: d.driver_info.clone(),
                })
                .collect();
        }
        Vec::new()
    }

    /// The render manifest's device set and placement (AC7b); both empty
    /// when no instance was placed.
    pub(crate) fn record(&self) -> (Vec<ComputeDevice>, Vec<Placement>) {
        #[cfg(feature = "gpu")]
        if let Some(host) = &self.host {
            let placement: Vec<Placement> = host
                .placement()
                .into_iter()
                .map(|p| Placement {
                    instance: p.instance,
                    device: p.device,
                    pinned: p.pinned,
                })
                .collect();
            if placement.is_empty() {
                return (Vec::new(), Vec::new());
            }
            let devices = host
                .pool()
                .devices()
                .iter()
                .map(|d| ComputeDevice {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    backend: d.backend.clone(),
                })
                .collect();
            return (devices, placement);
        }
        (Vec::new(), Vec::new())
    }

    /// What to tell the user, each once: the Setup that narrowed the
    /// devices, what opening them reported, unusable pins.
    pub(crate) fn warnings(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some((path, devices)) = self.setup.as_ref().filter(|_| !self.told) {
            warnings.push(format!(
                "GPU devices {} (Setup {path}: gpu_devices = {devices})",
                self.ids()
            ));
        }
        warnings.append(&mut self.reports);
        #[cfg(feature = "gpu")]
        if let Some(host) = &self.host {
            warnings.extend(host.take_warnings());
        }
        self.told = true;
        warnings
    }

    /// The placement's warnings made since the last call (unavailable pins,
    /// pins on instances that use no GPU), each once: for a run that
    /// prepares plans later, after each preparation (`signal play`).
    pub(crate) fn placement_warnings(&self) -> Vec<String> {
        #[cfg(feature = "gpu")]
        if let Some(host) = &self.host {
            return host.take_warnings();
        }
        Vec::new()
    }

    /// The Setup that narrowed the devices, for `--json` (`gpu_setup`).
    pub(crate) fn setup_json(&self) -> Option<Value> {
        self.setup.as_ref().map(|(path, devices)| {
            serde_json::json!({ "path": path, "gpu_devices": devices, "devices": self.ids() })
        })
    }

    fn ids(&self) -> String {
        #[cfg(feature = "gpu")]
        if let Some(host) = &self.host {
            let ids: Vec<&str> = host
                .pool()
                .devices()
                .iter()
                .map(|d| d.id.as_str())
                .collect();
            return if ids.is_empty() {
                "none".to_owned()
            } else {
                ids.join(", ")
            };
        }
        "none".to_owned()
    }
}

/// The machine Setup's `gpu_devices`, read as Signal.app reads its Setup.
#[cfg(feature = "gpu")]
pub(crate) mod setup {
    use std::path::{Path, PathBuf};

    use signal_gpu::Selection;

    /// The Setup file: `--setup`, else `SIGNAL_SETUP`, else
    /// `~/Library/Application Support/Signal/setup.toml` (as Signal.app).
    fn path(flag: Option<&Path>) -> Option<PathBuf> {
        if let Some(flag) = flag {
            return Some(flag.to_owned());
        }
        if let Some(env) = std::env::var_os("SIGNAL_SETUP").filter(|e| !e.is_empty()) {
            return Some(PathBuf::from(env));
        }
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|home| PathBuf::from(home).join("Library/Application Support/Signal/setup.toml"))
    }

    /// The devices the Setup allows; when it narrows them, its path and
    /// value; and what in it does not read (then every device).
    pub(crate) fn selection(
        flag: Option<&Path>,
    ) -> (Selection, Option<(String, String)>, Vec<String>) {
        let Some(path) = path(flag) else {
            return (Selection::All, None, Vec::new());
        };
        let shown = path.display().to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Selection::All, None, Vec::new());
            }
            Err(error) => {
                return (
                    Selection::All,
                    None,
                    vec![format!("{shown}: {error}; using every GPU device")],
                );
            }
        };
        let value = match text.parse::<toml::Table>() {
            Ok(mut table) => table.remove("gpu_devices"),
            Err(error) => {
                let reason = error.to_string();
                return (
                    Selection::All,
                    None,
                    vec![format!(
                        "{shown}: not valid TOML ({}); using every GPU device",
                        reason.trim()
                    )],
                );
            }
        };
        match value.as_ref().map(parse) {
            None => (Selection::All, None, Vec::new()),
            Some(Some(Selection::All)) => (Selection::All, None, Vec::new()),
            Some(Some(selection)) => {
                let described = selection.describe();
                (selection, Some((shown, described)), Vec::new())
            }
            Some(None) => (
                Selection::All,
                None,
                vec![format!(
                    "{shown}: gpu_devices: must be \"all\" or a list of GPU adapter ids or indices; using every GPU device"
                )],
            ),
        }
    }

    /// `"all"`, or a list of adapter ids (strings) or indices.
    fn parse(value: &toml::Value) -> Option<Selection> {
        match value {
            toml::Value::String(all) if all == "all" => Some(Selection::All),
            toml::Value::Array(entries) => entries
                .iter()
                .map(|entry| match entry {
                    toml::Value::String(id) if !id.is_empty() => Some(id.clone()),
                    toml::Value::Integer(index) if *index >= 0 => Some(index.to_string()),
                    _ => None,
                })
                .collect::<Option<Vec<String>>>()
                .map(Selection::Only),
            _ => None,
        }
    }
}
