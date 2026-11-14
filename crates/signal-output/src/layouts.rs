//! Layout adapters (ADR 0038 §2, §5): output policy (ADR 0022). Where a
//! project's layout for a role differs from the machine's, the audio adapter
//! converts, never the engine and never a render. Here, on the control
//! thread: the layouts and their channels, and the matrix a conversion is
//! ([`Mix`]), which the Runner applies per frame (gain → matrix → trim →
//! clamp) without allocating.
//!
//! - **Downmix (`itu`, the default):** ITU-R BS.775's rows, with
//!   *k* = 1/√2: 5.1 → stereo, 5.1 → quad, 5.1 → mono, quad → stereo,
//!   stereo → mono, and Signal's own 7.1 → 5.1 (side and back pair folded at
//!   −3 dB each). Every other downmix is their composition along
//!   7.1 → 5.1 → stereo → mono (quad → stereo → mono). No normalisation:
//!   a downmix may exceed full scale, and the device's clamp counts it.
//! - **LFE** is not in a downmix unless `lfe` (dB) says so; a target with
//!   an LFE channel always gets it as it is.
//! - **Upmix invents no surround:** channels of the same name are copied,
//!   the rest are exact 0; quad's back pair goes to 5.1's sides; mono goes
//!   to C where the target has one, else to L and R at *k* each.
//! - **`direct`:** copy by name only, no fold-down.
//! - **`matrix`:** the machine's own rows, keyed by channel names, for one
//!   source layout; an unknown name or a non-finite coefficient is refused
//!   (the caller falls back to `itu` and says why).

use std::collections::BTreeMap;
use std::f64::consts::FRAC_1_SQRT_2 as K;

/// The layouts of ADR 0038 §2, by name, with their channels in canonical
/// order (a render's WAV channel order).
pub const LAYOUTS: [(&str, &[&str]); 5] = [
    ("mono", &["m"]),
    ("stereo", &["l", "r"]),
    ("quad", &["l", "r", "lb", "rb"]),
    ("5.1", &["l", "r", "c", "lfe", "ls", "rs"]),
    ("7.1", &["l", "r", "c", "lfe", "lb", "rb", "ls", "rs"]),
];

/// The channel names of layout `name`, if it is one.
pub fn layout_channels(name: &str) -> Option<&'static [&'static str]> {
    LAYOUTS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// The layout of `width` channels, if one has that many (1 mono, 2 stereo,
/// 4 quad, 6 5.1, 8 7.1): a play side's layout by its width, and a binding's
/// by its channel count (ADR 0038 §4).
pub fn layout_by_width(width: usize) -> Option<&'static str> {
    LAYOUTS
        .iter()
        .find(|(_, c)| c.len() == width)
        .map(|(n, _)| *n)
}

/// How a machine converts a role's layout (ADR 0038 §5.6): the Setup's or
/// `devices.local.toml`'s `adapter` for the role.
#[derive(Clone, Debug, PartialEq)]
pub enum Adapter {
    /// ITU-R BS.775's downmix: the LFE into the fold-down at `lfe` dB (none:
    /// left out), then `trim` dB.
    Itu {
        /// The LFE's level in a fold-down, in dB; `None`: off.
        lfe: Option<f64>,
        /// After the matrix, in dB.
        trim: f64,
    },
    /// Copy by name only: a channel the target lacks is not heard.
    Direct,
    /// The machine's own matrices, one per source layout (`from`), each
    /// target channel's row keyed by source channel names; a source layout
    /// with none converts as `Itu` with these `lfe` and `trim`.
    Matrix {
        /// By source layout: target channel → source channel → coefficient.
        rows: BTreeMap<String, BTreeMap<String, BTreeMap<String, f64>>>,
        /// For a source layout without its own matrix.
        lfe: Option<f64>,
        /// After the matrix, in dB.
        trim: f64,
    },
}

impl Default for Adapter {
    fn default() -> Adapter {
        Adapter::Itu {
            lfe: None,
            trim: 0.0,
        }
    }
}

/// A conversion of a play side from layout `from` to the machine's `to`:
/// `rows[t * from_width + s]` is source channel `s`'s share of target
/// channel `t`, then `trim` (linear) on every target channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Mix {
    /// The project's layout.
    pub from: &'static str,
    /// The machine's.
    pub to: &'static str,
    /// Target by source, row-major.
    pub rows: Vec<f64>,
    /// After the matrix, linear.
    pub trim: f64,
    /// What it is, as the UI and `signal play` say it (ADR 0038 §5.8):
    /// `5.1 → stereo (ITU, LFE off)`.
    pub label: String,
}

