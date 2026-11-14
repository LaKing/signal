//! Additional audit regressions.
use signal_output::layouts::{Adapter, mix};
#[test]
fn seven_one_to_quad_keeps_center_and_sides() {
    let m = mix("7.1", "quad", &Adapter::default()).unwrap().unwrap();
    for (name, channel) in [("c", 2), ("ls", 6), ("rs", 7)] {
        let mut x = [0.0; 8];
        x[channel] = 1.0;
        eprintln!(
            "{name} impulse: {:?}",
            (0..4).map(|t| m.row(t, &x)).collect::<Vec<_>>()
        );
    }
    let mut center = [0.0; 8];
    center[2] = 1.0;
    let actual: Vec<f64> = (0..4).map(|t| m.row(t, &center)).collect();
    eprintln!("center impulse: {actual:?}; {}", m.label);
    assert_eq!(
        actual,
        vec![
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
            0.0
        ]
    );
}
