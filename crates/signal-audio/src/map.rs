//! The audio sinks' channels on the device's channels.

use std::ops::Range;

use crate::config::AudioError;

/// Maps each sink channel to one device channel: sink channel *i* to device
/// channel *i* by default (1 and 2 for a stereo sink), or to explicit targets
/// ([`with_targets`](ChannelMap::with_targets), the Setup's output channel
/// map), or to the routes of several play sides
/// ([`with_routes`](ChannelMap::with_routes), ADR 0038: a channel of the run's
/// audio frame with no route is not played). Device channels no sink channel
/// maps to are 0; sink channels beyond the device's are not played
/// ([`dropped`](ChannelMap::dropped)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMap {
    sink: usize,
    device: usize,
    /// The device channel (from 0) of each sink channel; `None`: not played.
    targets: Vec<Option<usize>>,
}

impl ChannelMap {
    /// The map of a sink with `sink` channels on a device with `device`.
    pub fn new(sink: usize, device: usize) -> ChannelMap {
        ChannelMap {
            sink,
            device,
            targets: (0..sink.min(device)).map(Some).collect(),
        }
    }

    /// The device channels a sink of `sink` channels needs to play on
    /// `targets` (from 0): enough for the sink and for the highest target.
    /// The width to choose the device's configuration for.
    pub fn width_for(sink: usize, targets: &[usize]) -> usize {
        targets.iter().map(|&t| t + 1).fold(sink, usize::max)
    }

    /// The map of a sink with `sink` channels on a device with `device`, with
    /// sink channel *i* on device channel `targets[i]` (from 0). Refused unless
    /// there is one target per sink channel, each a device channel, none
    /// repeated.
    pub fn with_targets(
        sink: usize,
        device: usize,
        targets: Vec<usize>,
    ) -> Result<ChannelMap, AudioError> {
        if targets.len() != sink {
            return Err(AudioError::ChannelMap(format!(
                "{} entries, but the sink has {sink} channels",
                targets.len()
            )));
        }
        for (i, &target) in targets.iter().enumerate() {
            if target >= device {
                return Err(AudioError::ChannelMap(format!(
                    "device channel {} does not exist; the device has {device}",
                    target + 1
                )));
            }
            if targets.iter().take(i).any(|&t| t == target) {
                return Err(AudioError::ChannelMap(format!(
                    "device channel {} is used twice",
                    target + 1
                )));
            }
        }
        Ok(ChannelMap {
            sink,
            device,
            targets: targets.into_iter().map(Some).collect(),
        })
    }

    /// The device channels a routing needs (ADR 0038): its highest target,
    /// at least `least`.
    pub fn width_for_routes(least: usize, routes: &[Option<usize>]) -> usize {
        routes
            .iter()
            .flatten()
            .map(|&t| t + 1)
            .fold(least, usize::max)
    }

    /// The map of a run's audio frame of `routes.len()` channels (several
    /// play sides, ADR 0038) on a device with `device`: channel *i* on device
    /// channel `routes[i]` (from 0), or not played. The routing
    /// (`signal_output::route`) has refused a channel used twice; refused
    /// here too, with a target the device lacks.
    pub fn with_routes(
        device: usize,
        routes: Vec<Option<usize>>,
    ) -> Result<ChannelMap, AudioError> {
        for (i, target) in routes.iter().enumerate() {
            let Some(target) = *target else { continue };
            if target >= device {
                return Err(AudioError::ChannelMap(format!(
                    "device channel {} does not exist; the device has {device}",
                    target + 1
                )));
            }
            if routes.iter().take(i).any(|&t| t == Some(target)) {
                return Err(AudioError::ChannelMap(format!(
                    "device channel {} is used twice",
                    target + 1
                )));
            }
        }
        Ok(ChannelMap {
            sink: routes.len(),
            device,
            targets: routes,
        })
    }

    /// The sink's channels.
    pub fn sink_channels(&self) -> usize {
        self.sink
    }

    /// The device's channels.
    pub fn device_channels(&self) -> usize {
        self.device
    }

    /// The device channel (from 0) of sink channel `channel`, if it is played.
    pub fn target(&self, channel: usize) -> Option<usize> {
        self.targets.get(channel).copied().flatten()
    }

