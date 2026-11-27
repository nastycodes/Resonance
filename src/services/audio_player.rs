use std::io::Cursor;

use rodio::{Decoder, OutputStream, Sink};
use zeroize::Zeroizing;

pub struct AudioPlayer {
    sink: Sink,
    _stream: OutputStream,
}

impl AudioPlayer {
    pub fn start(bytes: Zeroizing<Vec<u8>>) -> Result<Self, String> {
        let decoder = Decoder::new(Cursor::new(bytes.as_slice().to_vec()))
            .map_err(|error| format!("This audio format could not be played: {error}"))?;
        let (stream, stream_handle) = OutputStream::try_default()
            .map_err(|error| format!("No audio output device is available: {error}"))?;
        let sink = Sink::try_new(&stream_handle)
            .map_err(|error| format!("Could not start audio playback: {error}"))?;
        sink.append(decoder);
        sink.play();
        Ok(Self {
            sink,
            _stream: stream,
        })
    }

    pub fn pause(&self) {
        self.sink.pause();
    }

    pub fn play(&self) {
        self.sink.play();
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }
}

#[cfg(test)]
mod tests {
    use super::AudioPlayer;
    use zeroize::Zeroizing;

    #[test]
    fn invalid_audio_is_rejected_before_accessing_an_output_device() {
        assert!(AudioPlayer::start(Zeroizing::new(vec![0, 1, 2, 3])).is_err());
    }
}
