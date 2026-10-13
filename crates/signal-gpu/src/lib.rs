//! Signal's GPU device service (ADR 0019 §3).
//!
//! A [`GpuService`] owns one wgpu adapter and device (Metal backend), created
//! on the control side, and one dedicated thread, `signal-gpu`, which is the
//! only thread that submits GPU work or waits for it. A GPU-backed module
//! talks to it through a [`Channel`]: a fixed set of [`Job`] buffers moved
//! between the module and the GPU thread through two lock-free `rtrb` SPSC
//! queues (submit, done), whose atomic indices are the completion signal.
//! Submitting, polling and recycling never block, lock or allocate, so they
//! may run on the audio thread or the real-time workers; the GPU work itself,
//! and every wait for it, stays on the GPU thread (and, offline, on whoever
//! the engine waits on: never in real time).
//!
//! A machine without a usable adapter has no service ([`GpuService::shared`]
//! is `None`): GPU-backed modules then have no channel and the engine reports
//! their due results as lost. The GPU path is always optional; the CPU stays
//! the deterministic default.

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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the GPU thread sleeps when it found no job (it never parks, so a
/// submitting thread never has to wake it: no system call on the audio side).
pub const IDLE_SLEEP: Duration = Duration::from_micros(50);

/// How long it sleeps while no module has a channel (a new channel waits at
/// most this long to be served, well within a block).
pub const UNUSED_SLEEP: Duration = Duration::from_millis(1);

/// A compute kernel: WGSL with one entry point that reads
/// `@group(0) @binding(0) var<storage, read> input: array<f32>` and writes
/// `@group(0) @binding(1) var<storage, read_write> output: array<f32>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelSpec {
    /// The shader source; with `entry`, the key its pipeline is cached by.
    pub wgsl: &'static str,
    /// The entry point.
    pub entry: &'static str,
    /// The `f32` words of a job's input.
    pub input_words: usize,
    /// The `f32` words of a job's output.
    pub output_words: usize,
    /// The workgroups dispatched per job (x, y, z).
    pub workgroups: (u32, u32, u32),
}

/// Why the service or a channel could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuError {
    /// No Metal adapter, or the device could not be created.
    NoDevice(String),
    /// The kernel did not compile, or its sizes are 0.
    Kernel(String),
    /// The GPU thread has ended.
    Stopped,
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::NoDevice(why) => write!(f, "no usable GPU device: {why}"),
            GpuError::Kernel(why) => write!(f, "GPU kernel: {why}"),
            GpuError::Stopped => write!(f, "the GPU thread has ended"),
        }
    }
}

impl std::error::Error for GpuError {}

/// One job's buffers, moved (never copied, never freed) between a module and
/// the GPU thread.
#[derive(Debug)]
pub struct Job {
    seq: u64,
    failed: bool,
    input: Box<[f32]>,
    output: Box<[f32]>,
}

impl Job {
    /// The sequence number it was submitted with.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Whether the GPU could not run it (its output is then 0).
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Its output, once done.
    pub fn output(&self) -> &[f32] {
        &self.output
    }
}

/// A module's end of the service: a fixed pool of jobs. Real-time safe:
/// nothing here blocks, locks or allocates, and no job is ever dropped
/// here (they are dropped with the channel, on the control thread).
#[derive(Debug)]
pub struct Channel {
    submit: rtrb::Producer<Job>,
    done: rtrb::Consumer<Job>,
    /// The free jobs; its capacity is every job, so a push never allocates.
    free: Vec<Job>,
}