    /// The sink channels (from 0) that have no device channel.
    pub fn dropped(&self) -> Range<usize> {
        self.targets.len()..self.sink
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
            for (&target, &sample) in self.targets.iter().zip(from) {
                if let Some(out) = target.and_then(|t| to.get_mut(t)) {
                    *out = sample;
                }
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

    #[test]
    fn explicit_targets_play_on_the_chosen_device_channels() {
        let map = ChannelMap::with_targets(2, 4, vec![2, 3]).unwrap();
        let mut out = [9.0; 8];
        map.scatter(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, [0.0, 0.0, 0.1, 0.2, 0.0, 0.0, 0.3, 0.4]);
        assert_eq!(map.target(1), Some(3));
        assert!(map.dropped().is_empty());
    }

    #[test]
    fn a_map_that_does_not_fit_is_refused() {
        let refused = |sink, device, targets: Vec<usize>| {
            ChannelMap::with_targets(sink, device, targets)
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            refused(1, 2, vec![0, 1]),
            "the channel map: 2 entries, but the sink has 1 channels"
        );
        assert_eq!(
            refused(2, 2, vec![0]),
            "the channel map: 1 entries, but the sink has 2 channels"
        );
        assert_eq!(
            refused(2, 2, vec![0, 2]),
            "the channel map: device channel 3 does not exist; the device has 2"
        );
        assert_eq!(
            refused(2, 4, vec![1, 1]),
            "the channel map: device channel 2 is used twice"
        );
    }

    /// ADR 0038: `cue` then `main` (canonical order) on a 4-channel device,
    /// cue on 3–4 and main on 1–2; a channel with no route is silent.
    #[test]
    fn routes_put_each_play_side_on_its_device_channels() {
        let map = ChannelMap::with_routes(4, vec![Some(2), Some(3), Some(0), Some(1)]).unwrap();
        let mut out = [9.0; 4];
        map.scatter(&[0.3, 0.4, 0.1, 0.2], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
        let map = ChannelMap::with_routes(4, vec![None, Some(3), Some(0), Some(1)]).unwrap();
        let mut out = [9.0; 4];
        map.scatter(&[0.3, 0.4, 0.1, 0.2], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.4]);
        assert_eq!(map.target(0), None);
        assert!(map.dropped().is_empty());
        assert_eq!(ChannelMap::width_for_routes(2, &[None, Some(3)]), 4);
        assert!(ChannelMap::with_routes(4, vec![Some(4)]).is_err());
        assert!(ChannelMap::with_routes(4, vec![Some(1), Some(1)]).is_err());
    }

    /// A stereo sink mapped to device channels 3 and 4 of a device offering
    /// 2- and 4-channel configurations takes the 4-channel one, so the map
    /// holds (`CoreAudioDriver::open_selected` chooses this way).
    #[test]
    fn the_configuration_is_wide_enough_for_the_targets() {
        use crate::config::{ConfigRange, DeviceInfo, Direction, choose_output};
        let range = |channels| ConfigRange {
            channels,
            min_rate: 48_000,
            max_rate: 48_000,
            buffer: Some((14, 4096)),
            sample_format: "f32".to_owned(),
        };
        let device = DeviceInfo {
            name: "Quad".to_owned(),
            id: None,
            direction: Direction::Output,
            default: false,
            rate: None,
            configs: vec![range(2), range(4)],
        };
        let targets = vec![2, 3];
        assert_eq!(ChannelMap::width_for(2, &targets), 4);
        assert_eq!(ChannelMap::width_for(2, &[0, 1]), 2);
        let choice = choose_output(&device, 48_000, 128, ChannelMap::width_for(2, &targets));
        let channels = usize::from(choice.unwrap().channels);
        assert_eq!(channels, 4);
        let map = ChannelMap::with_targets(2, channels, targets).unwrap();
        assert_eq!((map.target(0), map.target(1)), (Some(2), Some(3)));
        // Without targets the stereo sink keeps the 2-channel configuration.
        assert_eq!(choose_output(&device, 48_000, 128, 2).unwrap().channels, 2);
    }
}
