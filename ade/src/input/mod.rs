//! Input pipeline — key decode, modifier state, and the routing table.
//!
//! The input producer (`libsarga::gui::Window::get_key`) delivers one byte
//! per key event: Ctrl+letter is folded into an ASCII control code
//! (`Ctrl+W` → 0x17), printable keys arrive as ASCII, and non-ASCII keys
//! (arrows, F-keys, Enter, Esc) arrive as PC scan-code-set-1 values.
//!
//! `KeyEvent::from_byte` recovers the modifier state that byte stream
//! hides, and `resolve`/`is_desktop_shortcut` route events through the
//! keymap table — replacing the historical `DESKTOP_KEYS` magic list and
//! the Ctrl+letter pile in `Desktop::handle_key`.
//!
//! Modifier state beyond Ctrl (Alt, Shift) cannot be carried in that one
//! byte, so `from_byte` always produces `alt: false, shift: false` — the
//! hardware delivery of Alt/Shift is a kernel question (the Phase C gate,
//! see docs/session-lifecycle.md). The pipeline is still modifier-aware:
//! `KeyEvent::new` constructs full events, the routing table matches on all
//! three modifier bits, and the session-end chord Ctrl+Alt+Backspace is
//! expressible and testable today (synthetic events) even though the byte
//! stream cannot deliver it yet. Esc on an empty desktop is the
//! byte-deliverable session-end path: 0x1B is the one distinct control byte
//! the stream carries, handled in the a11y Esc arm (`handle_a11y_key`
//! consumes Esc before this table's routing runs — see
//! `Desktop::handle_a11y_key`), not as a `Quit` binding.

/// Named key codes. ASCII for letters/controls; PC scan-code-set-1 values
/// for keys with no ASCII equivalent.
pub(crate) mod keys {
    // ASCII
    pub const KEY_ESC: u8 = 0x1B;
    pub const KEY_ENTER: u8 = 0x0D; // CR
    pub const KEY_LF: u8 = 0x0A; // LF — the driver also sends this for Enter
    pub const KEY_TAB: u8 = 0x09;
    pub const KEY_BACKSPACE: u8 = 0x7F; // DEL
    pub const KEY_BACKSPACE_ALT: u8 = 0x08; // Ctrl+H / legacy backspace
    pub const KEY_Q: u8 = b'q';
    pub const KEY_X: u8 = b'X';

    // PC scan-code set 1 (delivered by the producer for non-ASCII keys).
    pub const SCAN_ESC: u8 = 1; // also Ctrl+A — see `from_byte` note
    pub const SCAN_ENTER: u8 = 28;
    pub const SCAN_UP: u8 = 72;
    pub const SCAN_LEFT: u8 = 75;
    pub const SCAN_RIGHT: u8 = 77;
    pub const SCAN_DOWN: u8 = 80;
    /// F11. The byte 0x57 is indistinguishable from ASCII 'W' in a byte
    /// stream; the legacy code toggled the debug overlay on both.
    pub const SCAN_F11: u8 = 0x57;
}

