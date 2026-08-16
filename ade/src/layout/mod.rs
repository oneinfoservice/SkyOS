//! Desktop shell layout — the single source of truth for geometry constants
//! and hit-testing tables shared by `desktop.rs`, `window.rs`,
//! `start_menu.rs`, and `taskbar.rs`.
//!
//! Everything here was previously a literal or a per-file constant, which
//! drifted: the taskbar drew buttons on a 125px pitch but clicks tested 120px;
//! the titlebar hit-test used 28px for drag but 22px for the system menu; the
//! window control-button hit regions did not match the drawn buttons; and the
//! start-menu geometry was duplicated (with small deltas) across draw, click,
//! and the a11y tree. Draw and hit-testing now share the rect functions below,
//! so they cannot drift apart again.
//!
//! The unification fixes (behavior deltas, all deliberate):
//! - Taskbar buttons: 125px pitch / 120px wide everywhere (clicks were 120/115).
//! - Titlebar: 28px tall for all hit tests (drag, system menu, middle-click).
//! - Window buttons: hit regions come from the same rects as the draw
//!   (close/min only; the old maximize hit region had no drawn button).
//! - Start menu: bottom gap 4px (was 5px in click/a11y), sidebar item width
//!   122 (was 126), recent strip capped at 2 and hit exactly where drawn.
//! - Start button: 58px wide for clicks (was 60).

use crate::core::geometry::{Point, Rect};

// ---------------------------------------------------------------------------
// Taskbar
// ---------------------------------------------------------------------------

pub const TASKBAR_H: u32 = 36;

pub const TASKBAR_START_X: u32 = 5;
pub const TASKBAR_START_Y: u32 = 4;
pub const TASKBAR_START_W: u32 = 58;
pub const TASKBAR_START_H: u32 = TASKBAR_H - 8;

pub const TASKBAR_BTN_X0: u32 = 75;
pub const TASKBAR_BTN_PITCH: u32 = 125;
pub const TASKBAR_BTN_W: u32 = 120;
pub const TASKBAR_BTN_H: u32 = TASKBAR_H - 8;
pub const TASKBAR_MAX_BTNS: usize = 8;
pub const TASKBAR_OVERFLOW_W: u32 = 30;
pub const TASKBAR_TITLE_MAX: usize = 14;

pub const TRAY_ENTRY_W: u32 = 28;
pub const TRAY_ENTRY_H: u32 = 22;
pub const TRAY_PAD: u32 = 20;
pub const TASKBAR_CLOCK_W: u32 = 80;
pub const TASKBAR_PANEL_PAD: u32 = 8;
pub const TRAY_CLOCK_GAP: u32 = 10; // gap between the entries and the clock

/// Left edge of the taskbar window button `i` (draw + hit-test shared).
pub fn taskbar_btn_x(i: usize) -> u32 {
    TASKBAR_BTN_X0 + i as u32 * TASKBAR_BTN_PITCH
}

/// Start button rect at a given taskbar top `ty`.
pub fn start_btn_rect(ty: u32) -> Rect {
    Rect::new(
        TASKBAR_START_X as i32,
        ty as i32 + TASKBAR_START_Y as i32,
        TASKBAR_START_W,
        TASKBAR_START_H,
    )
}

/// Taskbar window button `i` at a given taskbar top `ty`.
pub fn taskbar_btn_rect(i: usize, ty: u32) -> Rect {
    Rect::new(
        taskbar_btn_x(i) as i32,
        ty as i32 + TASKBAR_START_Y as i32,
        TASKBAR_BTN_W,
        TASKBAR_BTN_H,
    )
}

/// Left edge of the overflow "..." button (first button past the cap).
pub fn taskbar_overflow_x() -> u32 {
    taskbar_btn_x(TASKBAR_MAX_BTNS)
}

