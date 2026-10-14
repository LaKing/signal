//! M2.7-2: the device pool. It owns one [`GpuService`] per device (there is
//! no process-wide service), each with its own `signal-gpu-<id>` thread and
//! lock-free queues. The device list is fixed when the pool opens: a device
//! keeps its index for the pool's life, lost or not.
//!
//! **Generations.** A lost service is replaced, not left: a control-side
//! thread, `signal-gpu-pool`, parks until a service reports a loss, retires
//! the lost service, opens the same adapter again as the next generation and
//! offers every channel made on it a replacement, which the module takes
//! with [`PlacedChannel::refresh`] without blocking. When the adapter is not
//! listed or its device cannot be made, the slot stays lost and the attempt
//! is repeated after 100 ms, doubling to at most 5 s; there is no limit on
//! generations. Every attempt runs on the watcher, never on a real-time
//! thread.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::inventory::{self, AdapterInfo};
use crate::{Channel, GpuError, GpuService, KernelSpec, ServiceState};

/// The first wait before a failed replacement is tried again.
pub const RETRY_FIRST: Duration = Duration::from_millis(100);

/// The longest wait between two replacement attempts.
pub const RETRY_MOST: Duration = Duration::from_secs(5);

/// How often the watcher collects retired channels while some are due.
const DRAIN_EVERY: Duration = Duration::from_millis(100);

/// Replacements offered to one channel and not yet taken.
const OFFERS: usize = 4;

/// Which adapters a pool opens: the machine Setup's `gpu_devices`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// Every adapter (`gpu_devices = "all"`, the default).
    #[default]
    All,
    /// These, each an adapter's id or its index in the inventory
    /// (`gpu_devices = ["…", …]`); an empty list opens none.
    Only(Vec<String>),
}

impl Selection {
    /// As the Setup writes it: `"all"` or `["…", …]`.
    pub fn describe(&self) -> String {
        match self {
            Selection::All => "\"all\"".to_owned(),
            Selection::Only(entries) => {
                let quoted: Vec<String> = entries.iter().map(|e| format!("\"{e}\"")).collect();
                format!("[{}]", quoted.join(", "))
            }
        }
    }
}

/// Where a slot's replacement comes from.
#[derive(Debug)]
enum Source {
    /// The Metal adapter with the slot's id.
    Metal,
    /// A fresh noop adapter (tests only).
    #[cfg(feature = "noop")]
    Noop,
}

/// A channel made on a slot, which the watcher re-creates on each new
/// generation.
struct Registration {
    spec: KernelSpec,
    jobs: usize,
    /// Replacements, to the module.
    rebind: rtrb::Producer<Channel>,
    /// Channels the module retired, back to be dropped here.
    retire: rtrb::Consumer<Channel>,
    /// Replacements offered and not yet retired in exchange.
    outstanding: usize,
    /// The generation of the newest channel offered (or first given) to
    /// the module; 0 for a channel made while the device was lost.
    generation: u64,
}

/// One device of the pool.
struct Slot {
    generation: u64,
    service: Option<GpuService>,
    last_error: Option<String>,
    registrations: Vec<Registration>,
}

impl Slot {
    /// Drops what the modules retired and forgets channels whose module is
    /// gone; whether some replacement is still to be retired.
    fn collect(&mut self) -> bool {
        for registration in &mut self.registrations {
            while let Ok(retired) = registration.retire.pop() {
                drop(retired);
                registration.outstanding = registration.outstanding.saturating_sub(1);
            }
        }
        self.registrations.retain(|r| !r.rebind.is_abandoned());
        self.registrations.iter().any(|r| r.outstanding > 0)
    }

    fn lost(&self) -> bool {
        self.service
            .as_ref()
            .is_none_or(|s| s.state() == ServiceState::Lost)
    }

