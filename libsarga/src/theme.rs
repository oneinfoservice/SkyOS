/// Chrome colors with no `Theme` field equivalent, consumed directly by draw
/// code. Palette values live inline in the `Theme` constructors (`dark()`/
/// `light()`) — the CI `dead-code` job rejects any constant in this module
/// with zero references outside theme.rs, so a color can never sit here
/// unused while a hardcoded duplicate value lives elsewhere (the
/// WIN_CLOSE_HOVER history: it sat unreferenced for months, then a
/// hardcoded copy appeared in window.rs before the constant was wired in).
pub mod colors {
    // Window close button — red in both themes, no theme-field equivalent.
    pub const WIN_CLOSE_HOVER: u32 = 0xFFE81123; // Close button hover
    pub const WIN_CLOSE_PRESSED: u32 = 0xFFB71C1C; // Close button held down
}

pub struct Theme {
    pub bg_primary: u32,
    pub bg_surface: u32,
    pub bg_elevated: u32,
    pub accent: u32,
    pub accent_light: u32,
    pub accent_dark: u32,
    pub text: u32,
    pub text_secondary: u32,
    pub text_disabled: u32,
    /// Text drawn on an accent/hover (indigo) fill. The accent and hover
    /// colors are theme-invariant (same indigo in both themes), so any text
    /// on them must stay white in both themes too — `theme.text` flips to
    /// black in the light theme and would vanish on the indigo surfaces
    /// (start button, selected menu rows, hovered buttons, notification
    /// rows). 5.13:1 on accent in both themes (WCAG AA).
    pub on_accent: u32,
    pub border: u32,
    pub hover: u32,
    pub pressed: u32,
    pub error: u32,
    pub success: u32,
    pub warning: u32,
    pub separator: u32,
    pub shadow: u32,
    pub font_size: u32,
    pub border_radius: u32,
    pub padding: u32,
    pub spacing: u32,
}

impl Theme {
    pub fn light() -> Self {
        Theme {
            bg_primary: 0xFFF5F5F5,
            bg_surface: 0xFFFFFFFF,
            bg_elevated: 0xFFEEEEEE,
            accent: 0xFF3D5AFE,
            accent_light: 0xFF1A8FE8,
            accent_dark: 0xFF005A9E,
            text: 0xFF000000,
            text_secondary: 0xFF555555,
            // 0xFF757575 (gray 600) instead of 0xFFAAAAAA: the old value is
            // 2.3:1 on white — a hint/placeholder (start-menu search,
            // "Recent:" label, "Coming soon") must be readable on the
            // white light surfaces. 4.6:1 on white; still clearly
            // "disabled" vs text_secondary.
            text_disabled: 0xFF757575,
            // Darker success green (green-700 family instead of 500): the
            // old 0x4CAF50 is only 2.4:1 on the light elevated surface the
            // toggles sit on. 0xFF2B6A30 is 5.6:1 there.
            success: 0xFF2B6A30,
            on_accent: 0xFFFFFFFF, // Text on accent/hover fills (5.13:1 in both themes)
            border: 0xFFCCCCCC,
            hover: 0xFF3D5AFE,
            pressed: 0xFFE0E0E0,
            error: 0xFFD32F2F,
            warning: 0xFFFFC107,
            separator: 0xFFCCCCCC,
            shadow: 0x40000000,
            font_size: 14,
            border_radius: 12,
            padding: 10,
            spacing: 6,
        }
    }

    pub fn dark() -> Self {
        Theme {
            bg_primary: 0xFF0F0F1A,     // Darker navy background
            bg_surface: 0xFF1A1A2E,     // Card/surface
            bg_elevated: 0xFF252540,    // Elevated surface
            accent: 0xFF3D5AFE,         // Indigo accent
            accent_light: 0xFF1A8FE8,   // Lighter accent
            accent_dark: 0xFF005A9E,    // Darker accent
            text: 0xFFFFFFFF,           // White text
            text_secondary: 0xFFB0B0B0, // Gray text
            text_disabled: 0xFF606060,  // Disabled text
            on_accent: 0xFFFFFFFF,      // Text on accent/hover fills (5.13:1 in both themes)
            border: 0xFF30304D,         // Border color
            hover: 0xFF3D5AFE,          // Hover state (indigo, same as accent)
            pressed: 0xFF1A1A30,        // Pressed state
            error: 0xFFD32F2F,          // Error red
            success: 0xFF4CAF50,        // Success green
            warning: 0xFFFFC107,        // Warning yellow
            separator: 0xFF3A3A5C,      // Separator line
            shadow: 0x80000000,         // Semi-transparent black
            font_size: 14,
            border_radius: 12, // More rounded corners
            padding: 10,
            spacing: 6,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn channel(c: u32, shift: u32) -> f64 {
        ((c >> shift) & 0xFF) as f64 / 255.0
    }

    fn srgb_linear(v: f64) -> f64 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(c: u32) -> f64 {
        let r = srgb_linear(channel(c, 16));
        let g = srgb_linear(channel(c, 8));
        let b = srgb_linear(channel(c, 0));
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let la = luminance(a);
        let lb = luminance(b);
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn test_on_accent_contrast_aa_both_themes() {
        // Documented 5.13:1 in both themes (comment on Theme::on_accent).
        for t in [Theme::dark(), Theme::light()] {
            assert!(contrast(t.on_accent, t.accent) >= 4.5);
        }
    }

    #[test]
    fn test_hover_matches_accent_both_themes() {
        // hover is documented as the same indigo as accent in both themes.
        for t in [Theme::dark(), Theme::light()] {
            assert_eq!(t.hover, t.accent);
        }
    }

    #[test]
    fn test_light_theme_text_contrast() {
        let t = Theme::light();
        assert!(contrast(t.text, t.bg_surface) >= 4.5);
        assert!(contrast(t.text, t.bg_primary) >= 4.5);
    }

    #[test]
    fn test_dark_theme_text_contrast() {
        let t = Theme::dark();
        assert!(contrast(t.text, t.bg_surface) >= 4.5);
        assert!(contrast(t.text, t.bg_elevated) >= 4.5);
    }

    #[test]
    fn test_light_disabled_text_aa() {
        // Documented 4.6:1 on white (comment on text_disabled in light()).
        assert!(contrast(Theme::light().text_disabled, 0xFFFFFFFF) >= 4.5);
    }

    #[test]
    fn test_light_success_on_elevated_aa() {
        // Documented 5.6:1 on the light elevated surface.
        assert!(contrast(Theme::light().success, Theme::light().bg_elevated) >= 4.5);
    }

    #[test]
    fn test_layout_metrics_consistent_across_themes() {
        let d = Theme::dark();
        let l = Theme::light();
        assert_eq!(d.font_size, l.font_size);
        assert_eq!(d.border_radius, l.border_radius);
        assert_eq!(d.padding, l.padding);
        assert_eq!(d.spacing, l.spacing);
    }

    #[test]
    fn test_close_button_colors_are_red() {
        for c in [colors::WIN_CLOSE_HOVER, colors::WIN_CLOSE_PRESSED] {
            let r = (c >> 16) & 0xFF;
            let g = (c >> 8) & 0xFF;
            let b = c & 0xFF;
            assert!(r > g && r > b, "close color {:#x} must be red-family", c);
        }
        assert_ne!(colors::WIN_CLOSE_HOVER, colors::WIN_CLOSE_PRESSED);
    }
}