/// A structured key event with modifier state.
///
/// `from_byte` can only recover Ctrl (the producer folds it into the byte);
/// `alt`/`shift` come from `KeyEvent::new` (and from `from_raw` once the
/// kernel delivers the packed modifier bits — Phase C, Design A) and stay
/// false for byte-decoded events until then — the historical "Ctrl+Shift+S" /
/// "Ctrl+Shift+X" bindings receive plain `Ctrl+S`/`Ctrl+X` (a pre-existing
/// limitation, preserved deliberately).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyEvent {
    /// Canonical key code: ASCII value, or a `keys::SCAN_*` constant.
    pub code: u8,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyEvent {
    /// Construct a full key event. The canonical constructor for chords
    /// (Ctrl+Alt+Backspace, etc.) that the byte stream cannot express;
    /// used by tests and by future structured input producers.
    pub fn new(code: u8, ctrl: bool, alt: bool, shift: bool) -> KeyEvent {
        KeyEvent {
            code,
            ctrl,
            alt,
            shift,
        }
    }

    /// Decode a raw input byte into a structured key event.
    ///
    /// Control bytes `0x01..=0x1A` are Ctrl+letter (the producer folds the
    /// modifier in). `0x08`/`0x0A`/`0x0D` are special-cased to Backspace /
    /// Enter because the legacy routing treated them that way, not as
    /// Ctrl+H / Ctrl+J / Ctrl+M. Scan-code `1` (Esc) is deliberately NOT
    /// decoded here: it collides with Ctrl+A, and the a11y pre-handler
    /// consumes it before `handle_key` ever runs (as in the legacy flow).
    pub fn from_byte(b: u8) -> KeyEvent {
        let (code, ctrl) = match b {
            keys::KEY_ESC => (keys::KEY_ESC, false),
            keys::KEY_ENTER | keys::KEY_LF | keys::SCAN_ENTER => (keys::KEY_ENTER, false),
            keys::KEY_TAB => (keys::KEY_TAB, false),
            keys::KEY_BACKSPACE | keys::KEY_BACKSPACE_ALT => (keys::KEY_BACKSPACE, false),
            // Ctrl+letter: the fold/unfold contract lives in sys::input so
            // the host tests pin the exact decode (1..=26 -> 'a'..='z').
            _ => match crate::sys::input::unfold_ctrl(b) {
                Some(letter) => (letter, true),
                None => (b, false),
            },
        };
        KeyEvent::new(code, ctrl, false, false)
    }

    /// Decode a kernel key value: low byte = the character (decoded exactly
    /// as `from_byte`), bits 8..10 = alt/ctrl/shift held. This is the
    /// userspace half of the Phase C packed-key contract
    /// (docs/kernel-gui-modifier-delivery.md, Design A): the kernel packs
    /// `char | (alt<<8) | (ctrl<<9) | (shift<<10) | (super<<11)` and the
    /// syscall already returns it losslessly (u64). Bit 11 (Super) is
    /// deliberately ignored — the routing table has no Super chords.
    ///
    /// When the high bits are zero this is exactly `from_byte`, so the
    /// change is additive and today's byte-stream behavior is preserved
    /// until the kernel actually sends modifier bits.
    pub fn from_raw(raw: u16) -> KeyEvent {
        let mut ev = Self::from_byte((raw & 0xFF) as u8);
        if raw & (1 << 8) != 0 {
            ev.alt = true;
        }
        if raw & (1 << 9) != 0 {
            ev.ctrl = true;
        }
        if raw & (1 << 10) != 0 {
            ev.shift = true;
        }
        ev
    }

    /// The character this event types, when it is an unmodified printable
    /// key. Mirrors the legacy printable range `0x20..=0x7E`. Shift is
    /// rejected too: the producer folds shift into the character (Shift+a
    /// arrives as 'A'), so a synthetic shift-modified event is not text.
    pub fn text(&self) -> Option<char> {
        if self.ctrl || self.alt || self.shift || !crate::sys::input::is_printable(self.code) {
            return None;
        }
        Some(self.code as char)
    }
}

/// Desktop-level action a key resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyAction {
    ToggleDebugOverlay,
    Enter,
    Backspace,
    /// Tab — a11y focus next (or menu category / switcher next in context).
    FocusNext,
    Quit,
    CloseFocused,
    CycleTiling,
    CycleWindow,
    ToggleAot,
    ClipboardPanel,
    DemoNotification,
    DismissNotification,
    ClearNotifications,
    ClearTerminal,
    OpenSettings,
    OpenTaskManager,
}

/// One row of the routing table.
#[derive(Clone, Copy)]
pub(crate) struct Binding {
    /// Key code matched against the decoded `KeyEvent.code`.
    pub code: u8,
    /// Modifier mask; `resolve` matches all three bits exactly.
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub action: KeyAction,
    /// When true, the desktop keeps handling this key even while a terminal
    /// window is focused (this replaces the old `DESKTOP_KEYS` list). When
    /// false, a focused terminal receives the key instead.
    pub desktop: bool,
}