    /// Offers every channel whose module does not have the current
    /// generation a channel on it (auditor-1-005: an offer never lost to a
    /// full queue; it waits until the module has taken older ones).
    /// Whether some module still lacks it: then the watcher tries again
    /// soon, also after a channel could not be made (auditor-2-019).
    /// `refuse` is the test hook that makes every channel fail.
    fn offer(&mut self, refuse: bool) -> bool {
        let Some(service) = self
            .service
            .as_ref()
            .filter(|s| s.state() == ServiceState::Running)
        else {
            return false;
        };
        let mut behind = false;
        for registration in &mut self.registrations {
            if registration.generation >= self.generation {
                continue;
            }
            if registration.rebind.slots() == 0 {
                behind = true;
                continue;
            }
            let made = if refuse {
                Err(GpuError::Stopped)
            } else {
                service.channel(registration.spec, registration.jobs)
            };
            match made {
                Ok(channel) => {
                    if registration.rebind.push(channel).is_ok() {
                        registration.generation = self.generation;
                        registration.outstanding += 1;
                    }
                }
                Err(e) => {
                    self.last_error = Some(e.to_string());
                    behind = true;
                }
            }
        }
        behind
    }
}

/// A device's state, as [`DevicePool::status`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotStatus {
    /// The device's id.
    pub id: String,
    /// Its service's generation: 1 at open, one more per replacement.
    pub generation: u64,
    /// Its service's state; `None` while it has none (a replacement failed).
    pub state: Option<ServiceState>,
    /// The last error: why it was lost, or why it could not be replaced.
    pub last_error: Option<String>,
    /// The name of its service's thread.
    pub thread: String,
}

struct Inner {
    source: Source,
    devices: Vec<AdapterInfo>,
    slots: Vec<Mutex<Slot>>,
    wake: Mutex<Sender<()>>,
    stop: AtomicBool,
    /// Test hook: no replacement while set.
    hold: AtomicBool,
    /// Test hook: every replacement attempt fails while set.
    refuse: AtomicBool,
    /// Test hook: every replacement channel fails while set.
    refuse_offers: AtomicBool,
    /// Times the watcher woke.
    wakes: AtomicU64,
}

