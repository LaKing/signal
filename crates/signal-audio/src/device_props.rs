//! A device's shared state, read from CoreAudio (ADR 0036): the fourth
//! module allowed `unsafe` code (AGENTS.md rule 9, ADR 0017's rules).
//!
//! It only **reads**: whether the device runs for any process
//! (`kAudioDevicePropertyDeviceIsRunningSomewhere`) and for this one
//! (`kAudioDevicePropertyDeviceIsRunning`), its I/O buffer
//! (`kAudioDevicePropertyBufferFrameSize`), its nominal rate
//! (`kAudioDevicePropertyNominalSampleRate`), the physical format cpal
//! reads on the device (`kAudioStreamPropertyPhysicalFormat`, for the
//! measurements of ADR 0036) and its output latency and safety offset
//! (`kAudioDevicePropertyLatency`, `kAudioDevicePropertySafetyOffset`, in
//! the output scope: a secondary device's reported latency, ADR 0052 §3)
//! and an aggregate device's active sub-devices
//! (`kAudioAggregateDevicePropertyActiveSubDeviceList`, ADR 0052 §9). It never sets a property: what Signal changes
//! on a device, cpal changes while it builds a stream, and
//! [`plan_join`](crate::join::plan_join) decides whether that may happen.
//!
//! Its calls, the only ones it makes outside safe Rust:
//! `AudioObjectGetPropertyData` and, for the sub-device list,
//! `AudioObjectGetPropertyDataSize` (`objc2-core-audio`) on a device object
//! found by [`device_ids`](crate::rt_thread::device_ids).
//!
//! The audit:
//! - **Every read** passes a live local of exactly the type the property
//!   writes (`u32` for the flags, the buffer and the latencies, `f64` for the rate,
//!   [`PhysicalFormat`] for the format), its size in the size argument, and
//!   no qualifier (null, size 0), in the global scope or, for the
//!   latencies, the output scope. The value is used only on status 0, and
//!   only when CoreAudio says it wrote all of it.
//! - **The sub-device list** is an array of `AudioObjectID`s: its size is
//!   asked first, a `Vec` of that many ids is passed with exactly its size
//!   in bytes, and it is cut to the ids CoreAudio says it wrote. A device
//!   that is no aggregate answers with an error status: an empty list.
//! - **The device object** may be gone by the time it is read (unplugged):
//!   CoreAudio then answers with an error status (`kAudioHardwareBadObjectError`),
//!   never undefined behaviour, and the read reports `None`.
//! - **Never on the audio thread**: the driver reads before it builds a
//!   stream, on the control thread. A read may take CoreAudio's locks.

#![allow(unsafe_code)]

use std::mem::size_of;
use std::ptr::{NonNull, null};

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioAggregateDevicePropertyActiveSubDeviceList,
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyDeviceIsRunning,
    kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, kAudioStreamPropertyPhysicalFormat,
};

use crate::join::DeviceState;

/// `AudioStreamBasicDescription` (SDK `CoreAudioTypes/CoreAudioBaseTypes.h`):
/// the physical format as cpal reads it on the device. Declared here
/// (`objc2-core-audio-types` is not a workspace dependency), `repr(C)`, 40
/// bytes on LP64 as in the SDK.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalFormat {
    /// `mSampleRate`.
    pub sample_rate: f64,
    /// `mFormatID` (`lpcm`).
    pub format_id: u32,
    /// `mFormatFlags` (float, signed integer, packed, …).
    pub format_flags: u32,
    /// `mBytesPerPacket`.
    pub bytes_per_packet: u32,
    /// `mFramesPerPacket`.
    pub frames_per_packet: u32,
    /// `mBytesPerFrame`.
    pub bytes_per_frame: u32,
    /// `mChannelsPerFrame`.
    pub channels_per_frame: u32,
    /// `mBitsPerChannel`.
    pub bits_per_channel: u32,
    /// `mReserved`.
    pub reserved: u32,
}

const _: () = assert!(size_of::<PhysicalFormat>() == 40);

/// A property's address in `scope`.
fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads the global property `selector` of `object` into a `T`; `None` on
/// an error status or a short answer. `T` must be the property's own type
/// (the callers below are the whole list).
fn read<T: Copy + Default>(object: AudioObjectID, selector: u32) -> Option<T> {
    read_in(object, selector, kAudioObjectPropertyScopeGlobal)
}