/// The keymap routing table — the single source of truth for what each key
/// does and whether it overrides a focused terminal.
pub(crate) const BINDINGS: &[Binding] = &[
    // Global grabs (contextual; see Desktop::handle_key for precedence).
    Binding {
        code: keys::KEY_X,
        ctrl: false,
        alt: false,
        shift: false,
        action: KeyAction::ToggleDebugOverlay,
        desktop: false,
    },
    Binding {
        code: keys::SCAN_F11,
        ctrl: false,
        alt: false,
        shift: false,
        action: KeyAction::ToggleDebugOverlay,
        desktop: false,
    },
    // NOTE: there is deliberately NO Escape binding row. `handle_a11y_key`
    // consumes Esc (byte 0x1B and SCAN_ESC) before `resolve` can ever see
    // it, so a row here would be unreachable dead weight — all Esc behavior
    // (dismiss overlays, exit fullscreen, terminal 0x1B forward, and the
    // empty-desktop logout) lives in that arm. The `Escape` action variant
    // is removed for the same reason: nothing can construct it, so the
    // table holds only reachable rows and the dispatcher only reachable
    // actions.
    // Navigation / editing.
    Binding {
        code: keys::KEY_TAB,
        ctrl: false,
        alt: false,
        shift: false,
        action: KeyAction::FocusNext,
        desktop: false,
    },
    Binding {
        code: keys::KEY_ENTER,
        ctrl: false,
        alt: false,
        shift: false,
        action: KeyAction::Enter,
        desktop: false,
    },
    Binding {
        code: keys::KEY_BACKSPACE,
        ctrl: false,
        alt: false,
        shift: false,
        action: KeyAction::Backspace,
        desktop: false,
    },
    // Quit: the session-end chord is Ctrl+Alt+Backspace, a desktop grab so it
    // works even while a terminal is focused (like a real logout chord). Plain
    // Backspace matches the row above (ctrl=false) and never quits. Ctrl+Q is
    // deliberately UNBOUND — the old Ctrl+Q/plain-'q' gates are gone. Only
    // this chord ends the session via the table; Esc on an empty desktop is
    // the second (byte-deliverable) session-end path, handled contextually in
    // the a11y Esc arm — NOT a Quit binding (and not a binding row at all,
    // see the NOTE above), so `quit_count` stays exactly 1 (pinned in
    // testing/input.rs).
    Binding {
        code: keys::KEY_BACKSPACE,
        ctrl: true,
        alt: true,
        shift: false,
        action: KeyAction::Quit,
        desktop: true,
    },
    // Ctrl+letter desktop shortcuts — these override a focused terminal.
    Binding {
        code: b'w',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::CloseFocused,
        desktop: true,
    },
    Binding {
        code: b't',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::CycleTiling,
        desktop: true,
    },
    Binding {
        code: b'e',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::CycleWindow,
        desktop: true,
    },
    Binding {
        code: b'a',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::ToggleAot,
        desktop: true,
    },
    Binding {
        code: b'b',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::ClipboardPanel,
        desktop: true,
    },
    Binding {
        code: b'n',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::DemoNotification,
        desktop: true,
    },
    Binding {
        code: b'd',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::DismissNotification,
        desktop: true,
    },
    Binding {
        code: b's',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::OpenSettings,
        desktop: true,
    },
    Binding {
        code: b'x',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::OpenTaskManager,
        desktop: true,
    },
    // Ctrl+letter keys that stay with the terminal when one is focused.
    Binding {
        code: b'c',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::ClearNotifications,
        desktop: false,
    },
    Binding {
        code: b'l',
        ctrl: true,
        alt: false,
        shift: false,
        action: KeyAction::ClearTerminal,
        desktop: false,
    },
];

/// Resolve a key event to its desktop action (exact table match on the key
/// code and all three modifier bits; plain text is detected via
/// `KeyEvent::text`).
pub(crate) fn resolve(ev: KeyEvent) -> Option<KeyAction> {
    for b in BINDINGS {
        if b.code == ev.code && b.ctrl == ev.ctrl && b.alt == ev.alt && b.shift == ev.shift {
            return Some(b.action);
        }
    }
    None
}