impl Inner {
    fn slot(&self, device: usize) -> Option<MutexGuard<'_, Slot>> {
        self.slots
            .get(device)
            .map(|s| s.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn waker(&self) -> Sender<()> {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A new service for `device`'s adapter.
    fn open(&self, device: usize) -> Result<GpuService, GpuError> {
        if self.refuse.load(Ordering::Acquire) {
            return Err(GpuError::NoDevice("replacement refused (test hook)".into()));
        }
        let id = self
            .devices
            .get(device)
            .map(|d| d.id.clone())
            .ok_or_else(|| GpuError::NoDevice(format!("no device {device}")))?;
        let adapter = match self.source {
            Source::Metal => inventory::adapters(inventory::metal())
                .into_iter()
                .find(|(info, _)| info.id == id)
                .map(|(_, adapter)| adapter)
                .ok_or_else(|| GpuError::NoDevice(format!("adapter `{id}` is not listed")))?,
            #[cfg(feature = "noop")]
            Source::Noop => inventory::noop_adapter()?,
        };
        GpuService::on_adapter(adapter, id, Some(self.waker()))
    }

    /// Replaces every lost service it can and collects retired channels:
    /// whether an attempt failed, and whether retired channels are due.
    fn maintain(&self) -> (bool, bool) {
        let (mut failed, mut draining) = (false, false);
        for (device, slot) in self.slots.iter().enumerate() {
            let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
            let slot = &mut *guard;
            draining |= slot.collect();
            if slot.lost() && !self.hold.load(Ordering::Acquire) {
                // Retired here: its shutdown is bounded (`SHUTDOWN_TIMEOUT`).
                if let Some(old) = slot.service.take() {
                    slot.last_error = old.faults().last_error;
                    drop(old);
                }
                match self.open(device) {
                    Ok(service) => {
                        slot.generation += 1;
                        slot.service = Some(service);
                    }
                    Err(e) => {
                        slot.last_error = Some(e.to_string());
                        failed = true;
                    }
                }
            }
            draining |= slot.offer(self.refuse_offers.load(Ordering::Acquire));
            draining |= slot.registrations.iter().any(|r| r.outstanding > 0);
        }
        (failed, draining)
    }
}

/// The watcher: parked until a loss, a retry or retired channels are due.
fn watch(inner: &Inner, wakes: &Receiver<()>) {
    let mut retry = RETRY_FIRST;
    let mut timeout = None;
    loop {
        let woke = match timeout {
            None => wakes.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(t) => wakes.recv_timeout(t),
        };
        if matches!(woke, Err(RecvTimeoutError::Disconnected)) || inner.stop.load(Ordering::Acquire)
        {
            return;
        }
        while wakes.try_recv().is_ok() {}
        inner.wakes.fetch_add(1, Ordering::Relaxed);
        let (failed, draining) = inner.maintain();
        timeout = if failed {
            let wait = retry;
            retry = (retry * 2).min(RETRY_MOST);
            Some(wait)
        } else {
            retry = RETRY_FIRST;
            draining.then_some(DRAIN_EVERY)
        };
    }
}

/// The devices this process uses, each with its service. Shared by
/// [`Arc`]; dropping the last one stops the watcher and shuts every service
/// down (each bounded). Control side.
pub struct DevicePool {
    inner: Arc<Inner>,
    reports: Vec<String>,
    watcher: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for DevicePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePool")
            .field("devices", &self.inner.devices)
            .finish_non_exhaustive()
    }
}

impl DevicePool {
    /// The adapters `selection` names, sorted by id, each with a service.
    /// An entry that names no adapter, or an adapter whose device cannot be
    /// made, is reported ([`reports`](DevicePool::reports)) and left out.
    pub fn open(selection: &Selection) -> DevicePool {
        let listed = inventory::adapters(inventory::metal());
        let mut reports = Vec::new();
        let chosen: Vec<(AdapterInfo, wgpu::Adapter)> = match selection {
            Selection::All => listed,
            Selection::Only(entries) => {
                let mut wanted = vec![false; listed.len()];
                for entry in entries {
                    let at = match entry.parse::<usize>() {
                        Ok(index) => (index < listed.len()).then_some(index),
                        Err(_) => listed.iter().position(|(info, _)| info.id == *entry),
                    };
                    match at.and_then(|at| wanted.get_mut(at)) {
                        Some(want) => *want = true,
                        None => reports.push(format!(
                            "gpu_devices: \"{entry}\" names no GPU adapter of this machine"
                        )),
                    }
                }
                listed
                    .into_iter()
                    .zip(wanted)
                    .filter_map(|(adapter, want)| want.then_some(adapter))
                    .collect()
            }
        };
        DevicePool::build(Source::Metal, chosen, reports)
    }

    /// Tests only (feature `noop`): `devices` noop devices, `noop-0`,
    /// `noop-1`…, each with its own service; they move jobs like a GPU but
    /// compute nothing.
    #[cfg(feature = "noop")]
    #[doc(hidden)]
    pub fn noop(devices: usize) -> Result<DevicePool, GpuError> {
        let mut chosen = Vec::new();
        for i in 0..devices {
            let adapter = inventory::noop_adapter()?;
            let info =
                inventory::describe(format!("noop-{i}"), &adapter.get_info(), &adapter.limits());
            chosen.push((info, adapter));
        }
        Ok(DevicePool::build(Source::Noop, chosen, Vec::new()))
    }

    /// A pool of no device: every GPU-backed instance has no channel.
    pub fn empty() -> DevicePool {
        DevicePool::build(Source::Metal, Vec::new(), Vec::new())
    }