impl Mix {
    /// The project's channels.
    pub fn from_width(&self) -> usize {
        self.rows.len() / self.to_width().max(1)
    }

    /// The machine's channels.
    pub fn to_width(&self) -> usize {
        layout_channels(self.to).map_or(0, <[_]>::len)
    }

    /// Target channel `t`'s value from source frame `x` (before `trim`);
    /// 0 for a channel it does not have. Real-time safe.
    pub fn row(&self, t: usize, x: &[f64]) -> f64 {
        let width = x.len();
        self.rows
            .get(t * width..(t + 1) * width)
            .map_or(0.0, |row| row.iter().zip(x).map(|(c, v)| c * v).sum())
    }
}

/// `dB` as a linear factor.
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A matrix by channel names: target → (source → coefficient).
type Named = Vec<(&'static str, Vec<(&'static str, f64)>)>;

/// The BS.775 row from `from` to the next layout down, if `from` has one.
fn step_down(from: &str) -> Option<(&'static str, Named)> {
    Some(match from {
        "7.1" => (
            "5.1",
            vec![
                ("l", vec![("l", 1.0)]),
                ("r", vec![("r", 1.0)]),
                ("c", vec![("c", 1.0)]),
                ("lfe", vec![("lfe", 1.0)]),
                ("ls", vec![("ls", K), ("lb", K)]),
                ("rs", vec![("rs", K), ("rb", K)]),
            ],
        ),
        "5.1" => (
            "stereo",
            vec![
                ("l", vec![("l", 1.0), ("c", K), ("ls", K)]),
                ("r", vec![("r", 1.0), ("c", K), ("rs", K)]),
            ],
        ),
        "quad" => (
            "stereo",
            vec![
                ("l", vec![("l", 1.0), ("lb", K)]),
                ("r", vec![("r", 1.0), ("rb", K)]),
            ],
        ),
        "stereo" => ("mono", vec![("m", vec![("l", K), ("r", K)])]),
        _ => return None,
    })
}

/// The direct BS.775 rows that are not a step of [`step_down`] (5.1 → quad,
/// whose target is not on the 5.1 → stereo path).
fn direct_row(from: &str, to: &str) -> Option<Named> {
    match (from, to) {
        ("5.1", "quad") => Some(vec![
            ("l", vec![("l", 1.0), ("c", K)]),
            ("r", vec![("r", 1.0), ("c", K)]),
            ("lb", vec![("ls", 1.0)]),
            ("rb", vec![("rs", 1.0)]),
        ]),
        _ => None,
    }
}

/// Adds `c` to `rows[at]` (a matrix cell; one past the end is no cell).
fn add(rows: &mut [f64], at: usize, c: f64) {
    if let Some(cell) = rows.get_mut(at) {
        *cell += c;
    }
}

/// Sets `rows[at]` to `c`.
fn set_cell(rows: &mut [f64], at: usize, c: f64) {
    if let Some(cell) = rows.get_mut(at) {
        *cell = c;
    }
}

/// `named` as a dense matrix from `from`'s channels to `to`'s.
fn dense(from: &str, to: &str, named: &Named) -> Vec<f64> {
    let source = layout_channels(from).unwrap_or(&[]);
    let target = layout_channels(to).unwrap_or(&[]);
    let mut rows = vec![0.0; target.len() * source.len()];
    for (t, name) in target.iter().enumerate() {
        if let Some((_, row)) = named.iter().find(|(n, _)| n == name) {
            for (channel, c) in row {
                if let Some(s) = source.iter().position(|n| n == channel) {
                    add(&mut rows, t * source.len() + s, *c);
                }
            }
        }
    }
    rows
}

/// `b` after `a`: `a` from x to y channels, `b` from y to z.
fn compose(a: &[f64], b: &[f64], x: usize, y: usize, z: usize) -> Vec<f64> {
    let mut out = vec![0.0; z * x];
    let at = |m: &[f64], i: usize| m.get(i).copied().unwrap_or(0.0);
    for t in 0..z {
        for s in 0..x {
            set_cell(
                &mut out,
                t * x + s,
                (0..y).map(|m| at(b, t * y + m) * at(a, m * x + s)).sum(),
            );
        }
    }
    out
}

