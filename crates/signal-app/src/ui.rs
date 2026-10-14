//! The window (egui through eframe, ADR 0018). It only draws the latest
//! [`View`] and turns input into [`Command`]s for the control thread; every
//! decision is in [`crate::state`] and [`crate::dialog`].

use std::path::PathBuf;

use eframe::egui;
use signal_audio::{Direction, Resolution};

use crate::control::Control;
use crate::dialog::{Choice, Draft, capabilities, rate_check};
use crate::panel::ParameterView;
use crate::setup::Window;
use crate::state::{Command, MeterView, View};

/// The meters' scale floor, in dBFS.
const FLOOR_DB: f64 = -60.0;

/// The app's window.
pub struct SignalApp {
    control: Control,
    view: View,
    project: String,
    dialog: Option<Draft>,
    dialog_error: Option<String>,
    size: Option<Window>,
    notice: Option<String>,
    /// The slider being dragged (instance, parameter) and its value, shown
    /// until the view catches up.
    editing: Option<((String, String), f64)>,
}

impl SignalApp {
    /// The window over the control thread `control`.
    pub fn new(control: Control) -> SignalApp {
        SignalApp {
            control,
            view: View::default(),
            project: String::new(),
            dialog: None,
            dialog_error: None,
            size: None,
            notice: None,
            editing: None,
        }
    }

    fn send(&mut self, command: Command) {
        self.notice = self
            .control
            .send(command)
            .err()
            .map(|_| "busy, try again".to_owned());
    }

    fn take_view(&mut self) {
        if let Some(view) = self.control.latest() {
            if view.project != self.view.project {
                self.project = view.project.clone();
            }
            if let Some(draft) = self.dialog.as_mut()
                && view.devices != self.view.devices
            {
                draft.refreshed(&view.devices.devices);
            }
            self.view = view;
        }
    }

    fn open_dialog(&mut self) {
        self.dialog = Some(Draft::new(&self.view));
        self.dialog_error = None;
    }