/// Width of just the tray-entries strip (entries + their pad) — the left
/// portion of the panel; the clock sits to its right.
pub fn tray_entries_w(tray_len: u32) -> u32 {
    tray_len * TRAY_ENTRY_W + TRAY_PAD
}

/// Full tray panel rect (tray entries + clock) at a given taskbar top `ty`
/// and screen width. Panel width is computed once here; the taskbar draw
/// and `Desktop::hover_target` share this whole rect, so the panel
/// background, its entries, and its clock cannot drift apart again.
pub fn tray_panel_rect(ty: u32, screen_w: u32, tray_len: u32) -> Rect {
    let panel_w = tray_entries_w(tray_len) + TASKBAR_CLOCK_W + TRAY_CLOCK_GAP;
    Rect::new(
        (screen_w - panel_w - TASKBAR_PANEL_PAD) as i32,
        ty as i32 + 4,
        panel_w,
        TASKBAR_H - 8,
    )
}

/// Tray entry `i` hit/draw rect at a given taskbar top `ty`, derived from
/// the panel rect so the entry always sits exactly where the panel
/// background is drawn.
pub fn tray_entry_rect(i: usize, ty: u32, screen_w: u32, tray_len: u32) -> Rect {
    let panel = tray_panel_rect(ty, screen_w, tray_len);
    Rect::new(
        panel.x + 8 + (i as u32 * TRAY_ENTRY_W) as i32,
        panel.y + 2,
        TRAY_ENTRY_H,
        TRAY_ENTRY_H,
    )
}

// ---------------------------------------------------------------------------
// Window titlebar / chrome
// ---------------------------------------------------------------------------

pub const TITLE_H: i32 = 28;
pub const TITLE_PAD_X: u32 = 12;
pub const TITLE_TEXT_Y: u32 = 7;
pub const TITLE_SEP_Y: u32 = 29; // separator line below the titlebar
pub const TITLE_AOT_OFFSET: u32 = 82; // "[A]" label, measured from the right edge

// Window control buttons (close rightmost, minimize to its left).
pub const BTN_W: u32 = 22;
pub const BTN_H: u32 = 18;
pub const BTN_Y: u32 = 6;
pub const BTN_RIGHT_PAD: u32 = 6; // close button's right edge from the window edge
pub const BTN_GAP: u32 = 4;

// Window content metrics.
pub const CONTENT_PAD_X: u32 = 8;
pub const LINE_H: u32 = 14;
pub const CHAR_W: u32 = 8;
pub const CONTENT_BOTTOM_PAD: u32 = 6;
pub const LINE_TRUNCATE_MAX: usize = 55;

/// Full titlebar (drag + system menu + middle-click share this height).
pub fn titlebar_rect(x: i32, y: i32, w: u32) -> Rect {
    Rect::new(x, y, w, TITLE_H as u32)
}

/// Close button — the same rect the draw uses.
pub fn close_btn_rect(x: i32, y: i32, w: u32) -> Rect {
    Rect::new(
        x + w as i32 - (BTN_RIGHT_PAD + BTN_W) as i32,
        y + BTN_Y as i32,
        BTN_W,
        BTN_H,
    )
}

/// Minimize button — the same rect the draw uses.
pub fn min_btn_rect(x: i32, y: i32, w: u32) -> Rect {
    Rect::new(
        x + w as i32 - (BTN_RIGHT_PAD + 2 * BTN_W + BTN_GAP) as i32,
        y + BTN_Y as i32,
        BTN_W,
        BTN_H,
    )
}

// ---------------------------------------------------------------------------
// Start menu
// ---------------------------------------------------------------------------

pub const MENU_X: u32 = 4;
pub const MENU_W: u32 = 480;
pub const MENU_H: u32 = 460;
pub const MENU_BOTTOM_GAP: u32 = 4;

