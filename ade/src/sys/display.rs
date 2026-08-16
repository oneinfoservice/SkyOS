//! Display pure logic — resolution validation, aspect ratio, DPI scaling,
//! and framebuffer pitch.
//!
//! Host-testable by design: pure integer math, so the `#[cfg(test)]` module
//! runs under host `cargo test` (the same cfg(not(test)) treatment as
//! libsarga's errno/net/semver).

/// Reduce a resolution to its aspect ratio `(w, h)` (e.g. 1920x1080 -> (16, 9)).
/// Zero dimensions pass through unchanged (the caller decides it is invalid).
pub fn aspect_ratio(w: u32, h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (w, h);
    }
    let g = gcd(w, h);
    (w / g, h / g)
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Snap a screen DPI to the nearest UI scale step: 100/125/150/200%.
/// A `base_dpi` of 0 is degenerate and yields 100.
pub fn scale_for_dpi(dpi: u32, base_dpi: u32) -> u8 {
    let ratio = if base_dpi == 0 {
        100
    } else {
        dpi.saturating_mul(100) / base_dpi
    };
    [100u32, 125, 150, 200]
        .into_iter()
        .min_by_key(|&step| (ratio as i64 - step as i64).abs())
        .unwrap() as u8
}

/// A mode is valid when both dimensions and the refresh rate are sane
/// (>= 320x200, 30..=240 Hz).
pub fn mode_valid(w: u32, h: u32, refresh: u32) -> bool {
    w >= 320 && h >= 200 && (30..=240).contains(&refresh)
}

/// Framebuffer row pitch in bytes for `w` pixels at `bpp` bits per pixel,
/// 32-bit aligned (the standard VBE/linear-framebuffer requirement).
pub fn pitch_bytes(w: u32, bpp: u8) -> usize {
    let bits = w as usize * bpp as usize;
    bits.div_ceil(32) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_aspect_ratios() {
        assert_eq!(aspect_ratio(1920, 1080), (16, 9));
        assert_eq!(aspect_ratio(2560, 1440), (16, 9));
        assert_eq!(aspect_ratio(1366, 768), (683, 384)); // not 16:9 — pinned as-is
        assert_eq!(aspect_ratio(1024, 768), (4, 3));
        assert_eq!(aspect_ratio(800, 600), (4, 3));
        assert_eq!(aspect_ratio(1, 1), (1, 1));
    }

    #[test]
    fn aspect_ratio_zero_passthrough() {
        assert_eq!(aspect_ratio(0, 1080), (0, 1080));
        assert_eq!(aspect_ratio(1920, 0), (1920, 0));
    }

    #[test]
    fn dpi_scale_snaps_to_steps() {
        assert_eq!(scale_for_dpi(96, 96), 100);
        assert_eq!(scale_for_dpi(120, 96), 125);
        assert_eq!(scale_for_dpi(144, 96), 150);
        assert_eq!(scale_for_dpi(192, 96), 200);
        assert_eq!(scale_for_dpi(110, 96), 125); // 114.6% rounds up to 125
        assert_eq!(scale_for_dpi(104, 96), 100); // 108.3% rounds down to 100
        assert_eq!(scale_for_dpi(0, 96), 100);
        assert_eq!(scale_for_dpi(96, 0), 100);
    }

    #[test]
    fn mode_validation() {
        assert!(mode_valid(1920, 1080, 60));
        assert!(mode_valid(320, 200, 30));
        assert!(!mode_valid(319, 200, 60)); // too narrow
        assert!(!mode_valid(1920, 199, 60)); // too short
        assert!(!mode_valid(1920, 1080, 29)); // too slow
        assert!(!mode_valid(1920, 1080, 241)); // too fast
    }

    #[test]
    fn pitch_is_32bit_aligned() {
        assert_eq!(pitch_bytes(640, 32), 2560); // 640*4
        assert_eq!(pitch_bytes(641, 32), 2564); // rounds up to 4
        assert_eq!(pitch_bytes(800, 16), 1600);
        assert_eq!(pitch_bytes(801, 16), 1604);
        assert_eq!(pitch_bytes(320, 8), 320);
        assert_eq!(pitch_bytes(0, 32), 0);
        for w in [320u32, 641, 1024, 1366, 1920] {
            assert_eq!(pitch_bytes(w, 32) % 4, 0);
        }
    }
}
