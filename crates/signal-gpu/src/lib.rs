//! Signal's GPU device service (ADR 0019 §3).
//!
//! A [`GpuService`] owns one wgpu adapter and device (Metal backend), created
//! on the control side, and one dedicated thread, `signal-gpu-<id>`, which is
//! the only thread that submits work to that device or polls it. A GPU-backed module
//! talks to it through a [`Channel`]: a fixed set of [`Job`] buffers moved
//! between the module and the GPU thread through two lock-free `rtrb` SPSC
//! queues (submit, done), whose atomic indices are the completion signal.
//! Submitting, polling and recycling never block, lock or allocate, so they
//! may run on the audio thread or the real-time workers; the GPU work itself
//! stays on the GPU thread (and, offline, whoever the engine waits on: never
//! in real time).
//!
//! M2.7-5: a channel that cannot run is refused when it is made
//! ([`GpuService::channel`]); every job ends with an explicit [`Outcome`];
//! a job belongs to its channel ([`Channel::recycle`]); jobs complete
//! asynchronously, at most [`MAX_IN_FLIGHT`] at a time, so a slow job holds
//! back no other channel; and the service is `running → draining → stopped`,
//! or `lost` from any state ([`ServiceState`]), with a bounded
//! [`shutdown`](GpuService::shutdown).
//!
//! M2.7-2: every adapter of the machine is a device ([`inventory`]). A
//! [`DevicePool`] owns one service per device (there is no process-wide
//! service), replaces a lost one with a new generation, and lends GPU-backed
//! instances their device by a deterministic placement ([`GpuHost`]); a
//! module's [`PlacedChannel`] takes the replacement without blocking. Large
//! work may be split across devices only through a fixed split and combine
//! order ([`SplitChannel`]).
//!
//! A machine without a usable adapter has an empty pool: GPU-backed modules
//! then have no channel and the engine reports their due results as lost.
//! The GPU path is always optional; the CPU stays the deterministic default.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

mod inventory;
mod placement;
mod pool;
mod split;

pub use inventory::{AdapterInfo, AdapterLimits, Inventory, inventory};
pub use placement::{GpuHost, Placed};
pub use pool::{DevicePool, Lease, PlacedChannel, Selection, SlotStatus};
pub use split::SplitChannel;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the GPU thread sleeps when nothing progressed (it never parks,
/// so a submitting thread never has to wake it: no system call on the audio
/// side).
pub const IDLE_SLEEP: Duration = Duration::from_micros(50);

/// How long it sleeps while no module has a channel (a new channel waits at
/// most this long to be served, well within a block).
pub const UNUSED_SLEEP: Duration = Duration::from_millis(1);

/// The most jobs in flight on the device at once (one per channel at a time).
pub const MAX_IN_FLIGHT: usize = 16;

/// How long a [`shutdown`](GpuService::shutdown) lets the jobs in flight and
/// the held results finish before the GPU thread ends without them.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// How much longer than [`SHUTDOWN_TIMEOUT`] the control side waits for the
/// GPU thread to end before it stops waiting and reports (never joining a
/// thread that has not ended).
pub const JOIN_MARGIN: Duration = Duration::from_millis(250);

/// A compute kernel: WGSL with one entry point that reads
/// `@group(0) @binding(0) var<storage, read> input: array<f32>` and writes
/// `@group(0) @binding(1) var<storage, read_write> output: array<f32>`, and
/// uses no other resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelSpec {
    /// The shader source; with `entry`, the key its pipeline is cached by
    /// (by content).
    pub wgsl: &'static str,
    /// The entry point.
    pub entry: &'static str,
    /// The `f32` words of a job's input.
    pub input_words: usize,
    /// The `f32` words of a job's output.
    pub output_words: usize,
    /// The workgroups dispatched per job (x, y, z): the channel's own.
    pub workgroups: (u32, u32, u32),
}

/// Why the service or a channel could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuError {
    /// No Metal adapter, or the device could not be created.
    NoDevice(String),
    /// The kernel did not compile, its bindings are not the channel's, or
    /// its sizes are 0.
    Kernel(String),
    /// The channel cannot run on this device: its workgroups or buffers
    /// exceed the device's limits, or its buffers are smaller than the
    /// shader needs.
    Invalid(String),
    /// The service is draining, stopped or lost.
    Stopped,
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::NoDevice(why) => write!(f, "no usable GPU device: {why}"),
            GpuError::Kernel(why) => write!(f, "GPU kernel: {why}"),
            GpuError::Invalid(why) => write!(f, "GPU channel cannot run: {why}"),
            GpuError::Stopped => write!(f, "the GPU service is not running"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Why a job did not produce a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A validation error around its submission.
    Validation,
    /// The device ran out of memory.
    OutOfMemory,
    /// An internal error of the device or driver.
    Internal,
    /// Its result could not be mapped for reading.
    Mapping,
    /// An error no scope captured was raised while it was in flight.
    Uncaptured,
    /// It was not run: the service was shutting down.
    Stopped,
}

/// How a job ended. Only `Ok` carries a result; otherwise its output is 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It ran and its result was read.
    Ok,
    /// It did not run correctly.
    Failed(Failure),
    /// The device was lost (reported once the device-lost callback, or the
    /// test hook, has run; a job just before may end `Failed`).
    DeviceLost,
}

/// A channel's identity, carried by each of its jobs. Unique in the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelId(u64);

static NEXT_CHANNEL: AtomicU64 = AtomicU64::new(1);