pub const MENU_SEARCH_PAD: u32 = 8;
pub const MENU_SEARCH_H: u32 = 36;
pub const MENU_SIDEBAR_W: u32 = 130;
pub const MENU_SIDEBAR_ITEM_H: u32 = 24;
pub const MENU_SIDEBAR_PITCH: u32 = 28;
pub const MENU_ITEM_H: u32 = 32;
pub const MENU_LIST_BOTTOM_RESERVE: u32 = 44;
pub const MENU_BOTTOM_STRIP_H: u32 = 36; // strip top edge, from the menu bottom
pub const MENU_BOTTOM_STRIP_HT: u32 = 34; // strip height (36 - 2px breathing room)
pub const MENU_RECENT_RIGHT_RESERVE: u32 = 210; // recent tiles stop this far from the right

pub const MENU_RECENT_X: u32 = 72;
pub const MENU_RECENT_W: u32 = 80;
pub const MENU_RECENT_H: u32 = 26;
pub const MENU_RECENT_PITCH: u32 = 84;
pub const MENU_RECENT_MAX: usize = 2;
pub const MENU_POWER_X: u32 = 202; // power cluster, measured from the right edge
pub const MENU_POWER_W: u32 = 58;
pub const MENU_POWER_H: u32 = 26;
pub const MENU_POWER_PITCH: u32 = 64;

pub const MENU_APP_NAME_MAX: usize = 26;
pub const MENU_RECENT_NAME_MAX: usize = 10;

/// Outer start-menu rect at a given taskbar top `ty` (fully open).
pub fn menu_rect(ty: u32) -> Rect {
    Rect::new(
        MENU_X as i32,
        ty.saturating_sub(MENU_H + MENU_BOTTOM_GAP) as i32,
        MENU_W,
        MENU_H,
    )
}

/// Search box inside a menu rect.
pub fn menu_search_rect(m: Rect) -> Rect {
    Rect::new(
        m.x + MENU_SEARCH_PAD as i32,
        m.y + MENU_SEARCH_PAD as i32,
        MENU_W - 2 * MENU_SEARCH_PAD,
        MENU_SEARCH_H,
    )
}

/// Sidebar (categories) inside a menu rect.
pub fn menu_sidebar_rect(m: Rect) -> Rect {
    let sy = m.y + MENU_SEARCH_PAD as i32 + MENU_SEARCH_H as i32 + 6;
    Rect::new(m.x + 4, sy, MENU_SIDEBAR_W, MENU_H - (sy - m.y) as u32 - 8)
}

/// Category row `i` inside the sidebar.
pub fn menu_category_rect(m: Rect, i: usize) -> Rect {
    let s = menu_sidebar_rect(m);
    Rect::new(
        s.x + 4,
        s.y + 4 + (i as u32 * MENU_SIDEBAR_PITCH) as i32,
        MENU_SIDEBAR_W - 8,
        MENU_SIDEBAR_ITEM_H,
    )
}

/// App list area inside a menu rect.
pub fn menu_list_rect(m: Rect) -> Rect {
    let sy = m.y + MENU_SEARCH_PAD as i32 + MENU_SEARCH_H as i32 + 6;
    let lx = m.x + 4 + MENU_SIDEBAR_W as i32 + 4;
    let lw = MENU_W - 4 - (lx - m.x) as u32;
    let lh = MENU_H - (sy - m.y) as u32 - MENU_LIST_BOTTOM_RESERVE;
    Rect::new(lx, sy, lw, lh)
}

/// App row `i` (visible window starts at `start`) inside the list.
pub fn menu_item_rect(m: Rect, i: usize, start: usize) -> Rect {
    let l = menu_list_rect(m);
    Rect::new(
        l.x,
        l.y + 2 + ((i - start) as u32 * MENU_ITEM_H) as i32,
        l.w,
        MENU_ITEM_H - 2,
    )
}

/// Top of the recent + power strip.
pub fn menu_bottom_y(m: Rect) -> i32 {
    m.y + MENU_H as i32 - MENU_BOTTOM_STRIP_H as i32
}

