//! The sequencers' instance tables (ADR 0028 §4): what `std.note_seq` and
//! `std.step_seq8` read, compiled from a pattern file's canonical source
//! (`docs/project-format.md` §12) when the plan is built, never on the audio
//! thread. Keys, notes, steps and lanes are this library's vocabulary; the
//! core only carries the rows.

use signal_core::{Table, TableRule};

/// The pattern kind `std.note_seq` takes.
pub const NOTES: &str = "notes";
/// The pattern kind `std.step_seq8` takes.
pub const STEPS: &str = "steps";

/// The lowest and highest key `std.note_seq` plays (60 = middle C), so
/// `hz = 440·2^((key − 69)/12)` stays below 12.6 kHz.
pub const KEYS: (f64, f64) = (0.0, 127.0);

/// The most lanes `std.step_seq8` plays.
pub const LANES: usize = 8;

/// `std.note_seq`'s table, row 0 `[length, 0, 0, 0, 0]`, then one row per
/// note that starts before `length`, in the order of the source (by start,
/// then key): `[start, length, key, velocity, hz]`, beats and Hz.
pub(crate) const NOTE_WIDTH: usize = 5;

/// `std.step_seq8`'s table, row 0 `[steps, step, level₁ … level₈]`, then
/// one row per step: `[cell₁ … cell₈, accented, 0]`, a cell 0 (rest), 1
/// (hit) or 2 (accent), `accented` 1 when a lane of the step is accented.
pub(crate) const STEP_WIDTH: usize = LANES + 2;

/// The rule `std.note_seq` is registered with.
pub const NOTE_RULE: TableRule = TableRule {
    kind: NOTES,
    compile: compile_notes,
};

/// The rule `std.step_seq8` is registered with.
pub const STEP_RULE: TableRule = TableRule {
    kind: STEPS,
    compile: compile_steps,
};

/// `hz = 440·2^((key − 69)/12)`.
pub(crate) fn hz(key: f64) -> f64 {
    440.0 * libm::exp2((key - 69.0) / 12.0)
}

/// Compiles a `notes` source (header `[length]`, rows `[key, start, length,
/// velocity]` sorted by start and key) into `std.note_seq`'s table. Refuses
/// a key outside [`KEYS`]. Notes starting at or after `length` are left out:
/// they never play.
pub fn compile_notes(header: &[f64], rows: &Table) -> Result<Table, String> {
    let length = header.first().copied().unwrap_or(0.0);
    let mut values = vec![length, 0.0, 0.0, 0.0, 0.0];
    for i in 0..rows.rows() {
        let &[key, start, len, velocity] = rows.row(i) else {
            return Err("a note has 4 values: key, start, length, velocity".to_owned());
        };
        if !(KEYS.0..=KEYS.1).contains(&key) {
            return Err(format!(
                "the note at beat {start} has key {key}; std.note_seq plays keys {} to {}",
                KEYS.0, KEYS.1
            ));
        }
        if start < length {
            values.extend([start, len, key, velocity, hz(key)]);
        }
    }
    Table::new(NOTE_WIDTH, values).map_err(|e| e.to_string())
}

/// Compiles a `steps` source (header `[steps, step]`, one row `[level,
/// cells…]` per lane) into `std.step_seq8`'s table. Refuses more than
/// [`LANES`] lanes.
pub fn compile_steps(header: &[f64], rows: &Table) -> Result<Table, String> {
    let &[steps, step] = header else {
        return Err("a step pattern's header is steps and step".to_owned());
    };
    let lanes = rows.rows();
    if lanes > LANES {
        return Err(format!(
            "{lanes} lanes; std.step_seq8 plays at most {LANES}"
        ));
    }
    let count = steps as usize;
    let mut values = vec![0.0; STEP_WIDTH * (count + 1)];
    let mut set = |row: usize, column: usize, value: f64| {
        if let Some(v) = values.get_mut(row * STEP_WIDTH + column) {
            *v = value;
        }
    };
    set(0, 0, steps);
    set(0, 1, step);
    for lane in 0..lanes {
        let row = rows.row(lane);
        set(0, 2 + lane, row.first().copied().unwrap_or(1.0));
        for s in 0..count {
            let cell = row.get(1 + s).copied().unwrap_or(0.0);
            set(1 + s, lane, cell);
            if cell == 2.0 {
                set(1 + s, LANES, 1.0);
            }
        }
    }
    Table::new(STEP_WIDTH, values).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_compile_with_their_frequency_and_drop_what_never_plays() {
        let source = Table::new(4, vec![69.0, 0.0, 1.0, 0.5, 60.0, 4.0, 1.0, 1.0]).unwrap();
        let table = compile_notes(&[4.0], &source).unwrap();
        assert_eq!(table.rows(), 2);
        assert_eq!(table.row(1), [0.0, 1.0, 69.0, 0.5, 440.0]);
        let high = Table::new(4, vec![128.0, 0.0, 1.0, 1.0]).unwrap();
        assert!(
            compile_notes(&[4.0], &high)
                .unwrap_err()
                .contains("key 128")
        );
        assert!((hz(81.0) - 880.0).abs() < 1e-9);
    }

    #[test]
    fn steps_compile_by_step_with_accents() {
        let source = Table::new(3, vec![0.5, 1.0, 2.0, 1.0, 0.0, 1.0]).unwrap();
        let table = compile_steps(&[2.0, 0.25], &source).unwrap();
        assert_eq!(table.rows(), 3);
        assert_eq!(table.row(0)[..4], [2.0, 0.25, 0.5, 1.0]);
        assert_eq!(
            table.row(1)[..LANES + 1],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            table.row(2)[..LANES + 1],
            [2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
        );
        let nine = Table::new(2, [1.0, 1.0].repeat(9)).unwrap();
        assert!(
            compile_steps(&[1.0, 1.0], &nine)
                .unwrap_err()
                .contains("9 lanes")
        );
    }
}
