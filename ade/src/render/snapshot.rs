//! Render snapshot — read-only capture of desktop state for the frame renderer.

use crate::apps::about::AboutState;
use crate::apps::settings::SettingsAppState;
use crate::apps::task_manager::TaskManagerState;
use crate::core::desktop::Desktop;
use crate::core::desktop_icons::DesktopIcon;
use crate::core::geometry::{ContextMenu, Rect, RubberBand};
use crate::core::settings::SettingsState;
use crate::core::start_menu::StartMenuState;
use crate::core::window::{AppWindow, HoverTarget, WindowState};
use crate::service::clipboard::ClipboardManager;
use crate::service::notification::Notification;
use crate::util::app_catalog::AppCatalog;
use crate::util::explorer::ExplorerState;
use crate::util::profiler::MetricsSnapshot;

pub struct RenderSnapshot<'a> {
    pub screen_w: u32,
    pub screen_h: u32,
    pub theme: &'a libsarga::theme::Theme,
    pub windows: &'a [AppWindow],
    pub(crate) icons: &'a [DesktopIcon],
    pub mouse: crate::core::geometry::Point,
    pub start_menu: bool,
    pub(crate) start_menu_state: Option<&'a StartMenuState>,
    pub context_menu: Option<ContextMenu>,
    pub cursor_visible: bool,
    pub cursor_alpha: u8,
    pub fullscreen: bool,
    pub switcher_active: bool,
    pub switcher_idx: usize,
    pub rubber: Option<RubberBand>,
    pub(crate) notifications: &'a [Notification],
    pub(crate) tray: &'a [crate::core::tray::TrayEntry],
    pub(crate) clipboard: Option<&'a ClipboardManager>,
    pub(crate) settings: Option<&'a SettingsState>,
    pub(crate) app_reg: Option<&'a AppCatalog>,
    pub(crate) explorers: &'a [ExplorerState],
    pub(crate) settings_app: Option<&'a SettingsAppState>,
    pub(crate) task_manager: Option<&'a TaskManagerState>,
    pub(crate) about: Option<&'a AboutState>,
    pub focus_visible: bool,
    pub focused_bounds: Option<Rect>,
    /// The interactive surface under a11y keyboard focus (payload: the same
    /// `HoverTarget` that surface uses for its hover light) — `None` when
    /// focus is on a non-interactive node or the mouse is in charge.
    /// Resolved from the a11y tree by `Desktop::focused_target`: a focused
    /// Button maps by parent role to the Start button, a taskbar window
    /// button, a start-menu app row, or a window Close/Minimize control.
    /// One value replaces the former per-surface focus fields, so every
    /// draw site compares a single `hover || focused` equality against the
    /// same payloads it already uses for hover.
    pub(crate) focused: Option<HoverTarget>,
    /// The interactive surface under the pointer, computed once per frame
    /// by `Desktop::hover_target()` (window control buttons, taskbar,
    /// start menu, tray, clipboard rows). Every surface reads this instead
    /// of hit-testing the mouse position itself.
    pub(crate) hover: Option<HoverTarget>,
    /// Raw primary-mouse-button state for this frame. This is button state,
    /// NOT a per-surface decision: each surface combines it with its own
    /// `hover` equality check (e.g. `snap.mouse_down && snap.hover == ...`)
    /// to render its pressed state.
    pub mouse_down: bool,
    pub tooltip: Option<&'a str>,
    pub tooltip_x: i32,
    pub tooltip_y: i32,
    /// Tooltip fade progress 0..=255 (fade-in on show, fade-out on dismiss).
    pub tooltip_alpha: u8,
    pub debug_overlay: bool,
    pub(crate) debug_metrics: MetricsSnapshot,
    pub window_count: usize,
    pub notification_count: usize,
    pub snap_preview: Option<Rect>,
}

impl<'a> RenderSnapshot<'a> {
    pub fn taskbar_y(&self) -> u32 {
        self.screen_h - crate::layout::TASKBAR_H
    }
}

impl<'a> From<&'a Desktop> for RenderSnapshot<'a> {
    /// Capture the desktop's renderable state in one snapshot — the frame
    /// renderer's input. Built here (not on Desktop) so the coordinator
    /// stays a thin shell over state and any new frame input lands in one
    /// place. `Desktop::snapshot()` is a one-line delegate to this.
    fn from(d: &'a Desktop) -> Self {
        let fs =
            d.wm.iter()
                .iter()
                .any(|w| w.state == WindowState::Fullscreen);

        let focused_bounds = d.focus.focused().and_then(|id| {
            d.a11y_tree
                .nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.bounds)
        });

        let hover = d.hover_target();
        let mouse_down = d.mouse_btn();

        // The surface under keyboard focus, resolved by one method
        // (`Desktop::focused_target`) to the same HoverTarget the surface
        // uses for hover — replacing the former three parallel per-surface
        // scans. None whenever the mouse is in charge (focus_visible false).
        let focused = if d.focus_visible {
            d.focus.focused().and_then(|fid| d.focused_target(fid))
        } else {
            None
        };

        let (tooltip_text, tooltip_x, tooltip_y, tooltip_alpha) = match d.tooltips.active {
            Some(ref t) => (Some(t.text.as_str()), t.x, t.y, t.alpha),
            _ => (None, 0, 0, 0),
        };

        RenderSnapshot {
            screen_w: d.screen_w,
            screen_h: d.screen_h,
            theme: d.theme_svc.current(),
            windows: d.wm.iter(),
            icons: &d.desktop_icons.icons,
            mouse: crate::core::geometry::Point::new(d.mouse_x, d.mouse_y),
            debug_overlay: d.debug_overlay,
            debug_metrics: d.profiler.snapshot(),
            window_count: d.wm.len(),
            notification_count: d.services.notifications.visible_notifications().len(),
            start_menu: d.start_menu.open,
            start_menu_state: Some(&d.start_menu),
            app_reg: Some(&d.app_reg),
            context_menu: d.context_menu,
            cursor_visible: d.cursor_visible,
            cursor_alpha: d.cursor_alpha(),
            fullscreen: fs,
            switcher_active: d.switcher_active,
            switcher_idx: d.switcher_idx,
            rubber: d.desktop_icons.rubber,
            notifications: d.services.notifications.visible_notifications(),
            tray: d.tray.entries,
            clipboard: Some(&d.services.clipboard),
            settings: Some(&d.settings),
            explorers: &d.explorers,
            settings_app: Some(&d.settings_app),
            task_manager: Some(&d.task_manager),
            about: Some(&d.about_state),
            focus_visible: d.focus_visible,
            focused_bounds,
            focused,
            hover,
            mouse_down,
            tooltip: tooltip_text,
            tooltip_x,
            tooltip_y,
            tooltip_alpha,
            snap_preview: d.render_snap_preview(),
        }
    }
}
