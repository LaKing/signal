//! How Signal joins an output or input device (ADR 0036): plain code, tested
//! without a device. A device's I/O cycle is shared by every process that
//! plays on it, so changing its buffer while another app plays (a browser, a
//! call) makes that app's sound jump. [`plan_join`] decides from the
//! device's [`DeviceState`] whether the driver may set the buffer:
//!
//! - **The device runs somewhere** (`running_somewhere`): Signal joins it as
//!   it is: it sets no buffer and follows the device's current one.
//! - **Signal would be its only client**: the Setup's buffer is requested
//!   (none with `buffer = "device"`).
//! - **The state could not be read**: as when Signal is the only client.
//!
//! The rate is never the plan's to change: the device's clock is the run's
//! (the owner's decision of 2026-10-23), and a live run plays at the rate the
//! device runs at ([`live_rate`](crate::live_rate)).

/// The buffer a Setup asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferRequest {
    /// This many frames, requested when Signal is the device's only client.
    Frames(u32),
    /// The device's current buffer, always (`buffer = "device"`).
    Device,
}

impl BufferRequest {
    /// The frames asked for, if a number.
    pub fn frames(self) -> Option<u32> {
        match self {
            BufferRequest::Frames(n) => Some(n),
            BufferRequest::Device => None,
        }
    }
}

/// A device's shared state as read before a stream is built
/// (`crate::device_props`); `None` where a read failed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceState {
    /// Whether the device runs for any process, this one included.
    pub running_somewhere: Option<bool>,
    /// Whether it runs for this process (Signal's own input meters, say).
    pub running_here: Option<bool>,
    /// Its I/O buffer, in frames.
    pub buffer: Option<u32>,
    /// Its nominal rate, in Hz.
    pub rate: Option<u32>,
}

impl DeviceState {
    /// Whether another client may be playing on it: it runs somewhere.
    pub fn in_use(&self) -> bool {
        self.running_somewhere == Some(true)
    }
}

/// What the driver does to join a device ([`plan_join`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinPlan {
    /// The device runs for another client (or for Signal itself): nothing
    /// of it is changed.
    pub shared: bool,
    /// The buffer to set on the device (`BufferSize::Fixed`); `None` leaves
    /// it as it is (`BufferSize::Default`).
    pub set_buffer: Option<u32>,
    /// The frames a callback is expected to hold: the buffer set, else the
    /// device's own, else [`FALLBACK_PERIOD`].
    pub period: u32,
}

/// The period assumed when a device's buffer cannot be read and none is
/// set (the engine's own default device buffer).
pub const FALLBACK_PERIOD: u32 = 512;

/// How to join a device with the buffer `request`, given its `state`
/// (module docs).
pub fn plan_join(request: BufferRequest, state: &DeviceState) -> JoinPlan {
    if state.in_use() {
        return JoinPlan {
            shared: true,
            set_buffer: None,
            period: state.buffer.or(request.frames()).unwrap_or(FALLBACK_PERIOD),
        };
    }
    let set_buffer = request.frames();
    JoinPlan {
        shared: false,
        set_buffer,
        period: set_buffer.or(state.buffer).unwrap_or(FALLBACK_PERIOD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(somewhere: Option<bool>, buffer: Option<u32>) -> DeviceState {
        DeviceState {
            running_somewhere: somewhere,
            running_here: Some(false),
            buffer,
            rate: Some(44_100),
        }
    }

    /// Chrome plays on the device at 512 frames: Signal joins at 512 and
    /// sets nothing, whatever buffer the Setup asks for.
    #[test]
    fn a_device_in_use_is_joined_as_it_is() {
        let chrome = state(Some(true), Some(512));
        for request in [BufferRequest::Frames(128), BufferRequest::Device] {
            assert_eq!(
                plan_join(request, &chrome),
                JoinPlan {
                    shared: true,
                    set_buffer: None,
                    period: 512
                }
            );
        }
    }

    /// Alone on the device: the Setup's buffer; `"device"` keeps the
    /// device's buffer.
    #[test]
    fn the_only_client_sets_the_buffer() {
        let idle = state(Some(false), Some(512));
        assert_eq!(
            plan_join(BufferRequest::Frames(128), &idle),
            JoinPlan {
                shared: false,
                set_buffer: Some(128),
                period: 128
            }
        );
        assert_eq!(
            plan_join(BufferRequest::Device, &idle),
            JoinPlan {
                shared: false,
                set_buffer: None,
                period: 512
            }
        );
    }

    /// A state that cannot be read: the Setup's buffer, as before ADR 0036.
    #[test]
    fn an_unread_state_sets_the_buffer_as_before() {
        let unknown = DeviceState::default();
        assert_eq!(
            plan_join(BufferRequest::Frames(256), &unknown),
            JoinPlan {
                shared: false,
                set_buffer: Some(256),
                period: 256
            }
        );
        assert_eq!(
            plan_join(BufferRequest::Device, &unknown).period,
            FALLBACK_PERIOD
        );
        // In use with an unread buffer: nothing set, the request assumed.
        assert_eq!(
            plan_join(BufferRequest::Frames(64), &state(Some(true), None)),
            JoinPlan {
                shared: true,
                set_buffer: None,
                period: 64
            }
        );
    }
}