/// One job's buffers, moved (never copied, never freed) between a module and
/// the GPU thread.
#[derive(Debug)]
pub struct Job {
    owner: ChannelId,
    seq: u64,
    outcome: Outcome,
    input: Box<[f32]>,
    output: Box<[f32]>,
}

impl Job {
    /// The sequence number it was submitted with.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The channel it belongs to.
    pub fn owner(&self) -> ChannelId {
        self.owner
    }

    /// How it ended, once done.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Whether it ended other than [`Outcome::Ok`] (its output is then 0).
    pub fn failed(&self) -> bool {
        self.outcome != Outcome::Ok
    }

    /// Its output, once done.
    pub fn output(&self) -> &[f32] {
        &self.output
    }
}

/// What a channel and its lane on the GPU thread share besides the queues.
#[derive(Debug, Default)]
struct Probe {
    /// Test hook: each job stays in flight at least this many µs.
    delay_us: AtomicU64,
    /// The workgroups last dispatched for the channel (x, y, z).
    dispatched: [AtomicU32; 3],
}

/// A module's end of the service: a fixed pool of jobs. Real-time safe:
/// nothing here blocks, locks or allocates, and no job is ever dropped
/// here (they are dropped with the channel, on the control thread).
#[derive(Debug)]
pub struct Channel {
    id: ChannelId,
    submit: rtrb::Producer<Job>,
    done: rtrb::Consumer<Job>,
    /// The free jobs; its capacity is every job, so a push never allocates.
    free: Vec<Job>,
    /// Read only by the test hooks.
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]
    probe: Arc<Probe>,
    /// Its service's state, for [`service_lost`](Channel::service_lost).
    service: Arc<Shared>,
}

impl Channel {
    /// Its identity, which its jobs carry.
    pub fn id(&self) -> ChannelId {
        self.id
    }

    /// Submits a job with `seq` whose input `fill` writes. Returns `false`
    /// (and submits nothing) when every job is in flight or held.
    pub fn submit(&mut self, seq: u64, fill: impl FnOnce(&mut [f32])) -> bool {
        let Some(mut job) = self.free.pop() else {
            return false;
        };
        job.seq = seq;
        job.outcome = Outcome::Ok;
        fill(&mut job.input);
        match self.submit.push(job) {
            Ok(()) => true,
            Err(rtrb::PushError::Full(job)) => {
                self.free.push(job);
                false
            }
        }
    }

    /// Jobs in the pool, ready for [`submit`](Channel::submit). Neither
    /// blocks, locks nor allocates.
    pub fn free_jobs(&self) -> usize {
        self.free.len()
    }

    /// The sequence number of the oldest finished job not yet taken.
    pub fn peek_done(&self) -> Option<u64> {
        self.done.peek().ok().map(|job| job.seq)
    }

    /// Takes the oldest finished job; give it back with [`recycle`](Channel::recycle).
    pub fn take_done(&mut self) -> Option<Job> {
        self.done.pop().ok()
    }

    /// Returns a job taken with [`take_done`](Channel::take_done) to the pool.
    /// Another channel's job is refused and handed back (as is one the pool
    /// has no room for): it is never dropped here, so this neither allocates
    /// nor frees; drop a refused job outside real time.
    #[must_use = "a refused job must be kept, to be dropped outside real time"]
    pub fn recycle(&mut self, job: Job) -> Result<(), Job> {
        if job.owner != self.id || self.free.len() >= self.free.capacity() {
            return Err(job);
        }
        self.free.push(job);
        Ok(())
    }

    /// Whether the GPU thread is gone: nothing in flight will come back.
    pub fn is_lost(&self) -> bool {
        self.done.is_abandoned() && self.done.is_empty()
    }

    /// Whether its service's device was lost (M2.7-2: why its jobs no longer
    /// come back). An atomic load: real-time safe.
    pub fn service_lost(&self) -> bool {
        self.service.is_lost()
    }

    /// A channel of `jobs` jobs of `spec` on no device (M2.7-2): made for an
    /// instance prepared while its device is lost, so that the pool can
    /// offer it a replacement. It is lost at once; what it is given never
    /// comes back. Control side: it allocates.
    pub(crate) fn detached(spec: KernelSpec, jobs: usize) -> Channel {
        let id = ChannelId(NEXT_CHANNEL.fetch_add(1, Ordering::Relaxed));
        let (submit, _gone) = rtrb::RingBuffer::new(jobs.max(1));
        let (_never, done) = rtrb::RingBuffer::new(jobs.max(1));
        let free = (0..jobs)
            .map(|_| Job {
                owner: id,
                seq: 0,
                outcome: Outcome::Ok,
                input: vec![0.0; spec.input_words].into_boxed_slice(),
                output: vec![0.0; spec.output_words].into_boxed_slice(),
            })
            .collect();
        let service = Shared::default();
        service.state.store(LOST, Ordering::Release);
        Channel {
            id,
            submit,
            done,
            free,
            probe: Arc::new(Probe::default()),
            service: Arc::new(service),
        }
    }

    /// Test hook (M2.7-5): every job of this channel stays in flight at
    /// least `delay`, as on a slow GPU, holding its place in the device's
    /// bound; other channels are not held back. Tests only (feature
    /// `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_execution_delay(&self, delay: Duration) {
        let us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
        self.probe.delay_us.store(us, Ordering::Relaxed);
    }

    /// Test hook (M2.7-5): the workgroups the GPU thread last dispatched for
    /// this channel. Tests only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn last_dispatch(&self) -> Option<(u32, u32, u32)> {
        let [x, y, z] = &self.probe.dispatched;
        let d = (
            x.load(Ordering::Acquire),
            y.load(Ordering::Acquire),
            z.load(Ordering::Acquire),
        );
        (d != (0, 0, 0)).then_some(d)
    }
}

