//! ADR 0052 phase 2 without a device: a secondary output's two callback
//! bodies, the clock device's [`OutputCallback`] feeding its adapter and the
//! secondary device's [`SecondaryCallback`] pulling from it, on
//! `projects/cue-mix` with `cue` routed to another device.
//!
//! - Verification 7: the clock device's output is bit-identical with a
//!   secondary routed and without.
//! - Verification 6: neither body allocates or frees, a feed swapped into
//!   its slot mid-run included.
//! - Two clocks in virtual time (44.1 kHz at +50 ppm against 48 kHz): the
//!   secondary locks without a slip and plays the cue.

signal_contract::install_counting_allocator!();

use std::path::Path;
use std::sync::Arc;

use signal_audio::{
    CallbackStats, ChannelMap, OutputCallback, SecondaryCallback, SecondaryFeed, SecondaryState,
    SecondaryStats,
};
use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::Registry;
use signal_engine::RealtimeConfig;
use signal_output::adapter::{AdapterConfig, clock_adapter};
use signal_output::{
    CallbackSizes, MAIN, Placement, RoleBinding, Routing, Runner, SoundSystem, realtime_routed,
    route,
};
use signal_project::Project;

const RATE: u32 = 48_000;

/// `cue-mix` with `main` on the clock device's 1–2 and `cue` on another
/// device's 1–2: its Runner and routing.
fn cue_elsewhere() -> (Runner, Routing) {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/cue-mix");
    let project = Project::load(&dir).unwrap();
    assert_eq!(project.sample_rate, RATE);
    let plan = project.plan(&registry).unwrap().plan;
    let sides = signal_project::device::play_sides(&project, &plan);
    let system = SoundSystem {
        roles: [
            (MAIN.to_owned(), RoleBinding::clock(vec![1, 2])),
            (
                "cue".to_owned(),
                RoleBinding {
                    placement: Placement::Other("Speakers".to_owned()),
                    channels: vec![1, 2],
                },
            ),
        ]
        .into(),
    };
    let routing = route(&sides, &system, Some(2));
    // Frame order cue, main; the clock device plays main only.
    assert_eq!(routing.targets(), [None, None, Some(0), Some(1)]);
    let config = RealtimeConfig::new(RATE, project.seed);
    let (runner, _controller) =
        realtime_routed(&plan, &registry, config, None, &routing.addresses()).unwrap();
    (runner, routing)
}

/// A secondary route's two ends for a device at `rate` with callbacks of
/// `period` frames, the clock device's period being `clock_period`.
fn secondary(
    routing: &Routing,
    clock_period: usize,
    rate: u32,
    period: usize,
) -> (SecondaryFeed, SecondaryCallback, Arc<SecondaryStats>) {
    let routes = routing.secondaries();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.sources, [0, 1]);
    let config = AdapterConfig::new(route.sources.len(), RATE, rate, clock_period, period);
    let (input, output) = clock_adapter(config).unwrap();
    let stats = Arc::new(SecondaryStats::default());
    let feed = SecondaryFeed::new(input, route.sources.clone(), 1_024);
    let callback = SecondaryCallback::new(output, route.targets.clone(), 2_048, Arc::clone(&stats));
    (feed, callback, stats)
}

/// Verification 7: under the same callbacks, the clock device's output with
/// the cue fed to a secondary is the output without it, bit for bit.
#[test]
fn the_clock_output_is_bit_identical_with_a_secondary_routed() {
    let (alone, routing) = cue_elsewhere();
    let (fed, _) = cue_elsewhere();
    let map = || ChannelMap::with_routes(2, routing.targets()).unwrap();
    let stats = Arc::new(CallbackStats::default());
    let (mut a, _ca) = OutputCallback::new(alone, map(), 1_024, RATE, Arc::clone(&stats));
    let (b, _cb) = OutputCallback::new(fed, map(), 1_024, RATE, Arc::clone(&stats));
    let (mut b, mut feeds) = b.with_feeds(1);
    let (feed, mut device, _) = secondary(&routing, 256, RATE, 256);
    assert!(feeds.set(0, Some(feed)));
    let mut sizes = CallbackSizes::new(3);
    let mut out_a = vec![0.0f32; 1_024 * 2];
    let mut out_b = vec![0.0f32; 1_024 * 2];
    let mut pulled = vec![0.0f32; 256 * 2];
    let mut time = 0u64;
    let mut heard = 0.0f32;
    for n in 0..400 {
        let frames = sizes.next_size().min(1_024);
        a.fill(&mut out_a[..frames * 2]);
        b.fill_at(&mut out_b[..frames * 2], Some(time));
        assert_eq!(out_a[..frames * 2], out_b[..frames * 2], "callback {n}");
        heard = out_a.iter().fold(heard, |h, x| h.max(x.abs()));
        time += frames as u64 * 1_000_000_000 / u64::from(RATE);
        device.fill(&mut pulled, 2, Some(time));
    }
    assert!(heard > 0.0, "main is heard");
}