/// The `itu` downmix from `from` to a smaller `to`: the direct row, else the
/// composition of steps down; `None` when `to` is not below `from`.
fn downmix(from: &str, to: &str) -> Option<Vec<f64>> {
    let width = |l: &str| layout_channels(l).map_or(0, <[_]>::len);
    let mut at = from;
    let mut rows: Option<Vec<f64>> = None;
    while at != to {
        // A direct row from here to `to` (5.1 → quad, also after 7.1 → 5.1:
        // the audit of 2026-11-14), else the next step down.
        let (next, named) = match direct_row(at, to) {
            Some(named) => (to, named),
            None => step_down(at)?,
        };
        let step = dense(at, next, &named);
        rows = Some(match rows {
            None => step,
            Some(before) => compose(&before, &step, width(from), width(at), width(next)),
        });
        at = next;
    }
    rows
}

/// Copy by name; the target's other channels 0 (`direct`, and the upmix's
/// base).
fn by_name(from: &str, to: &str) -> Vec<f64> {
    let source = layout_channels(from).unwrap_or(&[]);
    let target = layout_channels(to).unwrap_or(&[]);
    let mut rows = vec![0.0; target.len() * source.len()];
    for (t, name) in target.iter().enumerate() {
        if let Some(s) = source.iter().position(|n| n == name) {
            set_cell(&mut rows, t * source.len() + s, 1.0);
        }
    }
    rows
}

/// The upmix (ADR 0038 §5.3): by name; quad's back pair to the sides of a
/// target without a back pair; mono to C, else to L and R at *k*.
fn upmix(from: &str, to: &str) -> Vec<f64> {
    let source = layout_channels(from).unwrap_or(&[]);
    let target = layout_channels(to).unwrap_or(&[]);
    let mut rows = by_name(from, to);
    let mut set = |t: &str, s: &str, c: f64| {
        if let (Some(t), Some(s)) = (
            target.iter().position(|n| *n == t),
            source.iter().position(|n| *n == s),
        ) {
            set_cell(&mut rows, t * source.len() + s, c);
        }
    };
    if from == "quad" && !target.contains(&"lb") {
        set("ls", "lb", 1.0);
        set("rs", "rb", 1.0);
    }
    if from == "mono" {
        if target.contains(&"c") {
            set("c", "m", 1.0);
        } else {
            set("l", "m", K);
            set("r", "m", K);
        }
    }
    rows
}

/// Whether `to` is below `from` on the downmix path (a fold-down).
fn folds(from: &str, to: &str) -> bool {
    downmix(from, to).is_some()
}

/// The LFE in a fold-down at `db`: `10^(db/20)·lfe` into L and R (into M
/// for mono), when `from` has an LFE and `to` has none.
fn add_lfe(rows: &mut [f64], from: &str, to: &str, db: f64) {
    let source = layout_channels(from).unwrap_or(&[]);
    let target = layout_channels(to).unwrap_or(&[]);
    let (Some(lfe), false) = (
        source.iter().position(|n| *n == "lfe"),
        target.contains(&"lfe"),
    ) else {
        return;
    };
    for (t, name) in target.iter().enumerate() {
        if matches!(*name, "l" | "r" | "m") {
            add(rows, t * source.len() + lfe, db_to_linear(db));
        }
    }
}

/// A user matrix by names, dense, or why it does not read.
fn user_matrix(
    from: &'static str,
    to: &'static str,
    named: &BTreeMap<String, BTreeMap<String, f64>>,
) -> Result<Vec<f64>, String> {
    let source = layout_channels(from).unwrap_or(&[]);
    let target = layout_channels(to).unwrap_or(&[]);
    let mut rows = vec![0.0; target.len() * source.len()];
    for (t_name, row) in named {
        let Some(t) = target.iter().position(|n| n == t_name) else {
            return Err(format!("`{t_name}` is no channel of {to}"));
        };
        for (s_name, &c) in row {
            let Some(s) = source.iter().position(|n| n == s_name) else {
                return Err(format!("`{s_name}` is no channel of {from}"));
            };
            if !c.is_finite() {
                return Err(format!("{t_name}.{s_name} is not a finite number"));
            }
            set_cell(&mut rows, t * source.len() + s, c);
        }
    }
    Ok(rows)
}

fn lfe_label(lfe: Option<f64>) -> String {
    match lfe {
        None => "LFE off".to_owned(),
        Some(db) => format!("LFE {} dB", format_db(db)),
    }
}

fn format_db(db: f64) -> String {
    let text = format!("{db}");
    text.replace('-', "−")
}