/// [`read`] in `scope`.
fn read_in<T: Copy + Default>(object: AudioObjectID, selector: u32, scope: u32) -> Option<T> {
    let address = address(selector, scope);
    let mut value = T::default();
    let wanted = u32::try_from(size_of::<T>()).ok()?;
    let mut size = wanted;
    // SAFETY: `address`, `size` and `value` are live locals for the call;
    // `value` is a `T` of `size` bytes, the type the property writes (each
    // caller passes the property's own type), so CoreAudio writes at most
    // `size` bytes into it; no qualifier (null, size 0). An object that is
    // gone answers with an error status.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0 && size == wanted).then_some(value)
}

/// Whether `object` runs for any process, this one included.
pub fn running_somewhere(object: AudioObjectID) -> Option<bool> {
    read::<u32>(object, kAudioDevicePropertyDeviceIsRunningSomewhere).map(|v| v != 0)
}

/// Whether `object` runs for this process.
pub fn running_here(object: AudioObjectID) -> Option<bool> {
    read::<u32>(object, kAudioDevicePropertyDeviceIsRunning).map(|v| v != 0)
}

/// `object`'s I/O buffer, in frames: the period every client of it runs at.
pub fn buffer_frames(object: AudioObjectID) -> Option<u32> {
    read::<u32>(object, kAudioDevicePropertyBufferFrameSize)
}

/// `object`'s nominal sample rate, in whole Hz.
pub fn nominal_rate(object: AudioObjectID) -> Option<u32> {
    read::<f64>(object, kAudioDevicePropertyNominalSampleRate)
        .filter(|r| r.is_finite() && *r > 0.0 && *r < f64::from(u32::MAX))
        .map(|r| r.round() as u32)
}

/// `object`'s physical format as cpal reads it, on the device object.
pub fn physical_format(object: AudioObjectID) -> Option<PhysicalFormat> {
    read::<PhysicalFormat>(object, kAudioStreamPropertyPhysicalFormat)
}

/// `object`'s output latency plus its safety offset, in frames: what the
/// device adds after a callback's frames (ADR 0052 §3).
pub fn output_latency(object: AudioObjectID) -> Option<u32> {
    let latency = read_in::<u32>(
        object,
        kAudioDevicePropertyLatency,
        kAudioObjectPropertyScopeOutput,
    )?;
    let offset = read_in::<u32>(
        object,
        kAudioDevicePropertySafetyOffset,
        kAudioObjectPropertyScopeOutput,
    )?;
    latency.checked_add(offset)
}

/// The devices an aggregate device `object` uses now (ADR 0052 §9); empty
/// for a device that is no aggregate, or gone.
pub fn active_sub_devices(object: AudioObjectID) -> Vec<AudioObjectID> {
    let address = address(
        kAudioAggregateDevicePropertyActiveSubDeviceList,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut size = 0u32;
    // SAFETY: `address` and `size` are live locals for the call; no
    // qualifier (null, size 0).
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 || size == 0 {
        return Vec::new();
    }
    let mut ids: Vec<AudioObjectID> = vec![0; size as usize / size_of::<AudioObjectID>()];
    let mut size = u32::try_from(ids.len() * size_of::<AudioObjectID>()).unwrap_or(0);
    // SAFETY: `ids` holds exactly `size` bytes of `AudioObjectID`s, the type
    // this property writes; CoreAudio writes at most `size` bytes and says
    // how many in `size`; no qualifier.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(ids.as_mut_slice()).cast(),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / size_of::<AudioObjectID>());
    ids
}

/// `object`'s state as [`plan_join`](crate::join::plan_join) reads it.
pub fn state(object: AudioObjectID) -> DeviceState {
    DeviceState {
        running_somewhere: running_somewhere(object),
        running_here: running_here(object),
        buffer: buffer_frames(object),
        rate: nominal_rate(object),
    }
}

#[cfg(test)]
mod tests {
    /// On the machine (`--ignored`): every listed device answers the
    /// sub-device read, an aggregate with its members, any other with none.
    #[test]
    #[ignore = "reads the machine's CoreAudio devices"]
    fn every_device_answers_the_sub_device_read() {
        let ids = crate::rt_thread::device_ids().unwrap();
        for id in ids {
            let subs = super::active_sub_devices(id);
            assert!(!subs.contains(&id), "{id} lists itself");
            println!("device {id}: sub-devices {subs:?}");
        }
    }
}