/// Left edge of the recent strip.
pub fn menu_recent_x0(m: Rect) -> i32 {
    m.x + MENU_RECENT_X as i32
}

/// A recent-app tile anchored at `rx` on the strip.
pub fn menu_recent_rect(m: Rect, rx: i32) -> Rect {
    Rect::new(rx, menu_bottom_y(m) + 4, MENU_RECENT_W, MENU_RECENT_H)
}

/// Left edge of the power cluster.
pub fn menu_power_x0(m: Rect) -> i32 {
    m.x + MENU_W as i32 - MENU_POWER_X as i32
}

/// Power button `pi` inside a menu rect — shared by the start-menu draw and
/// `Desktop::hover_target` so hover matches the drawn button.
pub fn menu_power_rect(m: Rect, pi: usize) -> Rect {
    Rect::new(
        menu_power_x0(m) + (pi as u32 * MENU_POWER_PITCH) as i32,
        menu_bottom_y(m) + 4,
        MENU_POWER_W,
        MENU_POWER_H,
    )
}

// ---------------------------------------------------------------------------
// Clipboard panel
// ---------------------------------------------------------------------------

pub const CLIPBOARD_W: u32 = 280;
pub const CLIPBOARD_MAX_H: u32 = 300;
pub const CLIPBOARD_ROW_H: u32 = 28;
pub const CLIPBOARD_HEADER_H: u32 = 30;
pub const CLIPBOARD_ROW_INNER_H: u32 = 24;

/// Clipboard history panel rect (centered, height capped). Shared by the
/// overlay draw and `Desktop::hover_target`.
pub fn clipboard_panel_rect(screen_w: u32, screen_h: u32, rows: usize) -> Rect {
    let pw = CLIPBOARD_W;
    let ph = (rows as u32 * CLIPBOARD_ROW_H + 16).min(CLIPBOARD_MAX_H);
    Rect::new(
        ((screen_w - pw) / 2) as i32,
        ((screen_h - ph) / 3) as i32,
        pw,
        ph,
    )
}

/// Clipboard history row `i` inside a panel rect.
pub fn clipboard_row_rect(panel: Rect, i: usize) -> Rect {
    Rect::new(
        panel.x + 4,
        panel.y + CLIPBOARD_HEADER_H as i32 + (i as u32 * CLIPBOARD_ROW_H) as i32,
        panel.w - 8,
        CLIPBOARD_ROW_INNER_H,
    )
}

// ---------------------------------------------------------------------------
// Modal panels (legacy settings, settings app, task manager)
// ---------------------------------------------------------------------------
// The panels' draw and `Desktop::hover_target` share these rects so hover
// always lights exactly the drawn row. The click paths (`hit_test_action`)
// keep their own looser bounds (pinned by `test_overlay_actions`) — most
// notably the settings-app toggle, whose click region spans the sidebar gap.

pub const SETTINGS_W: u32 = 320;
pub const SETTINGS_H: u32 = 200;
pub const SETTINGS_ROW_Y0: u32 = 36;
pub const SETTINGS_ROW_H: u32 = 28;
pub const SETTINGS_ROW_GAP: u32 = 32;

/// Legacy settings panel (320×200, one-third from the top).
pub fn settings_panel_rect(screen_w: u32, screen_h: u32) -> Rect {
    Rect::new(
        ((screen_w - SETTINGS_W) / 2) as i32,
        ((screen_h - SETTINGS_H) / 3) as i32,
        SETTINGS_W,
        SETTINGS_H,
    )
}

/// Legacy settings toggle row `i` (Sound, Dark Theme).
pub fn settings_row_rect(panel: Rect, i: usize) -> Rect {
    Rect::new(
        panel.x + 8,
        panel.y + SETTINGS_ROW_Y0 as i32 + (i as u32 * SETTINGS_ROW_GAP) as i32,
        panel.w - 16,
        SETTINGS_ROW_H,
    )
}