/// The state of a [`GpuService`]: `Running → Draining → Stopped`, and
/// `Lost` from any state, for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    /// Jobs are accepted and run.
    Running,
    /// Shutting down: no job is started; jobs in flight finish.
    Draining,
    /// The GPU thread has ended.
    Stopped,
    /// The device was lost: every job ends [`Outcome::DeviceLost`].
    Lost,
}

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const STOPPED: u8 = 2;
const LOST: u8 = 3;

/// Test hook (M2.7-5): a real wgpu error for the next job started.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Injected {
    /// Its dispatch exceeds the device's limit (a validation error).
    Dispatch,
    /// Its result is mapped from an unaligned offset (a mapping error).
    Map,
}

const INJECT_DISPATCH: u8 = 1;
const INJECT_MAP: u8 = 2;

/// Jobs that ended other than `Ok`, by reason, since the start, and the
/// last device error's message. Diagnostics for the control side: each
/// job carries its own [`Outcome`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FaultCounts {
    /// `Failed(Validation)`.
    pub validation: u64,
    /// `Failed(OutOfMemory)`.
    pub out_of_memory: u64,
    /// `Failed(Internal)`.
    pub internal: u64,
    /// `Failed(Mapping)`.
    pub mapping: u64,
    /// `Failed(Uncaptured)`.
    pub uncaptured: u64,
    /// `Failed(Stopped)`.
    pub stopped: u64,
    /// `DeviceLost`.
    pub device_lost: u64,
    /// Errors no scope captured (each also fails the jobs then in flight).
    pub uncaptured_errors: u64,
    /// The last device error or loss, as wgpu described it.
    pub last_error: Option<String>,
}

/// What a [`shutdown`](GpuService::shutdown) did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    /// The state afterwards: `Stopped`, or `Lost`.
    pub final_state: ServiceState,
    /// How long it took.
    pub elapsed: Duration,
    /// Jobs in flight or held when the GPU thread ended (they never came
    /// back; their channels report [`is_lost`](Channel::is_lost)).
    pub abandoned_jobs: u64,
    /// The GPU thread did not end within [`SHUTDOWN_TIMEOUT`] +
    /// [`JOIN_MARGIN`]: it was left running, not joined.
    pub timed_out: bool,
}

impl fmt::Display for ShutdownReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "shutdown {:?} in {:?}, {} job(s) abandoned{}",
            self.final_state,
            self.elapsed,
            self.abandoned_jobs,
            if self.timed_out {
                ", the GPU thread did not end in time and was not joined"
            } else {
                ""
            }
        )
    }
}

/// What the control side and the GPU thread share.
#[derive(Debug, Default)]
struct Shared {
    state: AtomicU8,
    /// A shutdown was asked for.
    stop: AtomicBool,
    /// Test hook: finished jobs are published this many µs late.
    delay_us: AtomicU64,
    /// Jobs started on the device since the start.
    jobs: AtomicU64,
    /// Errors no scope captured.
    uncaptured: AtomicU64,
    /// Test hook: an [`Injected`] error for the next job.
    inject: AtomicU8,
    /// The most jobs in flight at once.
    peak_in_flight: AtomicUsize,
    /// Jobs the GPU thread ended without.
    abandoned: AtomicU64,
    /// Counts of [`FaultCounts`], in its field order.
    faults: [AtomicU64; 7],
    last_error: Mutex<Option<String>>,
    /// The pool's watcher, woken when the device is lost (M2.7-2). Never
    /// used on a real-time thread: a loss is recorded by wgpu's device-lost
    /// callback or the test hook.
    wake: Option<Mutex<Sender<()>>>,
}

impl Shared {
    fn state(&self) -> ServiceState {
        match self.state.load(Ordering::Acquire) {
            RUNNING => ServiceState::Running,
            DRAINING => ServiceState::Draining,
            STOPPED => ServiceState::Stopped,
            _ => ServiceState::Lost,
        }
    }

    fn is_lost(&self) -> bool {
        self.state.load(Ordering::Acquire) == LOST
    }

    /// `from → to`, unless the state is another (in particular `Lost`).
    fn advance(&self, from: u8, to: u8) {
        let _ = self
            .state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire);
    }

    fn lose(&self, why: String) {
        self.state.store(LOST, Ordering::Release);
        self.note(why);
        if let Some(wake) = &self.wake {
            let _ = wake.lock().unwrap_or_else(PoisonError::into_inner).send(());
        }
    }

    /// Off the real-time threads only (it locks).
    fn note(&self, why: String) {
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(why);
    }

    fn count(&self, outcome: Outcome) {
        let at = match outcome {
            Outcome::Ok => return,
            Outcome::Failed(Failure::Validation) => 0,
            Outcome::Failed(Failure::OutOfMemory) => 1,
            Outcome::Failed(Failure::Internal) => 2,
            Outcome::Failed(Failure::Mapping) => 3,
            Outcome::Failed(Failure::Uncaptured) => 4,
            Outcome::Failed(Failure::Stopped) => 5,
            Outcome::DeviceLost => 6,
        };
        if let Some(n) = self.faults.get(at) {
            n.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A compiled kernel: the pipeline only, shared by every channel of the
/// same shader and entry point.
struct Kernel {
    pipeline: wgpu::ComputePipeline,
}

/// The cache key: the shader's text (compared by content) and entry point.
type KernelKey = (&'static str, &'static str);

/// The adapter a service runs on, as its backend describes it: what a
/// session records of a device it used (M2.7-8,
/// `docs/session-format.md` §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterIdentity {
    /// The backend (`Metal`; `Noop` for the test device).
    pub backend: String,
    /// The adapter's name.
    pub name: String,
    /// Its PCI vendor id (0 where the backend has none).
    pub vendor: u32,
    /// Its PCI device id (0 where the backend has none).
    pub device_id: u32,
    /// The driver's name.
    pub driver: String,
    /// The driver's version or other information.
    pub driver_info: String,
}

/// The device service: adapter, device and the GPU thread. Made by a
/// [`DevicePool`], which owns it.
pub struct GpuService {
    id: String,
    adapter: String,
    identity: AdapterIdentity,
    noop: bool,
    device: wgpu::Device,
    shared: Arc<Shared>,
    lanes: Sender<Lane>,
    /// The one bind group layout every kernel and channel use.
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    kernels: Mutex<HashMap<KernelKey, Arc<Kernel>>>,
    /// Disconnected when the GPU thread has ended.
    ended: Mutex<Receiver<()>>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for GpuService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuService")
            .field("id", &self.id)
            .field("adapter", &self.adapter)
            .finish_non_exhaustive()
    }
}

