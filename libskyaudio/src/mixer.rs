//! Audio mixer pure logic — volume scaling, balance, and level validation.
//!
//! Host-testable by design: only integer arithmetic on mixer levels, no
//! syscalls, so the `#[cfg(test)]` module runs under host `cargo test` (the
//! same cfg(not(test)) treatment as libsarga's errno/net/semver).

/// Scale a linear mixer level (0..=max) to a percentage.
pub fn level_to_percent(level: u8, max: u8) -> u8 {
    if max == 0 {
        return 0;
    }
    (level.min(max) as u32 * 100 / max as u32) as u8
}

/// Convert a percentage (0..=100) back to a linear mixer level (0..=max).
pub fn percent_to_level(pct: u8, max: u8) -> u8 {
    (pct.min(100) as u32 * max as u32 / 100) as u8
}

/// Stereo balance: `balance` in -100 (full left) ..= 100 (full right).
/// Returns `(left, right)` channel levels derived from a master `level`.
/// The dominant channel holds the master level; the opposite channel
/// attenuates linearly to silence at hard pan (so turning the balance never
/// boosts a channel above the master).
pub fn balance_levels(level: u8, max: u8, balance: i8) -> (u8, u8) {
    let b = balance.clamp(-100, 100) as i32;
    let l = if b <= 0 {
        level as i32
    } else {
        level as i32 * (100 - b) / 100
    };
    let r = if b >= 0 {
        level as i32
    } else {
        level as i32 * (100 + b) / 100
    };
    (
        l.clamp(0, max as i32) as u8,
        r.clamp(0, max as i32) as u8,
    )
}

/// Whether a level is representable for the given mixer range.
pub fn level_valid(level: u8, max: u8) -> bool {
    max > 0 && level <= max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_bounds() {
        assert_eq!(level_to_percent(0, 255), 0);
        assert_eq!(level_to_percent(255, 255), 100);
        assert_eq!(level_to_percent(128, 255), 50); // 50.19 -> 50
        assert_eq!(level_to_percent(200, 0), 0); // degenerate range
        assert_eq!(percent_to_level(100, 255), 255);
        assert_eq!(percent_to_level(0, 255), 0);
        assert_eq!(percent_to_level(50, 255), 127); // 127.5 -> 127
        assert_eq!(percent_to_level(101, 255), 255); // clamped
    }

    #[test]
    fn percent_level_inverse_is_lossy_in_the_safe_direction() {
        // level_to_percent floors, so re-expanding never exceeds the source.
        for lvl in [0u8, 1, 2, 7, 64, 100, 127, 200, 254, 255] {
            let pct = level_to_percent(lvl, 255);
            assert!(percent_to_level(pct, 255) <= lvl);
            assert!(pct <= 100);
        }
    }

    #[test]
    fn balance_center_keeps_both_channels() {
        assert_eq!(balance_levels(200, 255, 0), (200, 200));
        assert_eq!(balance_levels(0, 255, 0), (0, 0));
    }

    #[test]
    fn balance_hard_pan_zeroes_one_channel() {
        // +100 is full RIGHT: the left channel goes silent, right holds.
        assert_eq!(balance_levels(200, 255, 100), (0, 200));
        // -100 is full LEFT: left holds, right goes silent.
        assert_eq!(balance_levels(200, 255, -100), (200, 0));
        // Out-of-range balance is clamped, not wrapped.
        assert_eq!(balance_levels(200, 255, 127), (0, 200));
        assert_eq!(balance_levels(200, 255, -127), (200, 0));
    }

    #[test]
    fn balance_partial_attenuates_opposite_channel() {
        // +50: left halved, right holds the master level (never boosted).
        assert_eq!(balance_levels(200, 255, 50), (100, 200));
        assert_eq!(balance_levels(100, 255, -25), (100, 75));
        assert_eq!(balance_levels(100, 255, 25), (75, 100));
    }

    #[test]
    fn level_validity() {
        assert!(level_valid(0, 255));
        assert!(level_valid(255, 255));
        assert!(!level_valid(u8::MAX, 254)); // above the range
        assert!(!level_valid(5, 0));
    }
}