impl Channel {
    /// Submits a job with `seq` whose input `fill` writes. Returns `false`
    /// (and submits nothing) when every job is in flight or held.
    pub fn submit(&mut self, seq: u64, fill: impl FnOnce(&mut [f32])) -> bool {
        let Some(mut job) = self.free.pop() else {
            return false;
        };
        job.seq = seq;
        job.failed = false;
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
    pub fn recycle(&mut self, job: Job) {
        // Every job belongs to this pool, so this is within its capacity.
        if self.free.len() < self.free.capacity() {
            self.free.push(job);
        }
    }

    /// Whether the GPU thread is gone: nothing in flight will come back.
    pub fn is_lost(&self) -> bool {
        self.done.is_abandoned() && self.done.is_empty()
    }
}

/// The GPU thread's end of a channel.
struct Lane {
    jobs: rtrb::Consumer<Job>,
    done: rtrb::Producer<Job>,
    /// Finished jobs and when they may be published (the test hook).
    held: VecDeque<(Instant, Job)>,
    kernel: Arc<Kernel>,
    bind_group: wgpu::BindGroup,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    /// The input as bytes, for the upload.
    bytes: Vec<u8>,
    staging: wgpu::Buffer,
}

/// A compiled kernel, shared by every channel of its spec.
struct Kernel {
    spec: KernelSpec,
    pipeline: wgpu::ComputePipeline,
}

/// What the control side and the GPU thread share.
#[derive(Debug, Default)]
struct Shared {
    stop: AtomicBool,
    /// Test hook: finished jobs are published this many µs late.
    delay_us: AtomicU64,
    /// Jobs run since the start.
    jobs: AtomicU64,
}

/// The device service: adapter, device and the GPU thread.
pub struct GpuService {
    adapter: String,
    noop: bool,
    device: wgpu::Device,
    shared: Arc<Shared>,
    lanes: Sender<Lane>,
    kernels: Mutex<HashMap<(usize, &'static str), Arc<Kernel>>>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for GpuService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuService")
            .field("adapter", &self.adapter)
            .finish_non_exhaustive()
    }
}

impl GpuService {
    /// Creates the adapter and device (Metal backend) on the calling thread,
    /// which should be the control thread, and starts the GPU thread.
    pub fn start() -> Result<GpuService, GpuError> {
        GpuService::start_on(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        })
    }