impl GpuService {
    /// Tests only (feature `noop`): a service on wgpu's noop device, which
    /// validates kernels and moves jobs like a GPU but computes nothing; on
    /// its own, outside any pool (the service's own tests).
    #[cfg(feature = "noop")]
    #[doc(hidden)]
    pub fn start_noop() -> Result<GpuService, GpuError> {
        GpuService::on_adapter(inventory::noop_adapter()?, "noop-0".to_owned(), None)
    }

    /// Whether this is the noop device (tests only), which computes nothing.
    pub fn is_noop(&self) -> bool {
        self.noop
    }

    /// Creates the device of `adapter` on the calling thread, which should be
    /// the control thread (or the pool's watcher), and starts its GPU thread,
    /// `signal-gpu-<id>`. A loss wakes `wake`.
    pub(crate) fn on_adapter(
        adapter: wgpu::Adapter,
        id: String,
        wake: Option<Sender<()>>,
    ) -> Result<GpuService, GpuError> {
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("signal-gpu"),
            ..wgpu::DeviceDescriptor::default()
        }))
        .map_err(|e| GpuError::NoDevice(e.to_string()))?;
        let shared = Arc::new(Shared {
            wake: wake.map(Mutex::new),
            ..Shared::default()
        });
        // An error no scope captured must not panic the GPU thread: it
        // fails the jobs in flight (their outcome is `Uncaptured`).
        let on_error = Arc::clone(&shared);
        device.on_uncaptured_error(Arc::new(move |error| {
            on_error.uncaptured.fetch_add(1, Ordering::AcqRel);
            on_error.note(error.to_string());
        }));
        let on_lost = Arc::clone(&shared);
        device.set_device_lost_callback(move |reason, message| {
            on_lost.lose(format!("device lost ({reason:?}): {message}"));
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                // Checked per channel, when it is made (`channel`), so
                // channels of any size share one pipeline.
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("signal-gpu"),
            entries: &[storage(0, true), storage(1, false)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("signal-gpu"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let (lanes, receiver) = mpsc::channel();
        let (ended_tx, ended) = mpsc::channel();
        let thread = {
            let (device, shared) = (device.clone(), Arc::clone(&shared));
            thread::Builder::new()
                .name(format!("signal-gpu-{id}"))
                .spawn(move || {
                    gpu_thread(&device, &queue, &shared, &receiver);
                    shared.advance(DRAINING, STOPPED);
                    shared.advance(RUNNING, STOPPED);
                    drop(ended_tx);
                })
                .map_err(|e| GpuError::NoDevice(e.to_string()))?
        };
        Ok(GpuService {
            id,
            adapter: format!("{} ({:?})", info.name, info.backend),
            identity: AdapterIdentity {
                backend: format!("{:?}", info.backend),
                name: info.name.clone(),
                vendor: info.vendor,
                device_id: info.device,
                driver: info.driver.clone(),
                driver_info: info.driver_info.clone(),
            },
            noop: info.backend == wgpu::Backend::Noop,
            device,
            shared,
            lanes,
            layout,
            pipeline_layout,
            kernels: Mutex::new(HashMap::new()),
            ended: Mutex::new(ended),
            thread: Some(thread),
        })
    }

    /// The adapter's name and backend.
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// The adapter as its backend describes it (M2.7-8).
    pub fn identity(&self) -> &AdapterIdentity {
        &self.identity
    }

    /// Its device's stable id (M2.7-2, [`inventory`]).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Jobs the GPU thread has started on the device since the start.
    pub fn jobs_run(&self) -> u64 {
        self.shared.jobs.load(Ordering::Relaxed)
    }

    /// The service's state.
    pub fn state(&self) -> ServiceState {
        self.shared.state()
    }

    /// Jobs that ended other than `Ok`, by reason, and the last device
    /// error. Control side only (it locks).
    pub fn faults(&self) -> FaultCounts {
        let n = |i: usize| {
            self.shared
                .faults
                .get(i)
                .map_or(0, |n| n.load(Ordering::Relaxed))
        };
        FaultCounts {
            validation: n(0),
            out_of_memory: n(1),
            internal: n(2),
            mapping: n(3),
            uncaptured: n(4),
            stopped: n(5),
            device_lost: n(6),
            uncaptured_errors: self.shared.uncaptured.load(Ordering::Acquire),
            last_error: self
                .shared
                .last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }

    /// Test hook (AC3 of M2.6-4): publish every finished job `delay` late,
    /// as a slow GPU would; zero restores normal completion. Process-wide.
    /// Tests only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_completion_delay(&self, delay: Duration) {
        let us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
        self.shared.delay_us.store(us, Ordering::Relaxed);
    }

    /// Test hook (M2.7-5): the next job started on this service gets a real
    /// wgpu error. Tests only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn fail_next_job(&self, injected: Injected) {
        let code = match injected {
            Injected::Dispatch => INJECT_DISPATCH,
            Injected::Map => INJECT_MAP,
        };
        self.shared.inject.store(code, Ordering::Release);
    }

    /// Test hook (M2.7-5): destroys the device, as a lost GPU. Tests only
    /// (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn lose_device(&self) {
        self.shared.lose("device lost (injected)".to_owned());
        self.device.destroy();
    }

    /// Test hook (M2.7-5): the most jobs that were in flight at once.
    /// Tests only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn peak_in_flight(&self) -> usize {
        self.shared.peak_in_flight.load(Ordering::Acquire)
    }

    /// Test hook (M2.7-5): the compiled pipelines in the cache. Tests only
    /// (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn kernels_cached(&self) -> usize {
        self.kernels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// A channel of `jobs` jobs for `spec` (its pipeline compiled once per
    /// shader and entry point). Everything that could stop it from running
    /// is checked here, before real time: the workgroups and buffer sizes
    /// against the device's limits, the shader's bindings against the
    /// channel's, and (by recording one dispatch, never submitted) the
    /// buffers against the sizes the shader needs. Control thread only: it
    /// allocates and may compile a shader.
    pub fn channel(&self, spec: KernelSpec, jobs: usize) -> Result<Channel, GpuError> {
        if spec.input_words == 0 || spec.output_words == 0 || jobs == 0 {
            return Err(GpuError::Kernel(
                "a channel needs input, output and jobs".into(),
            ));
        }
        if self.state() != ServiceState::Running {
            return Err(GpuError::Stopped);
        }
        let limits = self.device.limits();
        let (x, y, z) = spec.workgroups;
        let most = limits.max_compute_workgroups_per_dimension;
        if [x, y, z].iter().any(|&n| n == 0 || n > most) {
            return Err(GpuError::Invalid(format!(
                "workgroups {:?}: each must be 1 to {most}",
                spec.workgroups
            )));
        }
        let bytes = |words: usize| u64::try_from(words.saturating_mul(size_of::<f32>()));
        let most = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        for (name, words) in [("input", spec.input_words), ("output", spec.output_words)] {
            if bytes(words).map_or(true, |b| b > most) {
                return Err(GpuError::Invalid(format!(
                    "{name} of {words} words exceeds the device's {most} bytes"
                )));
            }
        }
        let size = |words| bytes(words).unwrap_or(u64::MAX);
        let kernel = self.kernel(spec)?;
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let buffer = |label, size, usage| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let input = buffer("input", size(spec.input_words), U::STORAGE | U::COPY_DST);
        let output = buffer("output", size(spec.output_words), U::STORAGE | U::COPY_SRC);
        let staging = buffer(
            "staging",
            size(spec.output_words),
            U::MAP_READ | U::COPY_DST,
        );
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("signal-gpu"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        // The trial: one dispatch recorded, never submitted. wgpu checks the
        // bound buffers against the shader's sizes when it records it.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&kernel.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(x, y, z);
        }
        drop(encoder.finish());
        let memory = pollster::block_on(memory.pop());
        let validation = pollster::block_on(validation.pop());
        if let Some(error) = validation.or(memory) {
            return Err(GpuError::Invalid(error.to_string()));
        }
        let id = ChannelId(NEXT_CHANNEL.fetch_add(1, Ordering::Relaxed));
        let (submit, consumer) = rtrb::RingBuffer::new(jobs);
        let (producer, done) = rtrb::RingBuffer::new(jobs);
        let mut free = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            free.push(Job {
                owner: id,
                seq: 0,
                outcome: Outcome::Ok,
                input: vec![0.0; spec.input_words].into_boxed_slice(),
                output: vec![0.0; spec.output_words].into_boxed_slice(),
            });
        }
        let probe = Arc::new(Probe::default());
        let lane = Lane {
            jobs: consumer,
            done: producer,
            held: VecDeque::with_capacity(jobs),
            kernel,
            workgroups: spec.workgroups,
            bind_group,
            input,
            output,
            staging,
            bytes: vec![0; spec.input_words * size_of::<f32>()],
            probe: Arc::clone(&probe),
            mapped: Arc::new(AtomicU8::new(MAP_NONE)),
            flight: None,
        };
        self.lanes.send(lane).map_err(|_| GpuError::Stopped)?;
        Ok(Channel {
            id,
            submit,
            done,
            free,
            probe,
            service: Arc::clone(&self.shared),
        })
    }

    fn kernel(&self, spec: KernelSpec) -> Result<Arc<Kernel>, GpuError> {
        let key = (spec.wgsl, spec.entry);
        let mut kernels = self.kernels.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(kernel) = kernels.get(&key) {
            return Ok(Arc::clone(kernel));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(spec.entry),
                source: wgpu::ShaderSource::Wgsl(spec.wgsl.into()),
            });
        // The explicit layout: a shader that uses any other group or binding,
        // or these with another access, is refused here.
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(spec.entry),
                layout: Some(&self.pipeline_layout),
                module: &module,
                entry_point: Some(spec.entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Kernel(error.to_string()));
        }
        let kernel = Arc::new(Kernel { pipeline });
        kernels.insert(key, Arc::clone(&kernel));
        Ok(kernel)
    }

    /// Shuts the service down: no new job starts, the jobs in flight and
    /// held results get up to [`SHUTDOWN_TIMEOUT`] to finish, and the GPU
    /// thread is joined only once it has ended; if it has not ended within
    /// [`SHUTDOWN_TIMEOUT`] + [`JOIN_MARGIN`], it is left (not joined) and
    /// the report says so. Never waits longer. Control thread.
    pub fn shutdown(&mut self) -> ShutdownReport {
        let start = Instant::now();
        self.shared.stop.store(true, Ordering::Release);
        self.shared.advance(RUNNING, DRAINING);
        let mut timed_out = false;
        if let Some(thread) = self.thread.take() {
            let ended = self.ended.get_mut().unwrap_or_else(PoisonError::into_inner);
            match ended.recv_timeout(SHUTDOWN_TIMEOUT + JOIN_MARGIN) {
                Err(RecvTimeoutError::Timeout) => timed_out = true,
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = thread.join();
                }
            }
        }
        ShutdownReport {
            final_state: self.state(),
            elapsed: start.elapsed(),
            abandoned_jobs: self.shared.abandoned.load(Ordering::Acquire),
            timed_out,
        }
    }
}

