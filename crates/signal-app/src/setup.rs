//! The machine Setup (ADR 0009): which devices, buffer and output channels
//! this machine plays a project with, the last project folder and the
//! window's size, and advanced keys not in the dialog: the real-time
//! wake policy and computation (M2.7-1) and the GPU devices (M2.7-2). One TOML file (`docs/project-format.md`, "Setup"), never
//! part of a project. Reading it never fails: what is unknown or invalid is
//! reported and replaced by its default. Writing it is atomic.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use signal_audio::DeviceRef;
use signal_engine::{Computation, RtPolicy};
use signal_gpu::Selection;

/// The file's format version.
pub const VERSION: i64 = 1;

/// The buffer requested without a Setup, in frames.
pub const DEFAULT_BUFFER: u32 = 128;

/// A device as the Setup names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceEntry {
    /// Its name.
    pub name: String,
    /// Its stable identifier, when the audio system reports one. A saved id
    /// is the device: no other device is taken for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Its channels and rates when chosen, which tell same-named devices
    /// apart when there is no id.
    #[serde(default)]
    pub signature: String,
}

impl DeviceEntry {
    /// The entry for a [`DeviceRef`].
    pub fn from_ref(device: DeviceRef) -> DeviceEntry {
        DeviceEntry {
            name: device.name,
            id: device.id,
            signature: device.signature,
        }
    }

    /// The [`DeviceRef`] to find it with.
    pub fn to_ref(&self) -> DeviceRef {
        DeviceRef {
            name: self.name.clone(),
            id: self.id.clone(),
            signature: self.signature.clone(),
        }
    }
}

/// The window's size, in points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// Its width.
    pub width: f32,
    /// Its height.
    pub height: f32,
}

/// The machine Setup.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// The output device; the system's default output without one.
    pub output: Option<DeviceEntry>,
    /// The input device, metered only; no input without one.
    pub input: Option<DeviceEntry>,
    /// The buffer to request, in frames.
    pub buffer: u32,
    /// "CPU workers": the threads a block runs on, the audio thread
    /// included (`EngineConfig::threads`; 1: no worker threads); `None` is
    /// auto, one per performance core. A machine setting, never a project's.
    pub cpu_workers: Option<u32>,
    /// The device channel (from 1) of each sink channel; sink channel *i* on
    /// device channel *i* without it.
    pub output_channels: Option<Vec<u16>>,
    /// The last project folder opened.
    pub last_project: Option<PathBuf>,
    /// The window's size when last closed.
    pub window: Option<Window>,
    /// Advanced, not in the dialog (M2.7-1): how idle worker threads wait
    /// and are woken (`rt_policy`, `docs/realtime-rules.md` "Wake
    /// policies"). A machine setting; it never changes the output.
    pub rt_policy: RtPolicy,
    /// Advanced, not in the dialog: the workers' time-constraint
    /// computation, a fraction of the device period (`rt_computation`).
    pub rt_computation: Computation,
    /// Advanced, not edited in the dialog (M2.7-2): the GPU adapters GPU-backed
    /// modules may use (`gpu_devices`: `"all"`, or a list of ids or indices).
    pub gpu_devices: Selection,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            output: None,
            input: None,
            buffer: DEFAULT_BUFFER,
            cpu_workers: None,
            output_channels: None,
            last_project: None,
            window: None,
            rt_policy: RtPolicy::default(),
            rt_computation: Computation::DEFAULT,
            gpu_devices: Selection::All,
        }
    }
}

/// The file as written.
#[derive(Serialize)]
struct File<'a> {
    version: i64,
    buffer: u32,
    /// `"auto"` or a number of threads.
    cpu_workers: toml::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_channels: Option<&'a [u16]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<&'a DeviceEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<&'a DeviceEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<Window>,
    /// Written only when not the default, so a Setup that never set them
    /// stays as it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    rt_policy: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rt_computation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu_devices: Option<&'a [String]>,
}

/// `gpu_devices`: `"all"`, or a list of adapter ids (strings) or indices.
fn gpu_devices(value: &toml::Value) -> Option<Selection> {
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

/// Where the Setup lives: `--setup`, else `SIGNAL_SETUP`, else
/// `~/Library/Application Support/Signal/setup.toml`.
pub fn setup_path(
    flag: Option<&Path>,
    env: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    if let Some(flag) = flag {
        return Ok(flag.to_owned());
    }
    if let Some(env) = env.filter(|e| !e.is_empty()) {
        return Ok(PathBuf::from(env));
    }
    match home.filter(|h| !h.is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join("Library/Application Support/Signal/setup.toml")),
        None => Err("HOME is not set; give the Setup file with --setup or SIGNAL_SETUP".to_owned()),
    }
}

impl Setup {
    /// Reads the Setup at `path`: a missing file is the default; anything
    /// that does not read is reported (one reason per problem) and replaced
    /// by its default.
    pub fn load(path: &Path) -> (Setup, Vec<String>) {
        match fs::read_to_string(path) {
            Ok(text) => Setup::parse(&text, path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Setup::default(), Vec::new()),
            Err(error) => (
                Setup::default(),
                vec![format!(
                    "{}: {error}; using the default Setup",
                    path.display()
                )],
            ),
        }
    }

