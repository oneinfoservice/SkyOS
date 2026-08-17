//! WAV (RIFF) header parsing — pure byte math, no syscalls.
//!
//! Host-testable by design (the same `cfg(not(test))` treatment as the rest
//! of the workspace): only slice indexing and little-endian reads, so the
//! `#[cfg(test)]` module runs under host `cargo test`.

/// Validated fields of a PCM WAV stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    /// Samples per second (from the fmt chunk).
    pub sample_rate: u32,
    /// 1 = mono, 2 = stereo (validated nonzero).
    pub channels: u16,
    /// 8 or 16 — the only bit depths the speaker path accepts.
    pub bits_per_sample: u16,
    /// Byte offset of the first PCM sample.
    pub data_offset: usize,
    /// Byte length of the PCM payload.
    pub data_len: usize,
}

/// Why a byte slice is not a playable PCM WAV.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavError {
    /// Fewer than 12 bytes — not even a RIFF/WAVE shell.
    TooShort,
    /// Missing the "RIFF" magic.
    NotRiff,
    /// Missing the "WAVE" form type.
    NotWave,
    /// No fmt chunk (or one that overruns the buffer).
    MissingFmt,
    /// The fmt chunk declares a non-PCM format, or degenerate geometry
    /// (zero channels / sample rate, unsupported bit depth, tiny chunk).
    UnsupportedFormat,
    /// No data chunk (or one that overruns the buffer).
    MissingData,
}

fn le_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}

fn le_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