impl Drop for GpuService {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let report = self.shutdown();
            if report.timed_out || report.abandoned_jobs > 0 {
                eprintln!("signal-gpu: {report}");
            }
        }
    }
}

/// A lane's staging map: none asked, asked, and its callback's answer.
const MAP_NONE: u8 = 0;
const MAP_PENDING: u8 = 1;
const MAP_OK: u8 = 2;
const MAP_ERR: u8 = 3;

/// A job on the device.
struct Flight {
    job: Job,
    /// An error of its submission.
    failure: Option<Failure>,
    /// Its `map_async` was refused in its scope.
    map_refused: bool,
    /// The uncaptured-error count when it started.
    uncaptured: u64,
    /// Not complete before (the execution delay test hook).
    ready_at: Instant,
}

/// The GPU thread's end of a channel.
struct Lane {
    jobs: rtrb::Consumer<Job>,
    done: rtrb::Producer<Job>,
    /// Finished jobs and when they may be published (the test hook).
    held: VecDeque<(Instant, Job)>,
    kernel: Arc<Kernel>,
    workgroups: (u32, u32, u32),
    bind_group: wgpu::BindGroup,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    /// The input as bytes, for the upload.
    bytes: Vec<u8>,
    staging: wgpu::Buffer,
    probe: Arc<Probe>,
    /// The staging map's state (`MAP_*`), written by its callback.
    mapped: Arc<AtomicU8>,
    /// Its job on the device: at most one at a time.
    flight: Option<Flight>,
}