    fn build(
        source: Source,
        chosen: Vec<(AdapterInfo, wgpu::Adapter)>,
        mut reports: Vec<String>,
    ) -> DevicePool {
        let (wake, wakes) = mpsc::channel();
        let mut devices = Vec::new();
        let mut slots = Vec::new();
        for (mut info, adapter) in chosen {
            match GpuService::on_adapter(adapter, info.id.clone(), Some(wake.clone())) {
                Ok(service) => {
                    info.index = devices.len();
                    devices.push(info);
                    slots.push(Mutex::new(Slot {
                        generation: 1,
                        service: Some(service),
                        last_error: None,
                        registrations: Vec::new(),
                    }));
                }
                Err(e) => reports.push(format!("GPU {}: {e}", info.id)),
            }
        }
        let inner = Arc::new(Inner {
            source,
            devices,
            slots,
            wake: Mutex::new(wake),
            stop: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            refuse: AtomicBool::new(false),
            refuse_offers: AtomicBool::new(false),
            wakes: AtomicU64::new(0),
        });
        let watcher = (!inner.slots.is_empty())
            .then(|| {
                let inner = Arc::clone(&inner);
                thread::Builder::new()
                    .name("signal-gpu-pool".to_owned())
                    .spawn(move || watch(&inner, &wakes))
                    .ok()
            })
            .flatten();
        DevicePool {
            inner,
            reports,
            watcher,
        }
    }

    /// The devices, sorted by id; each one's `index` is its place here.
    /// Fixed for the pool's life.
    pub fn devices(&self) -> &[AdapterInfo] {
        &self.inner.devices
    }

    /// What opening it reported: selection entries that name no adapter,
    /// devices that could not be made.
    pub fn reports(&self) -> &[String] {
        &self.reports
    }

    /// Device `device` of `pool`, to lend an instance.
    pub fn lease(pool: &Arc<DevicePool>, device: usize) -> Lease {
        Lease {
            pool: Arc::clone(pool),
            device,
        }
    }

    /// Every device's state.
    pub fn status(&self) -> Vec<SlotStatus> {
        self.inner
            .devices
            .iter()
            .enumerate()
            .filter_map(|(device, info)| {
                let slot = self.inner.slot(device)?;
                let service = slot.service.as_ref();
                Some(SlotStatus {
                    id: info.id.clone(),
                    generation: slot.generation,
                    state: service.map(GpuService::state),
                    last_error: slot
                        .last_error
                        .clone()
                        .or_else(|| service.and_then(|s| s.faults().last_error)),
                    thread: format!("signal-gpu-{}", info.id),
                })
            })
            .collect()
    }

    /// Test hook (M2.7-2): loses device `device`, as a GPU that went away.
    /// Tests only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn lose_device(&self, device: usize) {
        if let Some(slot) = self.inner.slot(device)
            && let Some(service) = &slot.service
        {
            service.lose_device();
        }
    }

    /// Test hook (M2.7-2): `f` on device `device`'s current service (its
    /// own test hooks: completion delay, injected failures, jobs run). Tests
    /// only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn with_service<R>(&self, device: usize, f: impl FnOnce(&GpuService) -> R) -> Option<R> {
        let slot = self.inner.slot(device)?;
        slot.service.as_ref().map(f)
    }

    /// Test hook (M2.7-2): while `hold`, a lost device is not replaced (as
    /// before M2.7-2: lost for good). Releasing it wakes the watcher. Tests
    /// only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn hold_replacement(&self, hold: bool) {
        self.inner.hold.store(hold, Ordering::Release);
        if !hold {
            let _ = self.inner.waker().send(());
        }
    }

    /// Test hook (M2.7-2): while `refuse`, every replacement attempt fails,
    /// as when the adapter is gone. Tests only (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn refuse_replacement(&self, refuse: bool) {
        self.inner.refuse.store(refuse, Ordering::Release);
    }

    /// Test hook (M2.7-2): while `refuse`, every channel the pool would offer
    /// on a new generation fails to be made. Tests only (feature
    /// `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn refuse_offers(&self, refuse: bool) {
        self.inner.refuse_offers.store(refuse, Ordering::Release);
    }

    /// Test hook (M2.7-2): how often the watcher has woken. Tests only
    /// (feature `test-hooks`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn watcher_wakes(&self) -> u64 {
        self.inner.wakes.load(Ordering::Relaxed)
    }
}