    /// Tests only (feature `noop`): a service on wgpu's noop device, which
    /// validates kernels and moves jobs like a GPU but computes nothing.
    #[cfg(feature = "noop")]
    #[doc(hidden)]
    pub fn start_noop() -> Result<GpuService, GpuError> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::NOOP;
        descriptor.backend_options.noop.enable = true;
        GpuService::start_on(descriptor)
    }

    /// Tests only (feature `noop`): [`shared`](GpuService::shared), or the
    /// noop service as the process's service where no GPU is usable.
    #[cfg(feature = "noop")]
    #[doc(hidden)]
    pub fn shared_or_noop() -> Option<&'static GpuService> {
        SHARED
            .get_or_init(|| GpuService::start().or_else(|_| GpuService::start_noop()))
            .as_ref()
            .ok()
    }

    /// Whether this is the noop device (tests only), which computes nothing.
    pub fn is_noop(&self) -> bool {
        self.noop
    }

    fn start_on(descriptor: wgpu::InstanceDescriptor) -> Result<GpuService, GpuError> {
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..wgpu::RequestAdapterOptions::default()
        }))
        .map_err(|e| GpuError::NoDevice(e.to_string()))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("signal-gpu"),
            ..wgpu::DeviceDescriptor::default()
        }))
        .map_err(|e| GpuError::NoDevice(e.to_string()))?;
        // A validation error must not panic the GPU thread: the job it
        // belongs to fails (its map does not succeed) instead.
        device.on_uncaptured_error(Arc::new(|error| eprintln!("signal-gpu: {error}")));
        let shared = Arc::new(Shared::default());
        let (lanes, receiver) = mpsc::channel();
        let thread = {
            let (device, shared) = (device.clone(), Arc::clone(&shared));
            thread::Builder::new()
                .name("signal-gpu".to_owned())
                .spawn(move || gpu_thread(&device, &queue, &shared, &receiver))
                .map_err(|e| GpuError::NoDevice(e.to_string()))?
        };
        Ok(GpuService {
            adapter: format!("{} ({:?})", info.name, info.backend),
            noop: info.backend == wgpu::Backend::Noop,
            device,
            shared,
            lanes,
            kernels: Mutex::new(HashMap::new()),
            thread: Some(thread),
        })
    }

    /// The process's service, started on first use (from a module's
    /// `prepare`, on the control thread); `None` when this machine has no
    /// usable GPU. Why not: [`shared_error`](GpuService::shared_error).
    pub fn shared() -> Option<&'static GpuService> {
        SHARED.get_or_init(GpuService::start).as_ref().ok()
    }

    /// Why [`shared`](GpuService::shared) is `None`, if it is.
    pub fn shared_error() -> Option<&'static GpuError> {
        SHARED.get_or_init(GpuService::start).as_ref().err()
    }

    /// The adapter's name and backend.
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Jobs the GPU thread has run since the start.
    pub fn jobs_run(&self) -> u64 {
        self.shared.jobs.load(Ordering::Relaxed)
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

    /// A channel of `jobs` jobs for `spec`'s kernel (compiled once per spec).
    /// Control thread only: it allocates and may compile a shader.
    pub fn channel(&self, spec: KernelSpec, jobs: usize) -> Result<Channel, GpuError> {
        if spec.input_words == 0 || spec.output_words == 0 || jobs == 0 {
            return Err(GpuError::Kernel(
                "a channel needs input, output and jobs".into(),
            ));
        }
        let kernel = self.kernel(spec)?;
        let bytes = |words: usize| (words * size_of::<f32>()) as u64;
        let buffer = |label, size, usage| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let input = buffer("input", bytes(spec.input_words), U::STORAGE | U::COPY_DST);
        let output = buffer("output", bytes(spec.output_words), U::STORAGE | U::COPY_SRC);
        let staging = buffer(
            "staging",
            bytes(spec.output_words),
            U::MAP_READ | U::COPY_DST,
        );
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("signal-gpu"),
            layout: &kernel.pipeline.get_bind_group_layout(0),
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
        let (submit, consumer) = rtrb::RingBuffer::new(jobs);
        let (producer, done) = rtrb::RingBuffer::new(jobs);
        let mut free = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            free.push(Job {
                seq: 0,
                failed: false,
                input: vec![0.0; spec.input_words].into_boxed_slice(),
                output: vec![0.0; spec.output_words].into_boxed_slice(),
            });
        }
        let lane = Lane {
            jobs: consumer,
            done: producer,
            held: VecDeque::with_capacity(jobs),
            kernel,
            bind_group,
            input,
            output,
            staging,
            bytes: vec![0; spec.input_words * size_of::<f32>()],
        };
        self.lanes.send(lane).map_err(|_| GpuError::Stopped)?;
        Ok(Channel { submit, done, free })
    }

    fn kernel(&self, spec: KernelSpec) -> Result<Arc<Kernel>, GpuError> {
        let key = (spec.wgsl.as_ptr() as usize, spec.entry);
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
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(spec.entry),
                layout: None,
                module: &module,
                entry_point: Some(spec.entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Kernel(error.to_string()));
        }
        let kernel = Arc::new(Kernel { spec, pipeline });
        kernels.insert(key, Arc::clone(&kernel));
        Ok(kernel)
    }
}