/// The GPU thread: starts at most one job per channel and at most
/// [`MAX_IN_FLIGHT`] in all, in turn, polls the device without blocking,
/// completes each job whose result is mapped, and publishes it (after the
/// test hook's delay). A slow job holds back only its own channel.
fn gpu_thread(device: &wgpu::Device, queue: &wgpu::Queue, shared: &Shared, new: &Receiver<Lane>) {
    let mut lanes: Vec<Lane> = Vec::new();
    let mut next = 0usize;
    let mut deadline = None;
    loop {
        let stopping = shared.stop.load(Ordering::Acquire);
        if stopping && deadline.is_none() {
            deadline = Some(Instant::now() + SHUTDOWN_TIMEOUT);
        }
        let mut gone = false;
        loop {
            match new.try_recv() {
                Ok(lane) => lanes.push(lane),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        let now = Instant::now();
        let delay = Duration::from_micros(shared.delay_us.load(Ordering::Relaxed));
        let mut busy = false;
        if !shared.is_lost() {
            let _ = device.poll(wgpu::PollType::Poll);
        }
        // Complete what is done; jobs never started while stopping come
        // back as such.
        for lane in &mut lanes {
            if lane
                .flight
                .as_ref()
                .is_some_and(|f| ready(shared, lane, f, now))
                && let Some(flight) = lane.flight.take()
            {
                busy = true;
                let job = complete(device, shared, lane, flight);
                lane.held.push_back((now + delay, job));
            }
            if stopping {
                while let Ok(mut job) = lane.jobs.pop() {
                    job.outcome = Outcome::Failed(Failure::Stopped);
                    job.output.fill(0.0);
                    lane.held.push_back((now, job));
                }
            }
        }
        // Start one job on each idle channel, in turn, within the bound.
        let mut in_flight = lanes.iter().filter(|l| l.flight.is_some()).count();
        if !stopping && !lanes.is_empty() {
            let count = lanes.len();
            for k in 0..count {
                if in_flight >= MAX_IN_FLIGHT {
                    break;
                }
                let Some(lane) = lanes.get_mut((next + k) % count) else {
                    continue;
                };
                if lane.flight.is_some() || lane.jobs.is_abandoned() {
                    continue;
                }
                if let Ok(job) = lane.jobs.pop() {
                    busy = true;
                    let flight = start(device, queue, shared, lane, job, now);
                    lane.flight = Some(flight);
                    in_flight += 1;
                }
            }
            next = (next + 1) % count;
            shared.peak_in_flight.fetch_max(in_flight, Ordering::AcqRel);
        }
        // Publish what is due; a lost device's jobs are all lost.
        let lost = shared.is_lost();
        for lane in &mut lanes {
            while lane.held.front().is_some_and(|(at, _)| *at <= now) {
                let Some((at, mut job)) = lane.held.pop_front() else {
                    break;
                };
                // Loss takes precedence over every other outcome, shutdown's too.
                if lost {
                    job.outcome = Outcome::DeviceLost;
                    job.output.fill(0.0);
                }
                let outcome = job.outcome;
                // The done queue holds every job of the channel: never full.
                if let Err(rtrb::PushError::Full(job)) = lane.done.push(job) {
                    lane.held.push_front((at, job));
                    break;
                }
                shared.count(outcome);
            }
        }
        // A dropped channel: its jobs are dropped here, not on the audio
        // thread, once none is on the device.
        lanes.retain(|lane| !lane.jobs.is_abandoned() || lane.flight.is_some());
        let pending = lanes
            .iter()
            .any(|l| l.flight.is_some() || !l.held.is_empty());
        if stopping && (!pending || deadline.is_some_and(|d| now >= d)) {
            let left = lanes
                .iter()
                .map(|l| l.held.len() as u64 + u64::from(l.flight.is_some()))
                .sum();
            shared.abandoned.store(left, Ordering::Release);
            return;
        }
        // The service is gone without a shutdown; finish what is pending.
        if gone && lanes.is_empty() {
            return;
        }
        if lanes.is_empty() {
            thread::sleep(UNUSED_SLEEP);
        } else if !busy {
            thread::sleep(IDLE_SLEEP);
        }
    }
}

/// Whether `flight` can be completed: its map's callback has answered (the
/// staging buffer is never unmapped while its map is pending) and its test
/// delay has passed; on a lost device at once (no device call is made).
fn ready(shared: &Shared, lane: &Lane, flight: &Flight, now: Instant) -> bool {
    if shared.is_lost() {
        return true;
    }
    let answered = matches!(lane.mapped.load(Ordering::Acquire), MAP_OK | MAP_ERR);
    answered && now >= flight.ready_at
}

/// Starts `job` on the device: upload, dispatch, copy and map, each inside
/// error scopes, so its errors are its own.
fn start(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shared: &Shared,
    lane: &mut Lane,
    job: Job,
    now: Instant,
) -> Flight {
    let ready_at = now + Duration::from_micros(lane.probe.delay_us.load(Ordering::Relaxed));
    let uncaptured = shared.uncaptured.load(Ordering::Acquire);
    let mut flight = Flight {
        job,
        failure: None,
        map_refused: false,
        uncaptured,
        ready_at,
    };
    if shared.is_lost() {
        return flight;
    }
    shared.jobs.fetch_add(1, Ordering::Relaxed);
    let inject = shared.inject.swap(0, Ordering::AcqRel);
    let (x, y, z) = if inject == INJECT_DISPATCH {
        (
            device.limits().max_compute_workgroups_per_dimension + 1,
            1,
            1,
        )
    } else {
        lane.workgroups
    };
    for (bytes, x) in lane
        .bytes
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(flight.job.input.iter())
    {
        bytes.copy_from_slice(&x.to_ne_bytes());
    }
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    queue.write_buffer(&lane.input, 0, &lane.bytes);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&lane.kernel.pipeline);
        pass.set_bind_group(0, &lane.bind_group, &[]);
        pass.dispatch_workgroups(x, y, z);
    }
    encoder.copy_buffer_to_buffer(&lane.output, 0, &lane.staging, 0, None);
    queue.submit([encoder.finish()]);
    let [dx, dy, dz] = &lane.probe.dispatched;
    dx.store(x, Ordering::Release);
    dy.store(y, Ordering::Release);
    dz.store(z, Ordering::Release);
    let internal = pollster::block_on(internal.pop());
    let memory = pollster::block_on(memory.pop());
    let validation = pollster::block_on(validation.pop());
    flight.failure = if let Some(e) = validation {
        shared.note(e.to_string());
        Some(Failure::Validation)
    } else if let Some(e) = memory {
        shared.note(e.to_string());
        Some(Failure::OutOfMemory)
    } else if let Some(e) = internal {
        shared.note(e.to_string());
        Some(Failure::Internal)
    } else {
        None
    };
    // The map: its result is the callback's, never the buffer's contents.
    lane.mapped.store(MAP_PENDING, Ordering::Release);
    let state = Arc::clone(&lane.mapped);
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let slice = if inject == INJECT_MAP {
        lane.staging.slice(4..)
    } else {
        lane.staging.slice(..)
    };
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let answer = if result.is_ok() { MAP_OK } else { MAP_ERR };
        state.store(answer, Ordering::Release);
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        shared.note(e.to_string());
        flight.map_refused = true;
    }
    flight
}