/// The conversion of a side of layout `from` on a machine role of layout
/// `to` with `adapter`, or `None` when they are the same layout. A custom
/// matrix that does not read is reported in the `Err`, with the `itu`
/// conversion to use instead.
pub fn mix(
    from: &'static str,
    to: &'static str,
    adapter: &Adapter,
) -> Option<Result<Mix, (String, Mix)>> {
    if from == to {
        return None;
    }
    let itu = |lfe: Option<f64>, trim: f64| -> Mix {
        let fold = folds(from, to);
        let mut rows = if fold {
            downmix(from, to).unwrap_or_else(|| by_name(from, to))
        } else {
            upmix(from, to)
        };
        let label = if fold {
            if let Some(db) = lfe {
                add_lfe(&mut rows, from, to, db);
            }
            let lfe_part = if layout_channels(from).is_some_and(|c| c.contains(&"lfe"))
                && !layout_channels(to).is_some_and(|c| c.contains(&"lfe"))
            {
                format!(", {}", lfe_label(lfe))
            } else {
                String::new()
            };
            format!("{from} → {to} (ITU{lfe_part})")
        } else {
            let kept: Vec<String> = layout_channels(from)
                .unwrap_or(&[])
                .iter()
                .map(|c| c.to_uppercase())
                .collect();
            format!("{from} → {to} ({} only)", kept.join("/"))
        };
        Mix {
            from,
            to,
            rows,
            trim: db_to_linear(trim),
            label: with_trim(label, trim),
        }
    };
    Some(match adapter {
        Adapter::Itu { lfe, trim } => Ok(itu(*lfe, *trim)),
        Adapter::Direct => Ok(Mix {
            from,
            to,
            rows: by_name(from, to),
            trim: 1.0,
            label: format!("{from} → {to} (direct)"),
        }),
        Adapter::Matrix { rows, lfe, trim } => match rows.get(from) {
            None => Ok(itu(*lfe, *trim)),
            Some(named) => match user_matrix(from, to, named) {
                Ok(dense) => Ok(Mix {
                    from,
                    to,
                    rows: dense,
                    trim: db_to_linear(*trim),
                    label: with_trim(format!("{from} → {to} (matrix, this machine)"), *trim),
                }),
                Err(why) => Err((
                    format!("the {from} matrix: {why}; the ITU fold-down plays instead"),
                    itu(*lfe, *trim),
                )),
            },
        },
    })
}

