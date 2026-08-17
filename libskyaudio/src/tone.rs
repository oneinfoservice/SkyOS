//! Beep/tone generation math — pure integer arithmetic, no syscalls.
//!
//! Host-testable by design (the same `cfg(not(test))` treatment as the rest
//! of the workspace): a 32-entry sine table with linear interpolation turns
//! a frequency into 16-bit PCM samples, so the `#[cfg(test)]` module runs
//! under host `cargo test`.

/// One full cycle of a sine wave over 32 entries, amplitude 8192 (a quarter
/// of the i16 range — headroom for later mixing). Index `i` holds
/// `sin(2π·i/32) · 8192` rounded to the nearest integer.
pub const SINE_TABLE: [i16; 32] = [
    0, 1598, 3135, 4552, 5793, 6811, 7568, 8035, 8192, 8035, 7568, 6811,
    5793, 4552, 3135, 1598, 0, -1598, -3135, -4552, -5793, -6811, -7568,
    -8035, -8192, -8035, -7568, -6811, -5793, -4552, -3135, -1598,
];

/// Peak amplitude of [`SINE_TABLE`].
pub const AMPLITUDE: i16 = 8192;

/// One 16-bit PCM sample of a sine tone at `freq_hz`, taken at sample index
/// `n` of a stream running at `sample_rate`. Degenerate inputs (zero
/// frequency or zero sample rate) yield silence.
pub fn sine_sample(freq_hz: u32, sample_rate: u32, n: u32) -> i16 {
    if freq_hz == 0 || sample_rate == 0 {
        return 0;
    }
    // The phase advances `freq`/`rate` of a cycle per sample. The u64 math
    // keeps `n * freq` from overflowing across the full u32 ranges.
    let phase = (n as u64 * freq_hz as u64) % sample_rate as u64;
    let slot = (phase * 32) / sample_rate as u64; // 0..=31 table index
    let frac = ((phase * 32) % sample_rate as u64) as u32; // interp weight
    let i0 = slot as usize;
    let i1 = (i0 + 1) % 32;
    let a = SINE_TABLE[i0] as i32;
    let b = SINE_TABLE[i1] as i32;
    let step = (b - a) * frac as i32 / sample_rate as i32;
    (a + step) as i16
}

/// Number of samples a tone of `duration_ms` occupies at `sample_rate`.
/// Truncates toward zero, so a sub-sample duration yields zero samples.
pub fn tone_samples(duration_ms: u32, sample_rate: u32) -> u32 {
    duration_ms * sample_rate / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_cycle_lands_on_table_extremes() {
        // freq = rate/4: the phase advances exactly 8 table slots per
        // sample, hitting 0, peak, 0, trough, 0 exactly.
        let sr = 8000;
        let f = sr / 4;
        assert_eq!(sine_sample(f, sr, 0), 0);
        assert_eq!(sine_sample(f, sr, 1), 8192);
        assert_eq!(sine_sample(f, sr, 2), 0);
        assert_eq!(sine_sample(f, sr, 3), -8192);
        assert_eq!(sine_sample(f, sr, 4), 0);
    }

    #[test]
    fn eighth_cycle_hits_the_table_entry() {
        // freq = rate/8: one sample advances 4 slots, so the first sample
        // after t=0 is sin(π/4)·8192 = 5793.
        let sr = 8000;
        assert_eq!(sine_sample(sr / 8, sr, 1), 5793);
        assert_eq!(sine_sample(sr / 8, sr, 2), 8192);
    }

    #[test]
    fn period_repeats_exactly() {
        // 1000 Hz @ 8000 Hz = 8 samples per cycle; the waveform must be
        // bit-identical one period later.
        let sr = 8000;
        for n in 0..64u32 {
            assert_eq!(sine_sample(1000, sr, n), sine_sample(1000, sr, n + 8));
        }
    }

    #[test]
    fn amplitude_is_bounded_across_a_coprime_freq() {
        // 997 Hz over a full second at 8000 Hz: coprime with the rate, so
        // every interpolated phase is exercised and no sample ever clips.
        for n in 0..8000u32 {
            let s = sine_sample(997, 8000, n);
            assert!(s.abs() <= AMPLITUDE, "sample {s} exceeded amplitude");
        }
    }

    #[test]
    fn whole_cycles_have_zero_dc() {
        // freq = rate/32: exact table hits with no interpolation, and the
        // table is antisymmetric, so every full cycle sums to exactly zero.
        let sr = 8000;
        let sum: i64 = (0..64u32).map(|n| sine_sample(sr / 32, sr, n) as i64).sum();
        assert_eq!(sum, 0);
    }

    #[test]
    fn degenerate_inputs_are_silent() {
        assert_eq!(sine_sample(0, 8000, 5), 0);
        assert_eq!(sine_sample(440, 0, 5), 0);
        assert_eq!(sine_sample(0, 0, 0), 0);
    }

    #[test]
    fn length_math_truncates_toward_zero() {
        assert_eq!(tone_samples(1000, 8000), 8000);
        assert_eq!(tone_samples(125, 8000), 1000);
        assert_eq!(tone_samples(1, 44100), 44); // 44.1 -> 44
        assert_eq!(tone_samples(0, 44100), 0);
    }
}