/// Parse and validate a WAV container, returning the stream geometry needed
/// to locate and interpret the PCM payload. Chunk order is not assumed
/// (fmt may legally follow data), and odd-sized chunks are skipped with
/// their pad byte per the RIFF spec. The RIFF size field at offset 4 is
/// intentionally not trusted — the chunk walk is authoritative.
pub fn parse_wav(data: &[u8]) -> Result<WavInfo, WavError> {
    if data.len() < 12 {
        return Err(WavError::TooShort);
    }
    if &data[0..4] != b"RIFF" {
        return Err(WavError::NotRiff);
    }
    if &data[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }

    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut bits_per_sample = 0u16;
    let mut data_offset = None;
    let mut data_len = 0usize;

    let mut off = 12usize;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let size = le_u32(data, off + 4) as usize;
        let payload = off + 8;
        if payload + size > data.len() {
            // A chunk header claims more payload than the buffer holds: the
            // file is truncated. A truncated fmt/data chunk is reported as
            // such; an unknown truncated chunk just ends the walk.
            if id == b"fmt " {
                return Err(WavError::MissingFmt);
            }
            if id == b"data" {
                return Err(WavError::MissingData);
            }
            break;
        }
        if id == b"fmt " {
            if size < 16 {
                return Err(WavError::UnsupportedFormat);
            }
            let audio_format = le_u16(data, payload);
            channels = le_u16(data, payload + 2);
            sample_rate = le_u32(data, payload + 4);
            bits_per_sample = le_u16(data, payload + 14);
            if audio_format != 1
                || channels == 0
                || sample_rate == 0
                || (bits_per_sample != 8 && bits_per_sample != 16)
            {
                return Err(WavError::UnsupportedFormat);
            }
        } else if id == b"data" {
            data_offset = Some(payload);
            data_len = size;
        }
        off = payload + size + (size & 1); // chunks are word-aligned
    }

    if sample_rate == 0 || channels == 0 || bits_per_sample == 0 {
        return Err(WavError::MissingFmt);
    }
    match data_offset {
        Some(offset) => Ok(WavInfo {
            sample_rate,
            channels,
            bits_per_sample,
            data_offset: offset,
            data_len,
        }),
        None => Err(WavError::MissingData),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a canonical 44-byte-header PCM WAV: RIFF/WAVE, fmt, data.
    fn pcm_header(sample_rate: u32, channels: u16, bits: u16, data_len: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&36u32.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        let byte_rate = sample_rate * channels as u32 * bits as u32 / 8;
        v.extend_from_slice(&byte_rate.to_le_bytes());
        v.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data_len as u32).to_le_bytes());
        v.extend_from_slice(&vec![0u8; data_len]);
        v
    }

    #[test]
    fn parses_mono_16bit_canonical() {
        let info = parse_wav(&pcm_header(44100, 1, 16, 1000)).unwrap();
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 1);
        assert_eq!(info.bits_per_sample, 16);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.data_len, 1000);
    }

    #[test]
    fn parses_stereo_8bit() {
        let info = parse_wav(&pcm_header(8000, 2, 8, 256)).unwrap();
        assert_eq!(info.sample_rate, 8000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bits_per_sample, 8);
        assert_eq!(info.data_len, 256);
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert_eq!(parse_wav(b""), Err(WavError::TooShort));
        assert_eq!(parse_wav(b"NOTARIFWWAVE"), Err(WavError::NotRiff)); // 12 bytes, wrong magic
        assert_eq!(parse_wav(b"RIFF\x10\x00\x00\x00NOPE"), Err(WavError::NotWave));
        assert_eq!(parse_wav(b"RIFF\x10\x00\x00\x00WAVE"), Err(WavError::MissingFmt));
        // Canonical header cut off mid-data-chunk-payload.
        let full = pcm_header(44100, 1, 16, 100);
        assert_eq!(parse_wav(&full[..40]), Err(WavError::MissingData));
    }

    #[test]
    fn rejects_non_pcm_and_degenerate_fmt() {
        // audio_format = 3 (IEEE float) instead of 1 (PCM).
        let mut wav = pcm_header(44100, 1, 16, 100);
        wav[20..22].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(parse_wav(&wav), Err(WavError::UnsupportedFormat));

        // Zero sample rate.
        assert_eq!(parse_wav(&pcm_header(0, 1, 16, 100)), Err(WavError::UnsupportedFormat));
        // Zero channels.
        assert_eq!(parse_wav(&pcm_header(44100, 0, 16, 100)), Err(WavError::UnsupportedFormat));
        // 24-bit is not a supported speaker format.
        assert_eq!(parse_wav(&pcm_header(44100, 1, 24, 100)), Err(WavError::UnsupportedFormat));
    }

    #[test]
    fn data_before_fmt_still_parses() {
        // The RIFF spec only says fmt is *usually* first; the walk must find
        // it wherever it appears.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF\x00\x00\x00\x00WAVE");
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[1, 2, 3, 4]);
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        let info = parse_wav(&wav).unwrap();
        assert_eq!(info.sample_rate, 8000);
        assert_eq!(info.data_offset, 20); // data payload right after its header
        assert_eq!(info.data_len, 4);
    }

    #[test]
    fn odd_sized_chunks_are_word_aligned() {
        // An odd-size LIST chunk before fmt: the walker must skip the pad
        // byte, otherwise it would misread the fmt magic.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF\x00\x00\x00\x00WAVE");
        wav.extend_from_slice(b"LIST");
        wav.extend_from_slice(&3u32.to_le_bytes());
        wav.extend_from_slice(b"abc"); // odd payload -> pad byte follows
        wav.extend_from_slice(b"\x00");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&22050u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&[0xAB, 0xCD]);
        let info = parse_wav(&wav).unwrap();
        assert_eq!(info.sample_rate, 22050);
        assert_eq!(info.channels, 2);
        assert_eq!(info.data_offset, 56); // 12 + LIST(12) + fmt(24) + data header(8)
        assert_eq!(info.data_len, 2);
    }

    #[test]
    fn truncated_data_chunk_is_rejected() {
        let mut wav = pcm_header(44100, 1, 16, 100);
        wav.truncate(50); // data chunk claims 100 bytes; only 6 remain
        assert_eq!(parse_wav(&wav), Err(WavError::MissingData));
    }
}
