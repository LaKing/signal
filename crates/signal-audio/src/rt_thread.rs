//! Real-time worker threads on macOS (ADR 0019 §2, §4): the third module
//! allowed `unsafe` code (AGENTS.md rule 9, ADR 0017's rules, ADR 0020).
//!
//! It reads an output device's I/O workgroup and puts the calling thread
//! into real-time scheduling, and takes it out again. Its calls, the only
//! ones it makes outside safe Rust (ADR 0019 §4, amended 2026-10-12):
//!
//! - `AudioObjectGetPropertyDataSize` / `AudioObjectGetPropertyData`
//!   (`objc2-core-audio`): the system's device list and a device's
//!   `kAudioDevicePropertyIOThreadOSWorkgroup`;
//! - `os_workgroup_join`, `os_workgroup_leave`, `os_retain` and `os_release`,
//!   declared here with `extern "C"` (libSystem; no locked crate binds them);
//! - `thread_policy_set`, `pthread_self`, `pthread_mach_thread_np` and
//!   `mach_timebase_info` (`libc`; the last is deprecated there in favour of
//!   the `mach2` crate, which is not a workspace dependency).
//!
//! The audit:
//! - **The workgroup's lifetime.** A [`Workgroup`] owns one reference: the
//!   property read returns one the caller must release (SDK
//!   `CoreAudio/AudioHardware.h`, `kAudioDevicePropertyIOThreadOSWorkgroup`:
//!   "The caller is responsible for releasing the returned object"), `Clone`
//!   retains another, `Drop` releases its own. Every call that takes the
//!   workgroup is made with the pointer of a live `Workgroup`, so its count
//!   is at least 1. A joined thread holds its own `Workgroup` until it has
//!   left, so whoever else releases theirs, the leave never sees a released
//!   object.
//! - **Join and leave.** `os_workgroup_join` fills a join token that must be
//!   passed to `os_workgroup_leave` by the same thread, and a thread must
//!   leave before it exits (SDK `os/workgroup_object.h`, `os_workgroup_leave`:
//!   "Failing to do so before exiting will result in undefined behavior").
//!   The token lives in a `Box` in this thread's [`ThreadState`] from the
//!   join to the leave (it never moves), the state is thread-local (only its
//!   thread leaves), and the state's `Drop` leaves, so a thread that ends
//!   while joined leaves first. The leave runs on the state's own fields,
//!   never through the thread-local, which may already be being destroyed.
//!   Only real-time threads can join an audio workgroup (Apple, "Understanding
//!   Audio Workgroups"), so the policy is set first.
//! - **The token's layout** is `struct os_workgroup_join_token_opaque_s {
//!   uint32_t sig; char opaque[36]; }` on LP64 (SDK `os/workgroup_base.h`,
//!   `__OS_WORKGROUP_JOIN_TOKEN_SIZE__`): [`JoinToken`], `repr(C)`.
//! - **Thread policies** are set and reset only on the calling thread's own
//!   Mach port, `pthread_mach_thread_np(pthread_self())`, which needs no
//!   release (unlike `mach_thread_self`). The policy structs are `libc`'s,
//!   passed with their `libc` counts.
//! - **Property reads** pass buffers of exactly the size they declare and
//!   use the data only on status 0.
//!
//! The real-time path is never on the audio thread's way: a driver runs
//! [`enter`] and [`exit`] on the workers between blocks
//! ([`Workers::post`](signal_engine::Workers::post)), on the control
//! thread's request. They may allocate and lock.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::mem::size_of;
use std::ptr::{NonNull, null, null_mut};
use std::sync::OnceLock;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyIOThreadOSWorkgroup,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
};

use crate::scheduling::{TimeConstraint, WorkerPath};

/// `os_workgroup_join_token_s` (SDK `os/workgroup_base.h`, LP64). Only
/// libSystem reads its fields.
#[repr(C)]
#[allow(dead_code)]
struct JoinToken {
    sig: u32,
    opaque: [u8; 36],
}