    fn main_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Project folder");
            let field = ui.add(egui::TextEdit::singleline(&mut self.project).desired_width(420.0));
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Open").clicked() || entered {
                let dir = PathBuf::from(self.project.trim());
                self.send(Command::OpenProject(dir));
            }
        });
        ui.label("…or drop a project folder onto the window.");
        if let Some(rate) = self.view.sample_rate {
            ui.label(format!("{rate} Hz"));
        }
        for diagnostic in &self.view.diagnostics {
            ui.colored_label(egui::Color32::LIGHT_RED, diagnostic);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if self.view.playing {
                if ui.button("■ Stop").clicked() {
                    self.send(Command::Stop);
                }
            } else if ui.button("▶ Play").clicked() {
                self.send(Command::Play);
            }
            if ui.button("Interface setup…").clicked() {
                self.open_dialog();
            }
        });
        let messages = self
            .view
            .message
            .iter()
            .chain(&self.notice)
            .chain(&self.view.notes);
        for message in messages {
            ui.colored_label(egui::Color32::YELLOW, message);
        }
        for problem in &self.view.setup_problems {
            ui.colored_label(egui::Color32::YELLOW, problem);
        }
        for resolution in [&self.view.output, &self.view.input].into_iter().flatten() {
            if let Resolution::Missing { reason, .. } | Resolution::Ambiguous { reason, .. } =
                resolution
            {
                ui.colored_label(egui::Color32::YELLOW, reason);
            }
        }
        if let Some((text, error)) = &self.view.reload {
            let color = if *error {
                egui::Color32::LIGHT_RED
            } else {
                ui.visuals().text_color()
            };
            ui.colored_label(color, format!("Live reload: {text}"));
        }
        ui.separator();
        let meters = [(false, &self.view.outputs), (true, &self.view.inputs)];
        let mut clear = None;
        for (input, list) in meters {
            for (channel, meter) in list.iter().enumerate() {
                if meter_row(ui, meter) {
                    clear = Some(Command::ClearClip { input, channel });
                }
            }
        }
        if let Some(command) = clear {
            self.send(command);
        }
        self.parameter_panel(ui);
    }

    /// The parameters of each top-level instance: a slider for the base
    /// (dragging sends it, releasing writes it to `project.toml`), the
    /// effective value, and Reset. A composite's inner parameters are shown
    /// read-only.
    fn parameter_panel(&mut self, ui: &mut egui::Ui) {
        if self.view.panel.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Parameters");
        ui.label(format!(
            "Releasing a slider writes its value to {}; Reset removes it from the file.",
            signal_project::PROJECT_FILE
        ));
        let mut commands = Vec::new();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for instance in &self.view.panel {
                let title = format!("{} ({})", instance.name, instance.module);
                egui::CollapsingHeader::new(title)
                    .default_open(true)
                    .show(ui, |ui| {
                        for p in &instance.parameters {
                            let key = (instance.name.clone(), p.name.clone());
                            let mut value = match &self.editing {
                                Some((k, v)) if *k == key => *v,
                                _ => p.base,
                            };
                            ui.horizontal(|ui| {
                                ui.label(&p.name);
                                let response = ui.add(slider(&mut value, p));
                                if response.changed() {
                                    self.editing = Some((key.clone(), value));
                                    commands.push(Command::Drag {
                                        instance: key.0.clone(),
                                        parameter: key.1.clone(),
                                        value,
                                    });
                                }
                                // A typed value or a click is a drag of one step.
                                if response.drag_stopped()
                                    || (response.changed() && !response.dragged())
                                {
                                    self.editing = None;
                                    commands.push(Command::Release {
                                        instance: key.0.clone(),
                                        parameter: key.1.clone(),
                                    });
                                }
                                details(ui, p);
                                if ui.button("Reset").clicked() {
                                    commands.push(Command::Reset {
                                        instance: key.0.clone(),
                                        parameter: key.1.clone(),
                                    });
                                }
                            });
                        }
                        if !instance.inner.is_empty() {
                            egui::CollapsingHeader::new(
                                "Inside the composite (read-only: set in the composite's file, \
                                 shared by every instance of it)",
                            )
                            .id_salt(("inner", &instance.name))
                            .show(ui, |ui| {
                                for p in &instance.inner {
                                    let mut value = p.base;
                                    ui.horizontal(|ui| {
                                        ui.label(&p.name);
                                        ui.add_enabled(false, slider(&mut value, p));
                                        details(ui, p);
                                    });
                                }
                            });
                        }
                    });
            }
        });
        for command in commands {
            self.send(command);
        }
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        let text = match &self.view.status {
            Some(s) => format!(
                "frame {}   elapsed {:.3} s   xruns {}   load {:.1} % avg, {:.1} % worst   clipped {}   buffer {}   threads {} ({} workers, {})",
                s.frame,
                s.elapsed,
                s.xruns,
                s.load_average * 100.0,
                s.load_worst * 100.0,
                s.clipped,
                s.buffer,
                s.threads,
                s.workers,
                s.scheduling
            ),
            None => "stopped".to_owned(),
        };
        ui.label(text);
    }

    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.dialog.take() else {
            return;
        };
        let mut open = true;
        let mut command = None;
        let mut error = self.dialog_error.take();
        let view = &self.view;
        let devices = &view.devices.devices;
        egui::Window::new("Interface setup")
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Setup file: {}", view.setup_path));
                if let Some(error) = &view.devices.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                let refresh = ui.add_enabled(!view.playing, egui::Button::new("Refresh"));
                let refresh = refresh.on_disabled_hover_text(
                    "Stop playback to refresh: listing devices pauses metering and status",
                );
                if refresh.clicked() {
                    command = Some(Command::RefreshDevices);
                }
                for (direction, label, none, list) in [
                    (
                        Direction::Output,
                        "Output",
                        "System default",
                        &view.devices.outputs,
                    ),
                    (
                        Direction::Input,
                        "Input (meters only)",
                        "None",
                        &view.devices.inputs,
                    ),
                ] {
                    let chosen = match direction {
                        Direction::Output => &draft.output,
                        Direction::Input => &draft.input,
                    };
                    let index = draft.index(direction, devices);
                    let text = match chosen {
                        Choice::Unresolved(_) => "— not found: choose a device —".to_owned(),
                        Choice::Unset => none.to_owned(),
                        Choice::Device(_) => list
                            .iter()
                            .find(|(i, _)| Some(*i) == index)
                            .map_or_else(String::new, |(_, l)| l.clone()),
                    };
                    if let Choice::Unresolved(reason) = chosen {
                        ui.colored_label(egui::Color32::YELLOW, reason);
                    }
                    let mut picked = None;
                    egui::ComboBox::from_label(label)
                        .width(360.0)
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(matches!(chosen, Choice::Unset), none)
                                .clicked()
                            {
                                picked = Some(None);
                            }
                            for (i, text) in list {
                                if ui.selectable_label(index == Some(*i), text).clicked() {
                                    picked = Some(Some(*i));
                                }
                            }
                        });
                    if let Some(picked) = picked {
                        draft.select(direction, picked, devices);
                    }
                }
                match draft.output_device(devices) {
                    Some(device) => {
                        ui.label(format!("Supported by '{}':", device.name));
                        for line in capabilities(device) {
                            ui.label(format!("  {line}"));
                        }
                        if let Some(rate) = view.sample_rate {
                            match rate_check(device, rate, draft.buffer) {
                                Ok(line) => ui.label(line),
                                Err(line) => ui.colored_label(egui::Color32::LIGHT_RED, line),
                            };
                        }
                    }
                    None if draft.output == Choice::Unset => {
                        ui.label("The system default output is not in the device list.");
                    }
                    None => {}
                }
                ui.horizontal(|ui| {
                    ui.label("Buffer (frames)");
                    ui.add(egui::DragValue::new(&mut draft.buffer).range(1..=8192));
                });
                ui.horizontal(|ui| {
                    ui.label("CPU workers");
                    let mut auto = draft.cpu_workers.is_none();
                    if ui.checkbox(&mut auto, "Auto").changed() {
                        draft.cpu_workers = if auto { None } else { Some(1) };
                    }
                    if let Some(threads) = &mut draft.cpu_workers {
                        ui.add(egui::DragValue::new(threads).range(1..=64));
                        ui.label("threads, the audio thread included (1: none extra)");
                    } else {
                        ui.label("one per performance core");
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Output channels (device channel per sink channel)");
                    ui.add(egui::TextEdit::singleline(&mut draft.channels).hint_text("1, 2"));
                });
                // M2.7-2: read-only, as `signal devices --compute` lists it.
                egui::CollapsingHeader::new("Compute devices (read-only)").show(ui, |ui| {
                    for line in &view.devices.compute {
                        ui.monospace(line);
                    }
                });
                if let Some(error) = &error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.label("Apply saves the Setup; while playing, it stops and starts again.");
                if ui.button("Apply").clicked() {
                    match draft.interface() {
                        Ok(interface) => {
                            error = None;
                            command = Some(Command::Apply(Box::new(interface)));
                        }
                        Err(message) => error = Some(message),
                    }
                }
            });
        if let Some(command) = command {
            self.send(command);
        }
        if open {
            self.dialog = Some(draft);
            self.dialog_error = error;
        }
    }
}