impl Drop for GpuService {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

static SHARED: OnceLock<Result<GpuService, GpuError>> = OnceLock::new();

/// The GPU thread: takes jobs from every channel in turn, runs each to
/// completion, and publishes it (after the test hook's delay).
fn gpu_thread(device: &wgpu::Device, queue: &wgpu::Queue, shared: &Shared, new: &Receiver<Lane>) {
    let mut lanes: Vec<Lane> = Vec::new();
    while !shared.stop.load(Ordering::Relaxed) {
        loop {
            match new.try_recv() {
                Ok(lane) => lanes.push(lane),
                Err(TryRecvError::Empty) => break,
                // The service is gone; finish what the channels still hold.
                Err(TryRecvError::Disconnected) if lanes.is_empty() => return,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        let delay = Duration::from_micros(shared.delay_us.load(Ordering::Relaxed));
        let mut busy = false;
        for lane in &mut lanes {
            while let Ok(mut job) = lane.jobs.pop() {
                busy = true;
                run(device, queue, lane, &mut job);
                shared.jobs.fetch_add(1, Ordering::Relaxed);
                lane.held.push_back((Instant::now() + delay, job));
            }
            let now = Instant::now();
            while lane.held.front().is_some_and(|(at, _)| *at <= now) {
                let Some((at, job)) = lane.held.pop_front() else {
                    break;
                };
                // The done queue holds every job of the channel: never full.
                if let Err(rtrb::PushError::Full(job)) = lane.done.push(job) {
                    lane.held.push_front((at, job));
                    break;
                }
            }
        }
        // A dropped channel: its jobs are dropped here, not on the audio thread.
        lanes.retain(|lane| !lane.jobs.is_abandoned());
        if lanes.is_empty() {
            thread::sleep(UNUSED_SLEEP);
        } else if !busy {
            thread::sleep(IDLE_SLEEP);
        }
    }
}

/// Runs one job on the GPU and waits for it, on the GPU thread.
fn run(device: &wgpu::Device, queue: &wgpu::Queue, lane: &mut Lane, job: &mut Job) {
    let spec = lane.kernel.spec;
    for (bytes, x) in lane
        .bytes
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(job.input.iter())
    {
        bytes.copy_from_slice(&x.to_ne_bytes());
    }
    queue.write_buffer(&lane.input, 0, &lane.bytes);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&lane.kernel.pipeline);
        pass.set_bind_group(0, &lane.bind_group, &[]);
        let (x, y, z) = spec.workgroups;
        pass.dispatch_workgroups(x, y, z);
    }
    encoder.copy_buffer_to_buffer(&lane.output, 0, &lane.staging, 0, None);
    queue.submit([encoder.finish()]);
    let slice = lane.staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let waited = device.poll(wgpu::PollType::wait_indefinitely());
    let read = waited.is_ok()
        && match slice.get_mapped_range() {
            Ok(view) => {
                for (x, bytes) in job.output.iter_mut().zip(view.as_chunks::<4>().0) {
                    *x = f32::from_ne_bytes(*bytes);
                }
                true
            }
            Err(_) => false,
        };
    lane.staging.unmap();
    if !read {
        job.failed = true;
        job.output.fill(0.0);
    }
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

    /// The GPU, or (with the `noop` feature) the noop device where there is
    /// none; `None` after saying why.
    fn service() -> Option<&'static GpuService> {
        #[cfg(feature = "noop")]
        let service = GpuService::shared_or_noop();
        #[cfg(not(feature = "noop"))]
        let service = GpuService::shared();
        match service {
            Some(service) => eprintln!("device: {}", service.adapter()),
            None => eprintln!(
                "skipped: no usable GPU adapter ({:?})",
                GpuService::shared_error()
            ),
        }
        service
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
        let Some(service) = service() else {
            return;
        };
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
            channel.recycle(job);
        }
        assert!(channel.submit(9, |input| input.fill(0.0)));
        assert_eq!(wait_done(&channel), 9);
        assert!(service.jobs_run() >= 3);
        assert!(!channel.is_lost());
    }

    /// A kernel that does not compile is refused on the control side.
    #[test]
    fn a_bad_kernel_is_refused() {
        let Some(service) = service() else {
            return;
        };
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
    #[cfg(feature = "noop")]
    #[test]
    fn the_completion_delay_holds_results_and_a_stopped_service_is_lost() {
        let service = GpuService::start().or_else(|_| GpuService::start_noop());
        let service = service.unwrap();
        let mut channel = service.channel(DOUBLE, 2).unwrap();
        service.set_completion_delay(Duration::from_millis(300));
        let start = Instant::now();
        assert!(channel.submit(1, |_| {}));
        assert_eq!(wait_done(&channel), 1);
        assert!(start.elapsed() >= Duration::from_millis(300));
        service.set_completion_delay(Duration::from_secs(60));
        let job = channel.take_done().unwrap();
        channel.recycle(job);
        assert!(channel.submit(2, |_| {}));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(channel.peek_done(), None, "held back");
        assert!(!channel.is_lost());
        drop(service);
        assert!(channel.is_lost(), "the GPU thread ended with job 2 held");
    }
}
