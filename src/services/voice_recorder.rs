use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use zeroize::{Zeroize, Zeroizing};

use super::attachment::MAX_ATTACHMENT_BYTES;

const MAX_VOICE_SAMPLES: usize = (MAX_ATTACHMENT_BYTES - 128) / 2;

pub struct VoiceRecorder {
    stream: Option<cpal::Stream>,
    samples: Arc<Mutex<Vec<i16>>>,
    sample_rate: u32,
}

impl VoiceRecorder {
    pub fn start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No microphone is available.".to_string())?;
        let supported_config = device
            .default_input_config()
            .map_err(|error| format!("Could not read the microphone format: {error}"))?;
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;
        if sample_rate == 0 || channels == 0 {
            return Err("The microphone returned an invalid audio format.".into());
        }
        let config: cpal::StreamConfig = supported_config.clone().into();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::F32 => {
                build_stream::<f32, _>(&device, &config, samples.clone(), channels, |sample| {
                    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                })
            }
            cpal::SampleFormat::F64 => {
                build_stream::<f64, _>(&device, &config, samples.clone(), channels, |sample| {
                    (sample.clamp(-1.0, 1.0) * i16::MAX as f64) as i16
                })
            }
            cpal::SampleFormat::I16 => {
                build_stream::<i16, _>(&device, &config, samples.clone(), channels, |sample| sample)
            }
            cpal::SampleFormat::I32 => {
                build_stream::<i32, _>(&device, &config, samples.clone(), channels, |sample| {
                    (sample >> 16) as i16
                })
            }
            cpal::SampleFormat::U16 => {
                build_stream::<u16, _>(&device, &config, samples.clone(), channels, |sample| {
                    (sample as i32 - 32768) as i16
                })
            }
            cpal::SampleFormat::U32 => {
                build_stream::<u32, _>(&device, &config, samples.clone(), channels, |sample| {
                    ((sample as i64 - 2_147_483_648) >> 16) as i16
                })
            }
            format => {
                return Err(format!(
                    "The microphone format {format:?} is not supported."
                ));
            }
        }
        .map_err(|error| format!("Could not start microphone recording: {error}"))?;
        stream
            .play()
            .map_err(|error| format!("Could not start microphone recording: {error}"))?;
        Ok(Self {
            stream: Some(stream),
            samples,
            sample_rate,
        })
    }

    pub fn finish(mut self) -> Result<Zeroizing<Vec<u8>>, String> {
        drop(self.stream.take());
        let mut sample_buffer = self
            .samples
            .lock()
            .map_err(|_| "The voice recording buffer is unavailable.".to_string())?;
        let samples = Zeroizing::new(std::mem::take(&mut *sample_buffer));
        drop(sample_buffer);
        if samples.is_empty() {
            return Err("No audio was recorded.".into());
        }
        encode_wav(&samples, self.sample_rate)
    }
}

impl Drop for VoiceRecorder {
    fn drop(&mut self) {
        drop(self.stream.take());
        if let Ok(mut samples) = self.samples.lock() {
            samples.zeroize();
        }
    }
}

fn build_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Arc<Mutex<Vec<i16>>>,
    channels: usize,
    convert: F,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + Copy,
    F: Fn(T) -> i16 + Send + 'static,
{
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            let Ok(mut output) = samples.lock() else {
                return;
            };
            for frame in input.chunks_exact(channels) {
                if output.len() >= MAX_VOICE_SAMPLES {
                    break;
                }
                let mixed = frame
                    .iter()
                    .map(|sample| i32::from(convert(*sample)))
                    .sum::<i32>()
                    / channels as i32;
                output.push(mixed as i16);
            }
        },
        |error| eprintln!("Microphone stream error: {error}"),
        None,
    )
}

fn encode_wav(samples: &[i16], sample_rate: u32) -> Result<Zeroizing<Vec<u8>>, String> {
    let data_size = samples
        .len()
        .checked_mul(2)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| "The voice recording is too large.".to_string())?;
    let mut wav = Zeroizing::new(Vec::with_capacity(data_size as usize + 44));
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    if wav.len() > MAX_ATTACHMENT_BYTES - 16 {
        return Err("The voice recording exceeds the 25 MiB attachment limit.".into());
    }
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_describes_mono_pcm_audio() {
        let wav = encode_wav(&[1, -1, 0], 16_000).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
    }
}
