//! The device end of the beeper: a `cpal` stream fed from the sample ring.
//!
//! [`rkw_audio::Output`] does the work — it drains the ring, applies the
//! volume, and fades rather than clicking when the machine has not kept up —
//! and everything here is the wiring around it: find a device, ask it what
//! rate it runs at, and hand its rate to the machine so the resampler targets
//! it (ADR-0021 keeps the rate outside the machine, which is why it can be the
//! device's own).
//!
//! # What the callback may do
//!
//! Nothing that can block or allocate. [`Output::fill`] promises both, and
//! what is added here keeps the promise: the mono-to-many spread is done in
//! chunks of a scratch buffer sized once, and the counters are relaxed atomic
//! adds. A `println!` in this function would be a lock and an underrun.
//!
//! # Sample formats
//!
//! The machine makes `f32` and most devices want `f32`, but a device that
//! wants integers is not an error case worth refusing to start over, so the
//! stream is built generically over `cpal`'s sample types and converted on the
//! way out.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use rkw_audio::{Output, SampleRx, Volume};

/// How many samples the callback spreads at a time. A device asking for more
/// than this gets several passes rather than an allocation; 4096 frames is
/// about 85 ms at 48 kHz, and no host asks for that much.
const CHUNK: usize = 4096;

/// What went wrong opening the speaker.
#[derive(Debug)]
pub enum Error {
    /// The host has no output device at all.
    NoDevice,
    /// The device would not say what it could do, or would not do it.
    Device(cpal::Error),
    /// A sample format `cpal` knows about and this does not.
    Format(SampleFormat),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoDevice => write!(f, "no audio output device"),
            Error::Device(e) => write!(f, "audio device: {e}"),
            Error::Format(format) => write!(f, "unsupported sample format {format}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<cpal::Error> for Error {
    fn from(e: cpal::Error) -> Error {
        Error::Device(e)
    }
}

/// Counters the callback keeps and the window reads.
///
/// Nothing here changes what is played. They are the way to notice that an
/// assumption has come apart: underruns mean the machine is not keeping up,
/// and drops mean the pacing has let it get too far ahead.
#[derive(Debug, Default)]
struct Counters {
    underruns: AtomicU64,
    callbacks: AtomicU64,
}

/// A running audio stream, and what it has to say about how it is going.
///
/// Dropping this stops the stream, which is why the frontend holds on to it.
pub struct Speaker {
    stream: cpal::Stream,
    counters: Arc<Counters>,
    volume: Volume,
    sample_rate: u32,
    channels: u16,
}

impl Speaker {
    /// The rate the device actually runs at, which is what the machine's
    /// resampler has to be built for.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Callbacks that asked for more sound than there was.
    pub fn underruns(&self) -> u64 {
        self.counters.underruns.load(Ordering::Relaxed)
    }

    /// Callbacks served, so that underruns can be read as a proportion rather
    /// than as a number that only grows.
    pub fn callbacks(&self) -> u64 {
        self.counters.callbacks.load(Ordering::Relaxed)
    }

    /// Start playing. A stream that is built but not played is silence with no
    /// error to report, which is a bad half hour.
    pub fn play(&self) -> Result<(), Error> {
        self.stream.play().map_err(Error::from)
    }
}

/// What the default output device says it wants: the rate the machine will be
/// built for, and the format the stream will be built in.
pub struct Device {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

impl Device {
    /// The host's default output.
    pub fn default_output() -> Result<Device, Error> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(Error::NoDevice)?;
        let config = device.default_output_config()?;
        Ok(Device { device, config })
    }

    /// The rate to build the machine's beeper for. Asked before the stream is
    /// opened, because the machine is made first and cannot be told later.
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate()
    }

    /// Open the stream, playing whatever `rx` supplies.
    pub fn open(self, rx: SampleRx, volume: Volume) -> Result<Speaker, Error> {
        let format = self.config.sample_format();
        let config: StreamConfig = self.config.into();
        let sample_rate = config.sample_rate;
        let channels = config.channels;
        let counters = Arc::new(Counters::default());
        let output = Output::new(rx, volume.clone(), sample_rate);

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&self.device, &config, output, &counters),
            SampleFormat::I16 => build::<i16>(&self.device, &config, output, &counters),
            SampleFormat::U16 => build::<u16>(&self.device, &config, output, &counters),
            SampleFormat::I32 => build::<i32>(&self.device, &config, output, &counters),
            SampleFormat::F64 => build::<f64>(&self.device, &config, output, &counters),
            other => return Err(Error::Format(other)),
        }?;

        let speaker = Speaker {
            stream,
            counters,
            volume,
            sample_rate,
            channels,
        };
        speaker.play()?;
        Ok(speaker)
    }
}

/// The callback, in whatever sample type the device wanted.
fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut output: Output,
    counters: &Arc<Counters>,
) -> Result<cpal::Stream, Error> {
    let channels = config.channels as usize;
    let counters = Arc::clone(counters);
    // Sized once, here, rather than in the callback.
    let mut mono = vec![0.0f32; CHUNK];

    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            counters.callbacks.fetch_add(1, Ordering::Relaxed);
            let mut underran = false;
            for block in data.chunks_mut(CHUNK * channels) {
                let frames = block.len() / channels;
                let mono = &mut mono[..frames];
                underran |= output.fill(mono).underran();
                spread(mono, block, channels);
            }
            if underran {
                counters.underruns.fetch_add(1, Ordering::Relaxed);
            }
        },
        // A device that errors mid-stream cannot be rescued from here, and the
        // machine goes on running without sound.
        |e| eprintln!("rkw: audio stream: {e}"),
        None,
    )?;
    Ok(stream)
}

/// One mono sample to every channel of a frame: the beeper is one bit of one
/// port, and putting it anywhere but the middle would be an invention.
fn spread<T: SizedSample + FromSample<f32>>(mono: &[f32], out: &mut [T], channels: usize) {
    for (frame, sample) in out.chunks_mut(channels).zip(mono) {
        let value = T::from_sample(*sample);
        for slot in frame.iter_mut() {
            *slot = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mono_sample_goes_to_every_channel() {
        let mono = [0.25f32, -0.5];
        let mut out = [0.0f32; 4];
        spread(&mono, &mut out, 2);
        assert_eq!(out, [0.25, 0.25, -0.5, -0.5]);
    }

    /// A device asking for fewer frames than there are samples is the ordinary
    /// case, and the tail must be left alone rather than filled with the last
    /// sample again.
    #[test]
    fn a_short_buffer_takes_only_what_fits() {
        let mono = [1.0f32, 1.0, 1.0];
        let mut out = [0.0f32; 2];
        spread(&mono, &mut out, 1);
        assert_eq!(out, [1.0, 1.0]);
    }
}