fn with_trim(label: String, trim: f64) -> String {
    if trim == 0.0 {
        label
    } else {
        let mut label = label;
        label.insert_str(label.len() - 1, &format!(", trim {} dB", format_db(trim)));
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn itu() -> Adapter {
        Adapter::default()
    }

    fn rows(from: &'static str, to: &'static str, adapter: &Adapter) -> Vec<f64> {
        mix(from, to, adapter).unwrap().unwrap().rows
    }

    /// ADR 0038 test 4: an impulse on each of l, r, c, lfe, ls, rs gives the
    /// `itu` row's coefficients on L and R; the LFE is silent by default and
    /// 0.31623 at −10 dB.
    #[test]
    fn five_one_folds_to_stereo_by_bs775() {
        assert_eq!(
            rows("5.1", "stereo", &itu()),
            [1.0, 0.0, K, 0.0, K, 0.0, 0.0, 1.0, K, 0.0, 0.0, K]
        );
        let lfe = rows(
            "5.1",
            "stereo",
            &Adapter::Itu {
                lfe: Some(-10.0),
                trim: 0.0,
            },
        );
        assert!((lfe[3] - 0.316_227_766).abs() < 1e-9 && lfe[3] == lfe[9]);
        let label = |a: &Adapter| mix("5.1", "stereo", a).unwrap().unwrap().label;
        assert_eq!(label(&itu()), "5.1 → stereo (ITU, LFE off)");
        assert_eq!(
            label(&Adapter::Itu {
                lfe: Some(-10.0),
                trim: 0.0
            }),
            "5.1 → stereo (ITU, LFE −10 dB)"
        );
        assert_eq!(
            label(&Adapter::Itu {
                lfe: None,
                trim: -3.0
            }),
            "5.1 → stereo (ITU, LFE off, trim −3 dB)"
        );
    }

    /// The composition 5.1 → stereo → mono equals the direct row
    /// (M = k·l + k·r + c + 0.5·ls + 0.5·rs).
    #[test]
    fn compositions_equal_the_direct_rows() {
        let mono = rows("5.1", "mono", &itu());
        let want = [K, K, 1.0, 0.0, 0.5, 0.5];
        for (a, b) in mono.iter().zip(want) {
            assert!((a - b).abs() < 1e-15, "{mono:?}");
        }
        // 7.1 → stereo: L = l + k·c + 0.5·ls + 0.5·lb.
        let seven = rows("7.1", "stereo", &itu());
        let l = &seven[..8];
        let want = [1.0, 0.0, K, 0.0, 0.5, 0.0, 0.5, 0.0];
        for (a, b) in l.iter().zip(want) {
            assert!((a - b).abs() < 1e-15, "{l:?}");
        }
        assert_eq!(
            rows("5.1", "quad", &itu()),
            [
                1.0, 0.0, K, 0.0, 0.0, 0.0, //
                0.0, 1.0, K, 0.0, 0.0, 0.0, //
                0.0, 0.0, 0.0, 0.0, 1.0, 0.0, //
                0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ]
        );
    }

    /// Every pair of layouts converts: a fold-down by BS.775, an upmix by
    /// name with exact zeros, quad's back pair to 5.1's sides, mono to C or
    /// to L and R at k.
    #[test]
    fn every_pair_converts() {
        for (from, _) in LAYOUTS {
            for (to, _) in LAYOUTS {
                let converted = mix(from, to, &itu());
                assert_eq!(converted.is_none(), from == to, "{from} → {to}");
                if let Some(m) = converted {
                    let m = m.unwrap();
                    assert_eq!(m.rows.len(), m.to_width() * m.from_width(), "{from} → {to}");
                    assert!(m.rows.iter().all(|c| c.is_finite()));
                    // A fold-down loses no channel but the LFE (the audit of
                    // 2026-11-14: 7.1 → quad lost C, Ls and Rs).
                    if folds(from, to) {
                        let w = m.from_width();
                        for (s, name) in layout_channels(from).unwrap().iter().enumerate() {
                            let reached = (0..m.to_width()).any(|t| m.rows[t * w + s] != 0.0);
                            assert!(reached || *name == "lfe", "{from} → {to}: {name} lost");
                        }
                    }
                }
            }
        }
        assert_eq!(rows("stereo", "5.1", &itu()), {
            let mut r = vec![0.0; 12];
            r[0] = 1.0;
            r[3] = 1.0;
            r
        });
        let quad = rows("quad", "5.1", &itu());
        assert_eq!(quad[4 * 4 + 2], 1.0, "lb → ls");
        assert_eq!(quad[5 * 4 + 3], 1.0, "rb → rs");
        assert_eq!(rows("mono", "5.1", &itu())[2], 1.0, "m → c");
        assert_eq!(rows("mono", "stereo", &itu()), [K, K]);
        assert_eq!(
            mix("stereo", "5.1", &itu()).unwrap().unwrap().label,
            "stereo → 5.1 (L/R only)"
        );
    }

    /// `direct` copies by name; a custom matrix applies exactly; a bad one
    /// falls back to `itu` with its reason.
    #[test]
    fn direct_and_matrix_adapters() {
        assert_eq!(
            rows("5.1", "stereo", &Adapter::Direct),
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]
        );
        let row = |pairs: &[(&str, f64)]| {
            pairs
                .iter()
                .map(|(n, c)| ((*n).to_owned(), *c))
                .collect::<BTreeMap<_, _>>()
        };
        let mut five = BTreeMap::new();
        five.insert(
            "l".to_owned(),
            row(&[("l", 1.0), ("c", 0.5), ("lfe", 0.25)]),
        );
        five.insert("r".to_owned(), row(&[("r", 1.0), ("c", 0.5)]));
        let mut by_from = BTreeMap::new();
        by_from.insert("5.1".to_owned(), five);
        let custom = Adapter::Matrix {
            rows: by_from.clone(),
            lfe: None,
            trim: 0.0,
        };
        assert_eq!(
            rows("5.1", "stereo", &custom),
            [1.0, 0.0, 0.5, 0.25, 0.0, 0.0, 0.0, 1.0, 0.5, 0.0, 0.0, 0.0]
        );
        by_from
            .get_mut("5.1")
            .unwrap()
            .insert("x".to_owned(), row(&[("l", 1.0)]));
        let bad = Adapter::Matrix {
            rows: by_from,
            lfe: None,
            trim: 0.0,
        };
        let (why, fallback) = mix("5.1", "stereo", &bad).unwrap().unwrap_err();
        assert!(why.contains("`x` is no channel of stereo"), "{why}");
        assert_eq!(fallback.rows, rows("5.1", "stereo", &itu()));
    }
}
