//! ADR 0036's measurement probe, run by hand on a machine with the device
//! (ignored by default; it reads devices and changes none):
//!
//! ```sh
//! SIGNAL_PROBE_DEVICE="BlackHole 64ch" SIGNAL_PROBE_SECONDS=3 \
//!   cargo test -p signal-audio --test device_state -- --ignored --nocapture
//! ```
//!
//! prints, every 20 ms for `SIGNAL_PROBE_SECONDS` (one read without it), a
//! line of the output device's shared state whenever it changes: whether it
//! runs somewhere, its I/O buffer, its nominal rate and its physical format
//! as cpal reads it. Without `SIGNAL_PROBE_DEVICE`, every output device is
//! read once.

use std::time::{Duration, Instant};

use signal_audio::{DeviceSelector, Direction, device_state, devices, physical_format};

fn line(selector: &DeviceSelector) -> String {
    let state = device_state(Direction::Output, selector);
    let format = physical_format(Direction::Output, selector);
    format!("{state:?} physical {format:?}")
}

#[test]
#[ignore = "reads the machine's audio devices; run by hand (ADR 0036)"]
fn print_the_device_state() {
    let Ok(name) = std::env::var("SIGNAL_PROBE_DEVICE") else {
        for device in devices().unwrap() {
            if device.direction == Direction::Output {
                let selector = DeviceSelector::Name(device.name.clone());
                println!("{}: {}", device.name, line(&selector));
            }
        }
        return;
    };
    let selector = DeviceSelector::Name(name.clone());
    let seconds: f64 = std::env::var("SIGNAL_PROBE_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let start = Instant::now();
    let mut last = String::new();
    loop {
        let now = line(&selector);
        if now != last {
            println!("{:8.3} s {name}: {now}", start.elapsed().as_secs_f64());
            last = now;
        }
        if start.elapsed().as_secs_f64() >= seconds {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// ADR 0036: how soon after Signal's own stream stops the device no longer
/// reads as running (a restart right after a stop must not take Signal's
/// own last stream for another app's). Plays `SIGNAL_PROBE_PROJECT` (a
/// silent scratch copy: `audio_out` gain 0) on `SIGNAL_PROBE_DEVICE` for
/// half a second, stops, and prints the state right after and later.
#[test]
#[ignore = "plays on a device; run by hand with a silent project (ADR 0036)"]
fn the_state_right_after_a_stop() {
    use signal_audio::CoreAudioDriver;
    use signal_core::Registry;
    use signal_engine::RealtimeConfig;
    use signal_output::{Driver, realtime};
    use signal_project::Project;

    let (Ok(name), Ok(dir)) = (
        std::env::var("SIGNAL_PROBE_DEVICE"),
        std::env::var("SIGNAL_PROBE_PROJECT"),
    ) else {
        return;
    };
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let project = Project::load(std::path::Path::new(&dir)).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (rate, seed) = (project.sample_rate, project.seed);
    let (runner, _controller) =
        realtime(&plan, &registry, RealtimeConfig::new(rate, seed)).unwrap();
    let selector = DeviceSelector::Name(name.clone());
    let mut driver = CoreAudioDriver::open_selected(&selector, rate, 512, 2, None).unwrap();
    println!("before: {}", line(&selector));
    driver.start(runner).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    println!("playing: {}", line(&selector));
    let stopped = Instant::now();
    driver.stop().unwrap();
    println!("stop took {:?}", stopped.elapsed());
    for wait in [0, 5, 20, 100] {
        std::thread::sleep(Duration::from_millis(wait));
        println!("+{:?}: {}", stopped.elapsed(), line(&selector));
    }
}
