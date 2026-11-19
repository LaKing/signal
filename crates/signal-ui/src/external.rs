//! External I/O for a run (ADR 0057): the project's `dmx` slots sent as
//! Art-Net, its `osc` and `midi` slots fed from their listeners, each role
//! bound by the Setup (`[play.<role>] artnet = "IP[:PORT][/UNIVERSE]"`,
//! `[capture.<role>] osc = "IP:PORT"` or `midi = "<name>"`; a project's
//! `devices.local.toml` over it). Opened on the control thread before the
//! Runner goes to its driver; polled while it plays; a role without a
//! binding is said once and left out.

use signal_audio::midi_in::MidiFeed;
use signal_graph::ExecutionPlan;
use signal_output::inputs::{InputRecord, RunInput};
use signal_output::osc::{OscListener, OscRoute};
use signal_output::{ArtNetSender, DataFrames, MAX_CALLBACK_FRAMES, Runner, parse_artnet};
use signal_project::Project;

use crate::setup::Setup;

/// A run's external I/O, kept for the run.
#[derive(Default)]
pub struct External {
    artnet: Option<(DataFrames, ArtNetSender)>,
    osc: Vec<OscListener>,
    midi: Vec<MidiFeed>,
    records: Vec<InputRecord>,
}

impl External {
    /// Binds `project`'s `dmx`, `osc` and `midi` slots on `setup`, sets the
    /// control inputs on `runner` (each recorded into a ring of
    /// `record_frames` when given) and its data frames for the sender; what
    /// is not bound or fails, in `notes`.
    pub fn open(
        project: &Project,
        plan: &ExecutionPlan,
        setup: &Setup,
        runner: &mut Runner,
        rate: u32,
        record_frames: Option<usize>,
        notes: &mut Vec<String>,
    ) -> External {
        let mut external = External::default();
        let mut data = signal_project::device::data_sides(project, plan);
        data.sort_by(|a, b| a.address.cmp(&b.address));
        if !data.is_empty() {
            let targets = data
                .iter()
                .map(|side| {
                    let written = setup.play.get(&side.role).and_then(|p| p.artnet.as_deref());
                    match written.map(parse_artnet) {
                        Some(Ok(target)) => Some(target),
                        Some(Err(why)) => {
                            notes.push(format!("{} ({}): [play.{}] artnet: {why}", side.role, side.address, side.role));
                            None
                        }
                        None => {
                            notes.push(format!(
                                "{} ({}): not sent (nothing binds it: [play.{}] artnet in the Setup)",
                                side.role, side.address, side.role
                            ));
                            None
                        }
                    }
                })
                .collect();
            match ArtNetSender::new(targets) {
                Ok(sender) => {
                    external.artnet =
                        Some((runner.send_data(signal_output::data_hop(rate)), sender));
                }
                Err(error) => notes.push(format!("Art-Net: {error}")),
            }
        }
        let mut inputs = Vec::new();
        let sides = signal_project::device::control_sides(project, plan);
        // The `osc` sides bound, one listener per address: roles bound to
        // the same address share its socket.
        let bound: Vec<(usize, (String, OscRoute))> = sides
            .iter()
            .enumerate()
            .filter(|(_, side)| side.kind != signal_project::device::MIDI)
            .filter_map(|(index, side)| {
                let address = setup.capture.get(&side.role)?.osc.clone()?;
                let route = OscRoute {
                    role: side.role.clone(),
                    names: side.ports.clone(),
                };
                Some((index, (address, route)))
            })
            .collect();
        let bindings: Vec<(String, OscRoute)> = bound.iter().map(|(_, b)| b.clone()).collect();
        let (listeners, opened) = signal_output::osc::listen_each(&bindings);
        external.osc = listeners;
        let mut opened: std::collections::BTreeMap<usize, signal_output::osc::Opened> =
            bound.iter().map(|(index, _)| *index).zip(opened).collect();
        for (index, side) in sides.into_iter().enumerate() {
            let binding = setup.capture.get(&side.role);
            let source: Box<dyn signal_output::inputs::InputSource> =
                if side.kind == signal_project::device::MIDI {
                    let Some(filter) = binding.and_then(|b| b.midi.as_deref()) else {
                        notes.push(format!(
                            "{} ({}): reads 0 (nothing binds it: [capture.{}] midi in the Setup)",
                            side.role, side.address, side.role
                        ));
                        continue;
                    };
                    match MidiFeed::start(filter, &side.ports) {
                        Ok((feed, source)) => {
                            external.midi.push(feed);
                            Box::new(source)
                        }
                        Err(why) => {
                            notes.push(format!("{} ({}): MIDI: {why}", side.role, side.address));
                            continue;
                        }
                    }
                } else {
                    let Some(address) = binding.and_then(|b| b.osc.as_deref()) else {
                        notes.push(format!(
                            "{} ({}): reads 0 (nothing binds it: [capture.{}] osc in the Setup)",
                            side.role, side.address, side.role
                        ));
                        continue;
                    };
                    match opened.remove(&index) {
                        Some(Ok((source, _))) => Box::new(source),
                        Some(Err(why)) => {
                            notes.push(format!(
                                "{} ({}): OSC on {address}: {why}",
                                side.role, side.address
                            ));
                            continue;
                        }
                        None => continue,
                    }
                };
            let (input, record) = RunInput::new(
                side.address.clone(),
                side.ports.len(),
                source,
                MAX_CALLBACK_FRAMES,
                record_frames,
            );
            inputs.push(input);
            external.records.extend(record);
        }
        runner.set_inputs(inputs);
        external
    }

    /// Sends the data frames waiting (control thread, while playing); a
    /// send failure to say ([`ArtNetSender::take_failure`]).
    pub fn poll(&mut self) -> Option<String> {
        let (rx, sender) = self.artnet.as_mut()?;
        sender.drain(rx);
        sender.take_failure()
    }

    /// The control inputs' records, once (a recording run's session).
    pub fn take_records(&mut self) -> Vec<InputRecord> {
        std::mem::take(&mut self.records)
    }
}