impl Drop for DevicePool {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
        let _ = self.inner.waker().send(());
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

/// One device of a pool, lent to an instance by a [`GpuHost`](crate::GpuHost).
#[derive(Clone, Debug)]
pub struct Lease {
    pool: Arc<DevicePool>,
    device: usize,
}

impl Lease {
    /// The device's index in its pool.
    pub fn device(&self) -> usize {
        self.device
    }

    /// The device's id.
    pub fn device_id(&self) -> &str {
        self.pool
            .devices()
            .get(self.device)
            .map_or("", |d| d.id.as_str())
    }

    /// A channel of `jobs` jobs for `spec` on the device's current service,
    /// as [`GpuService::channel`]; the pool offers it a replacement on each
    /// new generation. While the device is lost the channel is lost too,
    /// until the replacement. Control thread only.
    pub fn channel(&self, spec: KernelSpec, jobs: usize) -> Result<PlacedChannel, GpuError> {
        let mut slot = self
            .pool
            .inner
            .slot(self.device)
            .ok_or_else(|| GpuError::NoDevice(format!("no device {}", self.device)))?;
        slot.collect();
        let (channel, generation) = match &slot.service {
            Some(service) if service.state() != ServiceState::Lost => {
                (service.channel(spec, jobs)?, slot.generation)
            }
            _ => {
                if spec.input_words == 0 || spec.output_words == 0 || jobs == 0 {
                    return Err(GpuError::Kernel(
                        "a channel needs input, output and jobs".into(),
                    ));
                }
                (Channel::detached(spec, jobs), 0)
            }
        };
        let (rebind, offers) = rtrb::RingBuffer::new(OFFERS);
        let (retired, retire) = rtrb::RingBuffer::new(OFFERS);
        slot.registrations.push(Registration {
            spec,
            jobs,
            rebind,
            retire,
            outstanding: 0,
            generation,
        });
        Ok(PlacedChannel {
            channel,
            rebind: offers,
            retire: retired,
            device: self.device,
        })
    }
}

/// A module's channel on its device ([`Channel`] through `Deref`), which
/// takes a new generation's replacement with [`refresh`](PlacedChannel::refresh).
#[derive(Debug)]
pub struct PlacedChannel {
    channel: Channel,
    rebind: rtrb::Consumer<Channel>,
    retire: rtrb::Producer<Channel>,
    device: usize,
}

impl PlacedChannel {
    /// The device's index in its pool.
    pub fn device(&self) -> usize {
        self.device
    }

    /// Takes the replacement the pool offered after a loss, if any: `before`
    /// gets the old channel first, to give back every job taken from it
    /// (a job belongs to its channel), then the replacement takes its place
    /// and the old one goes back to the pool, to be dropped there. Whether
    /// it swapped. Jobs in flight on the old channel never come back.
    /// Real-time safe: no allocation, lock or system call.
    pub fn refresh(&mut self, before: impl FnOnce(&mut Channel)) -> bool {
        if self.retire.slots() == 0 {
            return false;
        }
        let Ok(new) = self.rebind.pop() else {
            return false;
        };
        before(&mut self.channel);
        let old = std::mem::replace(&mut self.channel, new);
        if let Err(rtrb::PushError::Full(old)) = self.retire.push(old) {
            // Checked above; never freed here.
            std::mem::forget(old);
        }
        true
    }
}

impl Deref for PlacedChannel {
    type Target = Channel;

    fn deref(&self) -> &Channel {
        &self.channel
    }
}

impl DerefMut for PlacedChannel {
    fn deref_mut(&mut self) -> &mut Channel {
        &mut self.channel
    }
}