/// Verification 6: the clock callback with a feed and the secondary's
/// callback allocate and free nothing, a feed replaced in its slot
/// mid-run included (the old one goes back to the control thread).
#[test]
fn neither_callback_allocates_and_a_swapped_feed_is_dropped_off_the_audio_thread() {
    assert!(alloc_counter::is_installed());
    let (runner, routing) = cue_elsewhere();
    let map = ChannelMap::with_routes(2, routing.targets()).unwrap();
    let stats = Arc::new(CallbackStats::default());
    let (callback, _control) = OutputCallback::new(runner, map, 1_024, RATE, stats);
    let (mut callback, mut feeds) = callback.with_feeds(2);
    let (feed, mut device, _) = secondary(&routing, 512, RATE, 512);
    assert!(feeds.set(1, Some(feed)));
    let mut spare = None;
    let mut out = vec![0.0f32; 1_024 * 2];
    let mut pulled = vec![0.0f32; 512 * 2];
    let mut sizes = CallbackSizes::new(11);
    let mut total = Counts::default();
    let mut time = 0u64;
    for n in 0..1_500 {
        if n == 700 {
            // A new route for the device (a rebuilt adapter after it came
            // back), made on the control thread.
            let (feed, other, _) = secondary(&routing, 512, RATE, 512);
            assert!(feeds.set(1, Some(feed)));
            spare = Some(std::mem::replace(&mut device, other));
        }
        let frames = sizes.next_size().min(1_024);
        let guard = Guard::start();
        callback.fill_at(&mut out[..frames * 2], Some(time));
        device.fill(&mut pulled, 2, Some(time));
        let counts = guard.counts();
        drop(guard);
        total.allocations += counts.allocations;
        total.deallocations += counts.deallocations;
        time += frames as u64 * 1_000_000_000 / u64::from(RATE);
        feeds.collect();
    }
    drop(spare);
    assert!(total.is_zero(), "{total:?}");
}

/// Two clocks in virtual time: the clock device at 48 kHz with 256-frame
/// callbacks, the secondary at 44.1 kHz with 441-frame ones, its crystal
/// 50 ppm fast. The secondary locks within 10 s, slips never, reads a
/// drift of about 50 ppm, and plays the cue (not silence).
#[test]
fn a_drifting_secondary_at_another_rate_locks_and_plays_the_cue() {
    let (runner, routing) = cue_elsewhere();
    let map = ChannelMap::with_routes(2, routing.targets()).unwrap();
    let stats = Arc::new(CallbackStats::default());
    let (callback, _control) = OutputCallback::new(runner, map, 256, RATE, stats);
    let (mut callback, mut feeds) = callback.with_feeds(1);
    let (feed, mut device, status) = secondary(&routing, 256, 44_100, 441);
    assert!(feeds.set(0, Some(feed)));
    let clock_period = 256.0 / f64::from(RATE);
    let device_period = 441.0 / 44_100.0 / (1.0 + 50e-6);
    let (mut next_clock, mut next_device) = (0.0f64, 0.004f64);
    let mut out = vec![0.0f32; 256 * 2];
    let mut pulled = vec![0.0f32; 441 * 2];
    let mut locked_at = None;
    let mut slips_at_lock = 0;
    let mut cue_peak = 0.0f32;
    while next_device < 20.0 {
        if next_clock <= next_device {
            callback.fill_at(&mut out, Some((next_clock * 1e9) as u64));
            next_clock += clock_period;
        } else {
            device.fill(&mut pulled, 2, Some((next_device * 1e9) as u64));
            next_device += device_period;
            let now = status.snapshot();
            if locked_at.is_none() && now.state == SecondaryState::Locked {
                locked_at = Some(next_device);
                slips_at_lock = now.slips;
            }
            if locked_at.is_some() {
                cue_peak = pulled.iter().fold(cue_peak, |p, x| p.max(x.abs()));
            }
        }
    }
    let now = status.snapshot();
    let locked_at = locked_at.expect("the secondary locks");
    assert!(locked_at < 10.0, "locked at {locked_at:.1} s");
    assert_eq!(now.state, SecondaryState::Locked);
    assert_eq!(now.slips, slips_at_lock, "no slip once locked: {now:?}");
    assert_eq!(now.slips, 0, "{now:?}");
    assert!(
        (now.drift_ppm.abs() - 50.0).abs() < 5.0,
        "drift {:.1} ppm",
        now.drift_ppm
    );
    assert!(cue_peak > 0.01, "the cue plays: {cue_peak}");
    assert!(now.latency_frames > 0.0);
}