fn db(x: f64) -> String {
    if x == f64::NEG_INFINITY {
        "−∞".to_owned()
    } else {
        format!("{x:.1}")
    }
}

/// A parameter's base slider over its range, with its unit; logarithmic for
/// a positive range spanning more than two decades.
fn slider<'a>(value: &'a mut f64, p: &ParameterView) -> egui::Slider<'a> {
    let slider = egui::Slider::new(value, p.min..=p.max)
        .logarithmic(p.min > 0.0 && p.max / p.min > 100.0)
        .max_decimals(6);
    match p.unit {
        Some(unit) => slider.suffix(format!(" {unit}")),
        None => slider,
    }
}

/// The range, the default, whether the file sets the value, and the
/// effective value (which differs from the base when modulated).
fn details(ui: &mut egui::Ui, p: &ParameterView) {
    let effective = p
        .effective
        .map_or_else(|| "—".to_owned(), |v| format!("{v:.6}"));
    let mut text = format!(
        "[{} … {}] default {}{}   effective {effective}",
        p.min,
        p.max,
        p.default,
        if p.in_file { "" } else { " (not in file)" }
    );
    if p.modulated {
        text.push_str("   modulated");
    }
    ui.monospace(text);
}

/// One meter: the RMS as a bar, peak, hold and RMS in dBFS, and the clip
/// indicator; true when the indicator was clicked.
fn meter_row(ui: &mut egui::Ui, meter: &MeterView) -> bool {
    let r = meter.reading;
    let fraction = |x: f64| ((x - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0) as f32;
    ui.horizontal(|ui| {
        ui.label(&meter.label);
        ui.add(egui::ProgressBar::new(fraction(r.held.max(r.rms))).desired_width(240.0));
        ui.monospace(format!(
            "peak {:>6} hold {:>6} rms {:>6} dBFS",
            db(r.peak),
            db(r.held),
            db(r.rms)
        ));
        let (text, fill) = if r.clip {
            ("CLIP", egui::Color32::RED)
        } else {
            ("clip", egui::Color32::DARK_GRAY)
        };
        ui.add(egui::Button::new(text).fill(fill)).clicked() && r.clip
    })
    .inner
}

impl eframe::App for SignalApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.take_view();
        let ctx = ui.ctx().clone();
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.size = Some(Window {
                width: rect.width(),
                height: rect.height(),
            });
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        });
        for path in dropped {
            if path.is_dir() {
                self.project = path.display().to_string();
                self.send(Command::OpenProject(path));
            } else {
                self.notice = Some("drop a project folder".to_owned());
            }
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_line(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.main_panel(ui));
        self.dialog(&ctx);
    }

    fn on_exit(&mut self) {
        self.control.exit(self.size);
    }
}