/// True when this key is a desktop grab — i.e. it must NOT be forwarded to
/// a focused terminal. This is the routing-table replacement for the old
/// `DESKTOP_KEYS: [1, 2, 4, 5, 14, 17, 19, 20, 23, 24]` magic list: exactly
/// those ten Ctrl+letter bindings (now nine — Ctrl+Q is unbound) have
/// `desktop: true`, plus the Ctrl+Alt+Backspace logout chord.
pub(crate) fn is_desktop_shortcut(ev: KeyEvent) -> bool {
    BINDINGS.iter().any(|b| {
        b.desktop
            && b.code == ev.code
            && b.ctrl == ev.ctrl
            && b.alt == ev.alt
            && b.shift == ev.shift
    })
}

/// A host-verifiable summary of the routing table, derived from `resolve`
/// (the observable routing behavior) rather than from the `BINDINGS` literal
/// directly — so a table edit that makes a binding unreachable, or that
/// re-adds a session-end binding, shows up here before any QEMU run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingDump {
    /// Number of distinct (code, ctrl, alt, shift) events that resolve to a
    /// desktop action — i.e. the number of reachable binding rows.
    pub count: u32,
    /// Number of distinct events that resolve to Quit — must stay exactly 1
    /// (the Ctrl+Alt+Backspace chord). Esc-on-empty-desktop is the second
    /// session-end path but a contextual a11y-arm behavior, not a Quit
    /// binding, so it does not count here. A second session-end *binding*
    /// trips the selftest with a sharper message than the raw count.
    pub quit_count: u32,
    /// The Ctrl+Alt+Backspace session-end chord resolves to Quit.
    pub has_quit_chord: bool,
    /// Ctrl+Q must be unbound — the old session-end gates are gone and only
    /// the chord may end the session.
    pub ctrl_q_unbound: bool,
    /// Number of distinct (code, ctrl, alt, shift) events that are desktop
    /// grabs — i.e. that `is_desktop_shortcut` returns true for over the
    /// whole event space. This is the routing-table replacement for the old
    /// `DESKTOP_KEYS` magic list, counted through the same predicate
    /// `Desktop::handle_key` uses for terminal routing, so a table edit
    /// that turns a terminal key into a desktop grab shows up here.
    pub desktop_grabs: u32,
}

/// Dump the routing table by resolving every canonical key event (all 256
/// codes × all 8 modifier combos) through `resolve` and
/// `is_desktop_shortcut`. Pinned by `testing/input.rs::test_keymap` so a
/// keymap edit that accidentally re-adds a session-end binding (e.g.
/// Ctrl+Q → Quit) or turns a terminal key into a desktop grab fails the
/// selftest first in the suite, before any QEMU run. Cost is trivial:
/// 2,048 `resolve` + 2,048 `is_desktop_shortcut` calls against a 17-row
/// table.
pub fn dump_bindings() -> BindingDump {
    let mut count: u32 = 0;
    let mut quit_count: u32 = 0;
    let mut desktop_grabs: u32 = 0;
    for code in 0..=255u8 {
        for ctrl in [false, true] {
            for alt in [false, true] {
                for shift in [false, true] {
                    let ev = KeyEvent::new(code, ctrl, alt, shift);
                    let action = resolve(ev);
                    if action.is_some() {
                        count += 1;
                    }
                    if action == Some(KeyAction::Quit) {
                        quit_count += 1;
                    }
                    if is_desktop_shortcut(ev) {
                        desktop_grabs += 1;
                    }
                }
            }
        }
    }
    BindingDump {
        count,
        quit_count,
        has_quit_chord: resolve(KeyEvent::new(keys::KEY_BACKSPACE, true, true, false))
            == Some(KeyAction::Quit),
        ctrl_q_unbound: resolve(KeyEvent::new(keys::KEY_Q, true, false, false)).is_none(),
        desktop_grabs,
    }
}