// SAFETY: these are the libSystem declarations of the SDK headers
// `os/workgroup_object.h` (`int os_workgroup_join(os_workgroup_t,
// os_workgroup_join_token_t)`, `void os_workgroup_leave(os_workgroup_t,
// os_workgroup_join_token_t)`) and `os/object.h` (`void *os_retain(void *)`,
// `void os_release(void *)`), with `os_workgroup_t` and the token as
// pointers; none of them unwinds.
unsafe extern "C" {
    fn os_workgroup_join(wg: *mut c_void, token_out: *mut JoinToken) -> c_int;
    fn os_workgroup_leave(wg: *mut c_void, token: *mut JoinToken);
    fn os_retain(object: *mut c_void) -> *mut c_void;
    fn os_release(object: *mut c_void);
}

/// An output device's I/O workgroup (`os_workgroup_t`), owning one
/// reference.
pub struct Workgroup(NonNull<c_void>);

// SAFETY: an os_workgroup is an os_object, reference-counted atomically and
// usable from any thread (joining is per thread, through the calling
// thread's own token); `Workgroup` gives no access but to that pointer.
unsafe impl Send for Workgroup {}
// SAFETY: as for `Send`: shared references only retain it or pass it to
// thread-safe calls.
unsafe impl Sync for Workgroup {}

impl Clone for Workgroup {
    fn clone(&self) -> Workgroup {
        // SAFETY: `self` holds a reference, so the object is live; `os_retain`
        // adds one and returns the same object, owned by the new value.
        unsafe { os_retain(self.0.as_ptr()) };
        Workgroup(self.0)
    }
}

impl Drop for Workgroup {
    fn drop(&mut self) {
        // SAFETY: `self` owns exactly one reference (from the property read
        // or `os_retain`), released once, here.
        unsafe { os_release(self.0.as_ptr()) };
    }
}

impl std::fmt::Debug for Workgroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Workgroup")
    }
}

/// A global property's address.
fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// The system's device list (`kAudioHardwarePropertyDevices`), in its order:
/// what cpal's `Host::devices` lists.
pub fn device_ids() -> Result<Vec<AudioObjectID>, String> {
    let address = address(kAudioHardwarePropertyDevices);
    let system = kAudioObjectSystemObject as AudioObjectID;
    let mut size = 0u32;
    // SAFETY: `address` and `size` are live locals for the call; no
    // qualifier (null, size 0).
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            system,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 {
        return Err(format!("listing the devices failed (OSStatus {status})"));
    }
    let mut ids: Vec<AudioObjectID> = vec![0; size as usize / size_of::<AudioObjectID>()];
    let mut size = u32::try_from(ids.len() * size_of::<AudioObjectID>()).unwrap_or(0);
    // SAFETY: `ids` holds exactly `size` bytes of `AudioObjectID`s, the type
    // this property writes; CoreAudio writes at most `size` bytes and says
    // how many in `size`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            system,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(ids.as_mut_slice()).cast(),
        )
    };
    if status != 0 {
        return Err(format!("listing the devices failed (OSStatus {status})"));
    }
    ids.truncate(size as usize / size_of::<AudioObjectID>());
    Ok(ids)
}

/// The I/O workgroup of the device `device`, or why there is none.
pub fn device_workgroup(device: AudioObjectID) -> Result<Workgroup, String> {
    let address = address(kAudioDevicePropertyIOThreadOSWorkgroup);
    let mut workgroup: *mut c_void = null_mut();
    let mut size = size_of::<*mut c_void>() as u32;
    // SAFETY: the property is an `os_workgroup_t`, a pointer; `workgroup` is
    // a live pointer-sized local and `size` says so.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut workgroup).cast(),
        )
    };
    if status != 0 {
        return Err(format!(
            "the device has no I/O workgroup (OSStatus {status})"
        ));
    }
    NonNull::new(workgroup)
        .map(Workgroup)
        .ok_or_else(|| "the device has no I/O workgroup".to_owned())
}

/// The Mach timebase `(numer, denom)`: one absolute-time tick is
/// numer / denom ns. Read once. (`libc` deprecates its Mach bindings in
/// favour of the `mach2` crate, which is not a workspace dependency.)
#[allow(deprecated)]
pub fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: `info` is a live `mach_timebase_info`, which the call
        // fills; it cannot fail on macOS.
        let status = unsafe { libc::mach_timebase_info(&raw mut info) };
        if status == 0 && info.numer > 0 && info.denom > 0 {
            (info.numer, info.denom)
        } else {
            (1, 1)
        }
    })
}

/// The calling thread's Mach port.
fn this_thread() -> libc::thread_t {
    // SAFETY: `pthread_self` returns the calling thread, and
    // `pthread_mach_thread_np` its port, which the thread owns (nothing to
    // release).
    unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) }
}

