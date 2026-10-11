//! The audio sink's channels on the device's channels.

use std::ops::Range;

/// Maps sink channel *i* to device channel *i* (1 and 2 for a stereo sink),
/// for the channels both have. Device channels beyond the sink's are 0; sink
/// channels beyond the device's are not played ([`dropped`](ChannelMap::dropped)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelMap {
    sink: usize,
    device: usize,
}

impl ChannelMap {
    /// The map of a sink with `sink` channels on a device with `device`.
    pub fn new(sink: usize, device: usize) -> ChannelMap {
        ChannelMap { sink, device }
    }

    /// The sink's channels.
    pub fn sink_channels(&self) -> usize {
        self.sink
    }

    /// The device's channels.
    pub fn device_channels(&self) -> usize {
        self.device
    }

    /// The sink channels (from 0) that have no device channel.
    pub fn dropped(&self) -> Range<usize> {
        self.device.min(self.sink)..self.sink
    }

    /// Writes `src`, frames of the sink's channels interleaved, to `dst`,
    /// frames of the device's channels interleaved, for the frames both hold.
    /// Unmapped device channels are 0. Allocates nothing.
    pub fn scatter(&self, src: &[f32], dst: &mut [f32]) {
        if self.sink == 0 || self.device == 0 {
            dst.fill(0.0);
            return;
        }
        for (from, to) in src
            .chunks_exact(self.sink)
            .zip(dst.chunks_exact_mut(self.device))
        {
            to.fill(0.0);
            for (out, &sample) in to.iter_mut().zip(from) {
                *out = sample;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelMap;

    #[test]
    fn a_stereo_sink_plays_on_device_channels_1_and_2() {
        let map = ChannelMap::new(2, 2);
        let mut out = [9.0; 4];
        map.scatter(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
        assert!(map.dropped().is_empty());
    }

    #[test]
    fn extra_device_channels_are_silent() {
        let map = ChannelMap::new(2, 4);
        let mut out = [9.0; 8];
        map.scatter(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0, 0.3, 0.4, 0.0, 0.0]);
    }

    #[test]
    fn a_mono_device_plays_the_first_sink_channel_only() {
        let map = ChannelMap::new(2, 1);
        let mut out = [9.0; 2];
        map.scatter(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, [0.1, 0.3]);
        assert_eq!(map.dropped(), 1..2);
    }

    #[test]
    fn a_mono_sink_plays_on_device_channel_1() {
        let map = ChannelMap::new(1, 2);
        let mut out = [9.0; 4];
        map.scatter(&[0.5, -0.5], &mut out);
        assert_eq!(out, [0.5, 0.0, -0.5, 0.0]);
    }
}