    /// [`load`](Setup::load) of the text of `path`.
    pub fn parse(text: &str, path: &Path) -> (Setup, Vec<String>) {
        let file = path.display();
        let mut setup = Setup::default();
        let mut problems = Vec::new();
        let table = match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(error) => {
                let reason = error.to_string();
                problems.push(format!(
                    "{file}: not valid TOML ({}); using the default Setup",
                    reason.trim()
                ));
                return (setup, problems);
            }
        };
        if let Some(version) = table.get("version")
            && version.as_integer() != Some(VERSION)
        {
            problems.push(format!(
                "{file}: version {version} is not {VERSION}; using the default Setup"
            ));
            return (setup, problems);
        }
        for (key, value) in table {
            let mut bad = |reason: String| {
                problems.push(format!("{file}: {key}: {reason}; using the default"));
            };
            match key.as_str() {
                "version" => {}
                "buffer" => match value.try_into::<u32>() {
                    Ok(frames) if frames > 0 => setup.buffer = frames,
                    Ok(_) => bad("must be at least 1 frame".to_owned()),
                    Err(error) => bad(error.to_string()),
                },
                "cpu_workers" => match value {
                    toml::Value::String(text) if text == "auto" => setup.cpu_workers = None,
                    toml::Value::Integer(n) if n >= 1 => match u32::try_from(n) {
                        Ok(n) => setup.cpu_workers = Some(n),
                        Err(error) => bad(error.to_string()),
                    },
                    _ => bad("must be \"auto\" or a number of at least 1".to_owned()),
                },
                "output_channels" => match value.try_into::<Vec<u16>>() {
                    Ok(channels) if channels.contains(&0) => {
                        bad("device channels count from 1".to_owned())
                    }
                    Ok(channels) => setup.output_channels = Some(channels),
                    Err(error) => bad(error.to_string()),
                },
                "last_project" => match value.try_into::<String>() {
                    Ok(dir) => setup.last_project = Some(PathBuf::from(dir)),
                    Err(error) => bad(error.to_string()),
                },
                "output" | "input" => match value.try_into::<DeviceEntry>() {
                    Ok(device) if device.name.is_empty() => bad("the name is empty".to_owned()),
                    Ok(device) if key == "output" => setup.output = Some(device),
                    Ok(device) => setup.input = Some(device),
                    Err(error) => bad(error.to_string()),
                },
                "window" => match value.try_into::<Window>() {
                    Ok(w)
                        if w.width.is_finite()
                            && w.height.is_finite()
                            && w.width > 0.0
                            && w.height > 0.0 =>
                    {
                        setup.window = Some(w);
                    }
                    Ok(_) => bad("width and height must be positive".to_owned()),
                    Err(error) => bad(error.to_string()),
                },
                "rt_policy" => match value.as_str().and_then(RtPolicy::from_name) {
                    Some(policy) => setup.rt_policy = policy,
                    None => {
                        let names: Vec<&str> = RtPolicy::ALL.iter().map(|p| p.name()).collect();
                        bad(format!("must be one of {}", names.join(", ")));
                    }
                },
                "rt_computation" => match value
                    .as_float()
                    .or_else(|| value.as_integer().map(|n| n as f64))
                    .and_then(Computation::from_fraction)
                {
                    Some(computation) => setup.rt_computation = computation,
                    None => {
                        bad("must be a fraction of the period above 0 and at most 1".to_owned())
                    }
                },
                "gpu_devices" => match gpu_devices(&value) {
                    Some(selection) => setup.gpu_devices = selection,
                    None => bad(
                        "must be \"all\" or a list of GPU adapter ids or indices (`signal devices --compute`)"
                            .to_owned(),
                    ),
                },
                _ => problems.push(format!("{file}: unknown key '{key}'; ignored")),
            }
        }
        (setup, problems)
    }

    /// The Setup as TOML.
    pub fn to_toml(&self) -> String {
        let file = File {
            version: VERSION,
            buffer: self.buffer,
            cpu_workers: match self.cpu_workers {
                Some(n) => toml::Value::Integer(i64::from(n)),
                None => toml::Value::String("auto".to_owned()),
            },
            output_channels: self.output_channels.as_deref(),
            last_project: self.last_project.as_ref().map(|p| p.display().to_string()),
            output: self.output.as_ref(),
            input: self.input.as_ref(),
            window: self.window,
            rt_policy: (self.rt_policy != RtPolicy::default()).then(|| self.rt_policy.name()),
            rt_computation: (self.rt_computation != Computation::DEFAULT)
                .then(|| self.rt_computation.fraction()),
            gpu_devices: match &self.gpu_devices {
                Selection::All => None,
                Selection::Only(entries) => Some(entries),
            },
        };
        toml::to_string(&file).unwrap_or_default()
    }

    /// Writes the Setup to `path` atomically: to a temporary file beside it,
    /// synced, then renamed over it. A failure leaves the old file as it was.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut name = path.file_name().unwrap_or_default().to_owned();
        name.push(".tmp");
        let temporary = path.with_file_name(name);
        let written = (|| {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(self.to_toml().as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written
    }
}