/// Sets the calling thread's time-constraint policy; the kern_return.
fn set_time_constraint(policy: &TimeConstraint) -> libc::kern_return_t {
    let mut data = libc::thread_time_constraint_policy {
        period: policy.period,
        computation: policy.computation,
        constraint: policy.constraint,
        preemptible: libc::boolean_t::from(policy.preemptible),
    };
    // SAFETY: `data` is a live `thread_time_constraint_policy`, passed with
    // its flavor and count; the port is this thread's own.
    unsafe {
        libc::thread_policy_set(
            this_thread(),
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            (&raw mut data).cast(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    }
}

/// Puts the calling thread back to the standard (timeshare) policy.
fn set_standard() {
    let mut data = libc::thread_standard_policy { no_data: 0 };
    // SAFETY: `data` is a live `thread_standard_policy`, passed with its
    // flavor and count (0); the port is this thread's own.
    unsafe {
        libc::thread_policy_set(
            this_thread(),
            libc::THREAD_STANDARD_POLICY as libc::thread_policy_flavor_t,
            (&raw mut data).cast(),
            libc::THREAD_STANDARD_POLICY_COUNT as libc::mach_msg_type_number_t,
        );
    }
}

/// What this thread has done: its policy, and the workgroup it joined with
/// the token, which stays in its box until the leave.
struct ThreadState {
    policy: bool,
    joined: Option<(Workgroup, Box<JoinToken>)>,
}

impl ThreadState {
    /// Leaves the workgroup (then releases this thread's reference) and puts
    /// the standard policy back; nothing for what was not done.
    fn release(&mut self) {
        if let Some((workgroup, mut token)) = self.joined.take() {
            // SAFETY: this thread joined `workgroup` with this token (the
            // state is thread-local), the token has not moved since, and
            // `workgroup` holds a reference until after the call.
            unsafe { os_workgroup_leave(workgroup.0.as_ptr(), &raw mut *token) };
            drop(workgroup);
        }
        if self.policy {
            set_standard();
            self.policy = false;
        }
    }
}

/// When the thread ends: leaves and resets on its own fields.
impl Drop for ThreadState {
    fn drop(&mut self) {
        self.release();
    }
}

thread_local! {
    static STATE: RefCell<ThreadState> = const {
        RefCell::new(ThreadState { policy: false, joined: None })
    };
}

/// Schedules the calling thread like the audio thread: the time-constraint
/// `policy` first, then a join of `workgroup` (only real-time threads can
/// join), or `Err` with why there is none. Undoes an earlier `enter` first.
/// The path taken: workgroup, time-constraint (with the join error or the
/// reason) or none (the policy was refused).
pub fn enter(policy: &TimeConstraint, workgroup: Result<&Workgroup, &str>) -> WorkerPath {
    STATE
        .try_with(|state| {
            let mut state = state.borrow_mut();
            state.release();
            let kr = set_time_constraint(policy);
            if kr != 0 {
                return WorkerPath::None(format!("thread_policy_set failed (kern_return {kr})"));
            }
            state.policy = true;
            let workgroup = match workgroup {
                Ok(workgroup) => workgroup.clone(),
                Err(why) => return WorkerPath::TimeConstraint(why.to_owned()),
            };
            let mut token = Box::new(JoinToken {
                sig: 0,
                opaque: [0; 36],
            });
            // SAFETY: `workgroup` holds a reference for the call (and, on
            // success, until the leave); `token` is a live join token that
            // the call fills and that stays in its box until the leave.
            let error = unsafe { os_workgroup_join(workgroup.0.as_ptr(), &raw mut *token) };
            if error != 0 {
                let name = match error {
                    libc::EINVAL => ": cancelled",
                    libc::EALREADY => ": already in another workgroup",
                    _ => "",
                };
                return WorkerPath::TimeConstraint(format!(
                    "os_workgroup_join failed (error {error}{name})"
                ));
            }
            state.joined = Some((workgroup, token));
            WorkerPath::Workgroup
        })
        .unwrap_or_else(|_| WorkerPath::None("the thread is ending".to_owned()))
}

/// Undoes [`enter`] on the calling thread: leaves the workgroup, then puts
/// the standard policy back. Nothing on a thread that never entered.
pub fn exit() {
    let _ = STATE.try_with(|state| state.borrow_mut().release());
}
