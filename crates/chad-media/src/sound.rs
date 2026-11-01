//! Soundboard clips: a file from chadd decoded to the engine's 48 kHz mono.
//! The bytes come from the server, so they're hostile (D14): the output is
//! capped, and callers decode off the UI thread.

use std::io::Cursor;
use std::sync::Arc;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::RATE;

/// Longer clips are cut here.
pub const MAX_SECS: u32 = 10;

/// Decodes the first [`MAX_SECS`] of an mp3, ogg/vorbis, wav or flac file;
/// `name`'s extension is a hint.
pub fn load(bytes: Arc<[u8]>, name: &str) -> Result<Arc<[i16]>, String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some((_, ext)) = name.rsplit_once('.') {
        hint.with_extension(ext);
    }
    let unsupported = |_| "Not a sound file IrChad can play.".to_owned();
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(unsupported)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("It has no audio.")?;
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("It has no audio.")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(unsupported)?;
    let (mut mono, mut rate, mut buf) = (Vec::new(), 0, Vec::<f32>::new());
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // A truncated file still plays up to where it breaks.
            Err(_) if !mono.is_empty() => break,
            Err(_) => return Err("It's damaged.".into()),
        };
        if packet.track_id != id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(a) => a,
            Err(Error::DecodeError(_)) => continue,
            Err(_) => return Err("It's damaged.".into()),
        };
        let channels = audio.spec().channels().count();
        rate = audio.spec().rate();
        if channels == 0 || rate == 0 {
            return Err("It has no audio.".into());
        }
        buf.resize(audio.samples_interleaved(), 0.0);
        audio.copy_to_slice_interleaved(&mut buf);
        mono.extend(
            buf.chunks_exact(channels)
                .map(|f| f.iter().sum::<f32>() / channels as f32),
        );
        if mono.len() as u64 >= u64::from(MAX_SECS) * u64::from(rate) {
            break;
        }
    }
    if mono.is_empty() {
        return Err("It has no audio.".into());
    }
    let mut out = resample(&mono, rate);
    out.truncate((MAX_SECS * RATE) as usize);
    Ok(out.into())
}

// shortcut: linear interpolation aliases a little when downsampling; fine for
// sound effects, use a windowed-sinc resampler if music clips sound harsh.
fn resample(x: &[f32], from: u32) -> Vec<i16> {
    let step = f64::from(from) / f64::from(RATE);
    let n = (x.len() as f64 / step) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let j = p as usize;
            let a = x[j];
            let b = x.get(j + 1).copied().unwrap_or(a);
            let s = a + (b - a) * (p - j as f64) as f32;
            (s * 32767.0).clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit PCM wav: `secs` of a constant level at `rate`, stereo.
    fn wav(rate: u32, secs: u32, level: i16) -> Vec<u8> {
        let data = (rate * secs * 4) as usize;
        let mut w = Vec::new();
        w.extend(b"RIFF");
        w.extend(((36 + data) as u32).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(2u16.to_le_bytes());
        w.extend(rate.to_le_bytes());
        w.extend((rate * 4).to_le_bytes());
        w.extend(4u16.to_le_bytes());
        w.extend(16u16.to_le_bytes());
        w.extend(b"data");
        w.extend((data as u32).to_le_bytes());
        for _ in 0..data / 2 {
            w.extend(level.to_le_bytes());
        }
        w
    }

    #[test]
    fn decodes_resamples_and_caps() {
        let clip = load(wav(24_000, 1, 8192).into(), "a.wav").unwrap();
        assert!(
            (clip.len() as i64 - i64::from(RATE)).abs() < 100,
            "{}",
            clip.len()
        );
        assert!(clip[100..].iter().all(|&s| (s - 8192).abs() <= 1));

        let long = load(wav(48_000, MAX_SECS + 2, 100).into(), "b").unwrap();
        assert_eq!(long.len(), (MAX_SECS * RATE) as usize);

        assert!(load(b"not audio at all"[..].into(), "c.mp3").is_err());
        assert!(load(Arc::from([]), "d.wav").is_err());
    }
}
