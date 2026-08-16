//! Input byte pure logic — ASCII control folding and classification.
//!
//! The kernel key producer folds Ctrl+letter into a single ASCII control
//! byte (`Ctrl+W` -> 0x17); `fold_ctrl`/`unfold_ctrl` implement that
//! contract, and `input::KeyEvent::from_byte` consumes them. Host-testable:
//! pure byte math, so the `#[cfg(test)]` module runs under host
//! `cargo test` (the same cfg(not(test)) treatment as libsarga).

/// Fold an ASCII letter into its Ctrl control byte (Ctrl+A -> 0x01,
/// Ctrl+Z -> 0x1A). Non-letters are not foldable.
pub fn fold_ctrl(key: u8) -> Option<u8> {
    match key {
        b'a'..=b'z' | b'A'..=b'Z' => Some(key & 0x1F),
        _ => None,
    }
}

/// Unfold a control byte 0x01..=0x1A back to the lowercase letter
/// (0x01 -> 'a', 0x1A -> 'z'). Mirrors `KeyEvent::from_byte`'s Ctrl+letter
/// arm exactly (`b'a' - 1 + b`).
pub fn unfold_ctrl(b: u8) -> Option<u8> {
    if (1..=26).contains(&b) {
        Some(b'a' - 1 + b)
    } else {
        None
    }
}

/// Whether the byte is a printable ASCII character (0x20..=0x7E).
pub fn is_printable(b: u8) -> bool {
    (0x20..=0x7E).contains(&b)
}

/// Whether the byte is an ASCII control character (0x00..=0x1F or 0x7F).
pub fn is_ascii_control(b: u8) -> bool {
    b < 0x20 || b == 0x7F
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_letters() {
        assert_eq!(fold_ctrl(b'a'), Some(0x01));
        assert_eq!(fold_ctrl(b'z'), Some(0x1A));
        assert_eq!(fold_ctrl(b'w'), Some(0x17)); // the documented Ctrl+W fold
        assert_eq!(fold_ctrl(b'A'), Some(0x01)); // case-insensitive fold
        assert_eq!(fold_ctrl(b'Z'), Some(0x1A));
    }

    #[test]
    fn fold_rejects_non_letters() {
        assert_eq!(fold_ctrl(b'1'), None);
        assert_eq!(fold_ctrl(b' '), None);
        assert_eq!(fold_ctrl(0x7F), None);
        assert_eq!(fold_ctrl(0), None);
    }

    #[test]
    fn unfold_round_trips_letters() {
        for k in b'a'..=b'z' {
            let c = fold_ctrl(k).unwrap();
            assert_eq!(unfold_ctrl(c), Some(k));
        }
        // The producer's fold is the same one; decoding is the inverse.
        assert_eq!(unfold_ctrl(0x01), Some(b'a'));
        assert_eq!(unfold_ctrl(0x1A), Some(b'z'));
    }

    #[test]
    fn unfold_rejects_out_of_range() {
        assert_eq!(unfold_ctrl(0), None);
        assert_eq!(unfold_ctrl(0x1B), None); // ESC is special-cased upstream
        assert_eq!(unfold_ctrl(0x7F), None);
    }

    #[test]
    fn printable_classification() {
        assert!(is_printable(b'a'));
        assert!(is_printable(b' '));
        assert!(is_printable(0x7E));
        assert!(!is_printable(0x1B)); // ESC
        assert!(!is_printable(0x7F)); // DEL
        assert!(!is_printable(0));
    }

    #[test]
    fn control_classification() {
        // Control is exactly 0x00..=0x1F plus 0x7F; printable is exactly
        // 0x20..=0x7E. The two are disjoint (0x80..=0xFF is neither — that
        // range is not claimed by either classifier).
        for b in 0u8..=255 {
            assert!(!(is_ascii_control(b) && is_printable(b)));
            assert_eq!(is_ascii_control(b), b < 0x20 || b == 0x7F);
            assert_eq!(is_printable(b), (0x20..=0x7E).contains(&b));
        }
        assert!(is_ascii_control(0x1F));
        assert!(is_ascii_control(0x7F));
        assert!(!is_ascii_control(0x20));
        assert!(!is_ascii_control(0x80)); // high bytes: neither
        assert!(!is_printable(0x80));
    }
}