/// Ends `flight` with its outcome: lost, a submission error, a mapping
/// error, an uncaptured error while it was in flight, or else `Ok` with its
/// result read. The staging buffer is unmapped after every map, answered
/// `Ok` or not, in scopes of its own, so the next map can succeed and no
/// error of this job reaches the next.
fn complete(device: &wgpu::Device, shared: &Shared, lane: &mut Lane, flight: Flight) -> Job {
    let Flight {
        mut job,
        failure,
        map_refused,
        uncaptured,
        ..
    } = flight;
    if shared.is_lost() {
        job.outcome = Outcome::DeviceLost;
        job.output.fill(0.0);
        return job;
    }
    let mapped = lane.mapped.swap(MAP_NONE, Ordering::AcqRel);
    job.outcome = if let Some(failure) = failure {
        Outcome::Failed(failure)
    } else if map_refused || mapped != MAP_OK {
        Outcome::Failed(Failure::Mapping)
    } else if shared.uncaptured.load(Ordering::Acquire) != uncaptured {
        Outcome::Failed(Failure::Uncaptured)
    } else {
        Outcome::Ok
    };
    if job.outcome == Outcome::Ok {
        // The view ends with this block, before the unmap.
        match lane.staging.slice(..).get_mapped_range() {
            Ok(view) => {
                for (x, bytes) in job.output.iter_mut().zip(view.as_chunks::<4>().0) {
                    *x = f32::from_ne_bytes(*bytes);
                }
            }
            Err(_) => job.outcome = Outcome::Failed(Failure::Mapping),
        }
    }
    if mapped != MAP_NONE {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        lane.staging.unmap();
        let internal = pollster::block_on(internal.pop());
        let memory = pollster::block_on(memory.pop());
        let validation = pollster::block_on(validation.pop());
        // After a refused map the unmap's error is expected and dropped.
        if mapped == MAP_OK && (validation.is_some() || memory.is_some() || internal.is_some()) {
            job.outcome = Outcome::Failed(Failure::Mapping);
        }
    }
    if job.outcome != Outcome::Ok {
        job.output.fill(0.0);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `output[i] = 2 · input[i] + 1`, for 64 words.
    const DOUBLE: KernelSpec = KernelSpec {
        wgsl: "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    output[id.x] = 2.0 * input[id.x] + 1.0;
}",
        entry: "main",
        input_words: 64,
        output_words: 64,
        workgroups: (1, 1, 1),
    };

    /// A private noop service (the GPU's own values are checked by the
    /// adapter tests in `tests/service.rs`, on a GPU).
    fn service() -> GpuService {
        GpuService::start_noop().unwrap()
    }

    fn wait_done(channel: &Channel) -> u64 {
        let start = Instant::now();
        loop {
            if let Some(seq) = channel.peek_done() {
                return seq;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no result");
            thread::yield_now();
        }
    }

    /// Jobs round-trip through the GPU thread in submission order (with the
    /// kernel's result on a GPU; the noop device computes nothing); a full
    /// pool refuses without allocating; a taken job goes back to the pool.
    #[test]
    fn jobs_run_in_order_on_the_gpu_thread() {
        let service = service();
        let mut channel = service.channel(DOUBLE, 2).unwrap();
        assert!(channel.submit(7, |input| {
            for (i, x) in input.iter_mut().enumerate() {
                *x = i as f32;
            }
        }));
        assert!(channel.submit(8, |input| input.fill(-1.0)));
        assert!(!channel.submit(9, |_| {}), "both jobs are in flight");
        for (seq, expect) in [(7, None), (8, Some(-1.0))] {
            assert_eq!(wait_done(&channel), seq);
            let job = channel.take_done().unwrap();
            assert_eq!(job.seq(), seq);
            assert!(!job.failed());
            if !service.is_noop() {
                for (i, &y) in job.output().iter().enumerate() {
                    let x = expect.unwrap_or(i as f32);
                    assert_eq!(y, 2.0 * x + 1.0);
                }
            }
            channel.recycle(job).unwrap();
        }
        assert!(channel.submit(9, |input| input.fill(0.0)));
        assert_eq!(wait_done(&channel), 9);
        assert!(service.jobs_run() >= 3);
        assert!(!channel.is_lost());
    }

    /// A kernel that does not compile is refused on the control side.
    #[test]
    fn a_bad_kernel_is_refused() {
        let service = service();
        let bad = KernelSpec {
            wgsl: "this is not wgsl",
            ..DOUBLE
        };
        assert!(matches!(service.channel(bad, 1), Err(GpuError::Kernel(_))));
        let empty = KernelSpec {
            input_words: 0,
            ..DOUBLE
        };
        assert!(matches!(
            service.channel(empty, 1),
            Err(GpuError::Kernel(_))
        ));
    }

    /// The test hook holds finished jobs back; a stopped service loses the
    /// jobs in flight, and its channels say so instead of waiting forever.
    #[test]
    fn the_completion_delay_holds_results_and_a_stopped_service_is_lost() {
        let service = service();
        let mut channel = service.channel(DOUBLE, 2).unwrap();
        service.set_completion_delay(Duration::from_millis(300));
        let start = Instant::now();
        assert!(channel.submit(1, |_| {}));
        assert_eq!(wait_done(&channel), 1);
        assert!(start.elapsed() >= Duration::from_millis(300));
        service.set_completion_delay(Duration::from_secs(60));
        let job = channel.take_done().unwrap();
        channel.recycle(job).unwrap();
        assert!(channel.submit(2, |_| {}));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(channel.peek_done(), None, "held back");
        assert!(!channel.is_lost());
        drop(service);
        assert!(channel.is_lost(), "the GPU thread ended with job 2 held");
    }

    /// An error no scope captured, raised while a job is in flight, is that
    /// job's: it ends `Failed(Uncaptured)`, not `Ok`.
    #[test]
    fn an_uncaptured_error_fails_the_job_in_flight() {
        let service = GpuService::start_noop().unwrap();
        let mut channel = service.channel(DOUBLE, 1).unwrap();
        channel.set_execution_delay(Duration::from_millis(200));
        assert!(channel.submit(1, |_| {}));
        thread::sleep(Duration::from_millis(50));
        // As the device's handler does, for an error raised outside a scope.
        service.shared.uncaptured.fetch_add(1, Ordering::AcqRel);
        wait_done(&channel);
        let job = channel.take_done().unwrap();
        assert_eq!(job.outcome(), Outcome::Failed(Failure::Uncaptured));
        channel.recycle(job).unwrap();
        channel.set_execution_delay(Duration::ZERO);
        assert!(channel.submit(2, |_| {}));
        wait_done(&channel);
        assert_eq!(channel.take_done().unwrap().outcome(), Outcome::Ok);
    }
}