/// Legacy settings Close button.
pub fn settings_close_rect(panel: Rect) -> Rect {
    Rect::new(
        panel.x + 100,
        panel.y + panel.h as i32 - SETTINGS_ROW_Y0 as i32,
        120,
        SETTINGS_ROW_H,
    )
}

pub const SETTINGS_APP_W: u32 = 560;
pub const SETTINGS_APP_H: u32 = 400;
pub const SETTINGS_APP_TOGGLE_Y: u32 = 68; // below the panel title

/// Full settings app (560×400, one-third from the top).
pub fn settings_app_panel_rect(screen_w: u32, screen_h: u32) -> Rect {
    Rect::new(
        ((screen_w - SETTINGS_APP_W) / 2) as i32,
        ((screen_h - SETTINGS_APP_H) / 3) as i32,
        SETTINGS_APP_W,
        SETTINGS_APP_H,
    )
}

/// The Appearance theme toggle — the drawn rect (content area only), shared
/// by the draw and hover. The click path keeps its looser region.
pub fn settings_app_toggle_rect(panel: Rect) -> Rect {
    Rect::new(
        panel.x + 158,
        panel.y + SETTINGS_APP_TOGGLE_Y as i32,
        panel.w - 158 - 16,
        SETTINGS_ROW_H,
    )
}

pub const TASK_MANAGER_W: u32 = 560;
pub const TASK_MANAGER_H: u32 = 360;
pub const TASK_MANAGER_ROW_Y0: u32 = 54; // below title + column header
pub const TASK_MANAGER_ROW_H: u32 = 20;

/// Task manager panel (560×360, one-third from the top).
pub fn task_manager_panel_rect(screen_w: u32, screen_h: u32) -> Rect {
    Rect::new(
        ((screen_w - TASK_MANAGER_W) / 2) as i32,
        ((screen_h - TASK_MANAGER_H) / 3) as i32,
        TASK_MANAGER_W,
        TASK_MANAGER_H,
    )
}

/// Task manager row `i` (full-width, 20px).
pub fn task_manager_row_rect(panel: Rect, i: usize) -> Rect {
    Rect::new(
        panel.x + 4,
        panel.y + TASK_MANAGER_ROW_Y0 as i32 + (i as u32 * TASK_MANAGER_ROW_H) as i32,
        panel.w - 8,
        TASK_MANAGER_ROW_H,
    )
}

/// Max visible task-manager rows — the same cap the draw applies, so hover
/// never lights a row past the panel bottom.
pub fn task_manager_max_visible(panel: Rect) -> usize {
    (panel.h.saturating_sub(TASK_MANAGER_ROW_Y0 + 4) / TASK_MANAGER_ROW_H) as usize
}

// ---------------------------------------------------------------------------
// Notification overlay
// ---------------------------------------------------------------------------

pub const NOTIF_X_RIGHT: u32 = 10; // right edge of a row, from the screen edge
pub const NOTIF_W: u32 = 300;
pub const NOTIF_H: u32 = 64;
pub const NOTIF_TOP: u32 = 10;
pub const NOTIF_PITCH: u32 = 72;
pub const NOTIF_MAX_VISIBLE: usize = 4;

/// Notification overlay row `i` (top-right corner). Shared by the overlay
/// draw and `Desktop::hover_target` so hover always matches the drawn row.
pub fn notification_rect(screen_w: u32, i: usize) -> Rect {
    Rect::new(
        (screen_w - NOTIF_W - NOTIF_X_RIGHT) as i32,
        (NOTIF_TOP + i as u32 * NOTIF_PITCH) as i32,
        NOTIF_W,
        NOTIF_H,
    )
}

// ---------------------------------------------------------------------------
// Window resize / snap
// ---------------------------------------------------------------------------

