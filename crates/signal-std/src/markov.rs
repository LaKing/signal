//! `std.markov` (ADR 0056 §4): a seeded state machine that moves on each
//! `step`, by the weights of a chain pattern (`kind = "chain"`,
//! `patterns/<name>.toml`).
//!
//! **Ports**
//! - `step` (events in): each `Trigger` draws the next state from the
//!   current state's row, with the instance's random stream (ADR 0008).
//! - `reset` (events in): back to state 0 (before a `step` on the same frame).
//! - `state` (signal out): the state, 0 … N − 1.
//! - `changed` (events out): a `Trigger` on each frame the state changes.
//!
//! **Its table:** compiled from the chain by [`CHAIN_RULE`]: one row per
//! state of the cumulative weights and their total, so a draw `u` in
//! `[0, total)` takes the first state whose cumulative weight exceeds it. A
//! row of zeros stays put; without a table the state stays 0.
//!
//! The same seed gives the same states; another seed, others. The draw is
//! one `next_f64` per step, whatever the row, so the stream stays aligned.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, RngStream, Table, TableRule,
};

use crate::{event_port, push_trigger, signal_port, triggered};

/// The kind of pattern `std.markov` takes.
pub const CHAIN: &str = "chain";

/// The rule `std.markov` is registered with.
pub const CHAIN_RULE: TableRule = TableRule {
    kind: CHAIN,
    compile: compile_chain,
};

/// Compiles a chain (header `[states]`, one row of `states` weights per
/// state) into rows of cumulative weights and their total.
pub fn compile_chain(header: &[f64], rows: &Table) -> Result<Table, String> {
    let states = header.first().copied().unwrap_or(0.0) as usize;
    if states == 0 || rows.width() != states || rows.rows() != states {
        return Err(format!(
            "a chain of {states} states has {states} rows of {states} weights"
        ));
    }
    let mut values = Vec::with_capacity(states * (states + 1));
    for i in 0..states {
        let mut sum = 0.0;
        for &w in rows.row(i) {
            if !(w.is_finite() && w >= 0.0) {
                return Err("a weight is a finite number, at least 0".to_owned());
            }
            sum += w;
            values.push(sum);
        }
        values.push(sum);
    }
    Table::new(states + 1, values).map_err(|e| e.to_string())
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.markov",
    doc: "A seeded state machine: each step draws the next state from the current state's weights in a chain pattern; state is 0..N-1, changed fires on a move, reset returns to 0 (ADR 0056).",
    ports: &[
        event_port("step", Direction::In),
        event_port("reset", Direction::In),
        signal_port("state", Direction::Out, PortKind::Generic, None, None),
        event_port("changed", Direction::Out),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.markov`.
#[derive(Clone, Debug, Default)]
pub struct Markov {
    seed: Option<RngStream>,
    rng: Option<RngStream>,
    state: usize,
}

/// The next state from `row` (cumulative weights, then their total) for a
/// draw `u` in `[0, 1)`; `current` when the total is 0.
fn next_state(row: &[f64], u: f64, current: usize) -> usize {
    let Some((&total, cumulative)) = row.split_last() else {
        return current;
    };
    if total <= 0.0 {
        return current;
    }
    let target = u * total;
    cumulative
        .iter()
        .position(|&c| target < c)
        .unwrap_or(cumulative.len().saturating_sub(1))
}

impl DirectPrimitive for Markov {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.seed = Some(ctx.rng.clone());
        self.reset();
    }

    fn reset(&mut self) {
        self.rng = self.seed.clone();
        self.state = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let table = inputs.table().filter(|t| t.width() == t.rows() + 1);
        let (step, reset) = (inputs.events(0), inputs.events(1));
        let (mut next_step, mut next_reset) = (0, 0);
        for n in 0..inputs.frames() {
            let before = self.state;
            if triggered(reset, &mut next_reset, n) {
                self.state = 0;
            }
            if triggered(step, &mut next_step, n)
                && let Some(rng) = self.rng.as_mut()
            {
                let u = rng.next_f64();
                if let Some(table) = table
                    && self.state < table.rows()
                {
                    self.state = next_state(table.row(self.state), u, self.state);
                }
            }
            if let Some(out) = outputs.signal(0).get_mut(n) {
                *out = self.state as f64;
            }
            if self.state != before {
                push_trigger(outputs, 0, n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draw_takes_the_first_state_past_it() {
        // Weights 1, 0, 3: cumulative 1, 1, 4; total 4.
        let row = [1.0, 1.0, 4.0, 4.0];
        assert_eq!(next_state(&row, 0.0, 9), 0);
        assert_eq!(next_state(&row, 0.24, 9), 0);
        assert_eq!(next_state(&row, 0.25, 9), 2, "a zero weight is never drawn");
        assert_eq!(next_state(&row, 0.999, 9), 2);
        assert_eq!(
            next_state(&[0.0, 0.0, 0.0], 0.5, 1),
            1,
            "a row of zeros stays"
        );
    }

    #[test]
    fn a_chain_compiles_to_cumulative_rows() {
        let rows = Table::new(2, vec![1.0, 3.0, 0.0, 0.0]).unwrap();
        let table = compile_chain(&[2.0], &rows).unwrap();
        assert_eq!(table.row(0), [1.0, 4.0, 4.0]);
        assert_eq!(table.row(1), [0.0, 0.0, 0.0]);
        assert!(compile_chain(&[3.0], &rows).is_err());
    }
}