pub const RESIZE_MARGIN: i32 = 4;
pub const SNAP_MARGIN: i32 = 15;
pub const MIN_WIN_W: u32 = 100;
pub const MIN_WIN_H: u32 = 80;
pub const SNAP_PREVIEW_COLOR: u32 = 0x403D5AFE;

/// What a left-click hits on a window, in priority order: the control
/// buttons (close, then minimize) are tested BEFORE the titlebar they are
/// drawn on, so a click on a button actually activates it; everything else
/// on the 28px strip is a titlebar drag; then the resize edges; then the
/// content. Right/middle-click do NOT use this table — they keep the whole
/// strip as the system-menu / close target (see `Desktop::handle_*_click`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowHit {
    Titlebar,
    Close,
    Minimize,
    ResizeEdge(u8),
    Content,
    Outside,
}

/// Classify a left-click position against a window's chrome. The control
/// buttons win over the titlebar they are drawn on (see `WindowHit`).
pub(crate) fn hit_window(x: i32, y: i32, w: u32, h: u32, pt: Point) -> WindowHit {
    if close_btn_rect(x, y, w).hit_test(pt) {
        WindowHit::Close
    } else if min_btn_rect(x, y, w).hit_test(pt) {
        WindowHit::Minimize
    } else if titlebar_rect(x, y, w).hit_test(pt) {
        WindowHit::Titlebar
    } else {
        let edges = hit_window_edge(x, y, w, h, pt);
        if edges != 0 {
            WindowHit::ResizeEdge(edges)
        } else if Rect::new(x, y, w, h).hit_test(pt) {
            WindowHit::Content
        } else {
            WindowHit::Outside
        }
    }
}

/// Resize edge flags (1 = left, 2 = right, 4 = bottom) under the pointer.
pub fn hit_window_edge(x: i32, y: i32, w: u32, h: u32, pt: Point) -> u8 {
    let mx = pt.x;
    let my = pt.y;
    let mut edges = 0u8;
    if mx >= x && mx < x + RESIZE_MARGIN && my >= y && my < y + h as i32 {
        edges |= 1;
    }
    if mx >= x + w as i32 - RESIZE_MARGIN && mx < x + w as i32 && my >= y && my < y + h as i32 {
        edges |= 2;
    }
    if my >= y + h as i32 - RESIZE_MARGIN && my < y + h as i32 {
        edges |= 4;
    }
    edges
}

/// Which screen region a release-drag lands in, if any. Same precedence as
/// the historical release logic: corners first, then edges, then none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapRegion {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// The snap region under `(mx, my)` given the work area `(sw, ty)`.
pub(crate) fn snap_region_at(mx: i32, my: i32, sw: i32, ty: i32) -> Option<SnapRegion> {
    let edge_left = mx < SNAP_MARGIN;
    let edge_right = mx > sw - SNAP_MARGIN;
    let edge_top = my < SNAP_MARGIN;
    let edge_bot = my > ty - SNAP_MARGIN;
    match (edge_left, edge_right, edge_top, edge_bot) {
        (true, _, true, _) => Some(SnapRegion::TopLeft),
        (true, _, _, true) => Some(SnapRegion::BottomLeft),
        (_, true, true, _) => Some(SnapRegion::TopRight),
        (_, true, _, true) => Some(SnapRegion::BottomRight),
        (true, _, _, _) => Some(SnapRegion::Left),
        (_, true, _, _) => Some(SnapRegion::Right),
        (_, _, true, _) => Some(SnapRegion::Top),
        (_, _, _, true) => Some(SnapRegion::Bottom),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Max tooltip text chars — the tooltip draw truncates with this before
/// sizing, so a long window title can't push the box off the screen edge.
pub const TOOLTIP_TEXT_MAX: usize = 42;

/// Truncate a display string to `max` chars for drawing, without allocating
/// and without ever splitting a UTF-8 char (falls back to the full string).
pub fn trunc(s: &str, max: usize) -> &str {
    s.get(..max).unwrap_or(s)
}
