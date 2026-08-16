//! Desktop coordinator — event dispatch, window management, layout logic.

use crate::core::damage::DamageTracker;
use crate::core::desktop_icons::DesktopIcons;
use crate::core::event::Event;
use crate::core::geometry::{ContextMenu, MenuItem, Point, Rect};
use crate::core::start_menu::StartMenuState;
use crate::core::tray::SystemTray;
use crate::core::window::{WindowButton, WindowId, WindowState};
use crate::util::app_catalog::AppId;
use crate::util::log::Logger;
use crate::util::profiler::Profiler;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

pub(crate) enum Cursor {
    Default,
    ResizeH,
    ResizeV,
    ResizeDiagonal,
    Move,
    Hand,
}

const DESKTOP_MENU: &[MenuItem] = &[
    MenuItem {
        label: "Refresh",
        action: "refresh",
    },
    MenuItem {
        label: "---",
        action: "",
    },
    MenuItem {
        label: "Settings",
        action: "settings",
    },
    MenuItem {
        label: "Terminal",
        action: "terminal",
    },
];

const ICON_MENU: &[MenuItem] = &[
    MenuItem {
        label: "Open",
        action: "open",
    },
    MenuItem {
        label: "Delete",
        action: "delete",
    },
    MenuItem {
        label: "Rename",
        action: "rename",
    },
];

// Window system menu for right-click on titlebar
const SYSTEM_MENU: &[MenuItem] = &[
    MenuItem {
        label: "Restore",
        action: "restore",
    },
    MenuItem {
        label: "Move",
        action: "move",
    },
    MenuItem {
        label: "Size",
        action: "size",
    },
    MenuItem {
        label: "Minimize",
        action: "minimize",
    },
    MenuItem {
        label: "Maximize",
        action: "maximize",
    },
    MenuItem {
        label: "---",
        action: "",
    },
    MenuItem {
        label: "Close",
        action: "close",
    },
];
use crate::core::window_manager::WindowManager;

pub(crate) enum TilingMode {
    Floating,
    Tile,
    Monocle,
}
use crate::input::keys;
use crate::input::{is_desktop_shortcut, resolve, KeyAction, KeyEvent};
use crate::layout::{self, WindowHit};
use crate::render::snapshot::RenderSnapshot;

pub struct Desktop {
    pub(crate) screen_w: u32,
    pub(crate) screen_h: u32,
    pub(crate) wm: WindowManager,
    pub(crate) start_menu: StartMenuState,
    pub(crate) context_menu: Option<ContextMenu>,
    pub clock_ticks: u64,
    pub(crate) mouse_x: i32,
    pub(crate) mouse_y: i32,
    mouse_btn: bool,
    prev_mouse_btn: bool,
    pub(crate) drag_active: bool,
    pub(crate) cursor: Cursor,
    pub(crate) cursor_visible: bool,
    cursor_alpha: u8,
    cursor_blink_tick: u8,
    resize_win: Option<WindowId>,
    resize_edges: u8,
    resize_rect: Rect,
    last_click_time: u64,
    last_click_pos: Point,
    pub(crate) double_click: bool,
    pub(crate) desktop_icons: DesktopIcons,
    pub(crate) theme_svc: crate::core::theme_service::ThemeService,
    pub damage: DamageTracker,
    pub(crate) clock_cache: crate::render::clock::ClockCache,
    tiling_mode: TilingMode,
    prev_tiling_geos: alloc::vec::Vec<Rect>,
    focus_history: VecDeque<u64>,
    pub(crate) switcher_active: bool,
    pub(crate) switcher_idx: usize,
    pub(crate) app_reg: crate::util::app_catalog::AppCatalog,
    pub session: crate::service::session::SessionManager,
    pub(crate) services: crate::service::service_manager::ServiceManager,
    pub(crate) tray: SystemTray,
    pub(crate) settings: crate::core::settings::SettingsState,
    pub(crate) task_manager: crate::apps::task_manager::TaskManagerState,
    pub(crate) about_state: crate::apps::about::AboutState,
    pub(crate) settings_app: crate::apps::settings::SettingsAppState,
    pub(crate) explorers: alloc::vec::Vec<crate::util::explorer::ExplorerState>,
    pub(crate) a11y_tree: crate::sec::a11y::A11yTree,
    pub(crate) focus: crate::sec::a11y::FocusManager,
    pub(crate) tooltips: crate::apps::tooltip::TooltipManager,
    pub(crate) focus_visible: bool,
    // The start-menu app row the a11y ring intends to be on — durable app-id
    // identity, not a positional node id. The tree models only the VISIBLE
    // row window, so scroll and typed-search changes can push the focused
    // row's node off-window (or renumber it); `build_tree` clamps the scroll
    // and re-lands the ring on this row every frame, letting arrows reach
    // rows beyond the visible window and keeping the focused row visible.
    pub(crate) menu_focus_app: Option<AppId>,
    // Window-activation intent from the a11y path. `activate_a11y_node`
    // brings the activated window to front, which REORDERS `wm`; node ids
    // are positional, so the next rebuild renumbers every window surface
    // and `validate`'s fingerprint check sees the old id name a different
    // node — it would re-sync the ring to a sibling taskbar button instead
    // of the window the user just activated. `build_tree` consumes this
    // field to re-land the ring on the activated window's OWN node in the
    // fresh tree (the durable-identity twin of `menu_focus_app`).
    pub(crate) pending_window_focus: Option<WindowId>,
    tooltip_hover_ticks: u32,
    tooltip_last_hover: Option<u32>,
    system_menu_for: Option<WindowId>,
    pub(crate) ipc_server: crate::ipc::IpcServer,
    pub(crate) ipc_transport: crate::ipc::transport::IpcTransport,
    pub(crate) service_registry: crate::ipc::ServiceRegistry,
    pub(crate) permissions: crate::sec::perms::PermissionManager,
    pub(crate) profiler: Profiler,
    pub(crate) logger: Logger,
    pub(crate) debug_overlay: bool,
}

impl Desktop {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            screen_w: w,
            screen_h: h,
            wm: WindowManager::new(),
            start_menu: StartMenuState::new(),
            context_menu: None,
            clock_ticks: 0,
            mouse_x: (w / 2) as i32,
            mouse_y: (h / 2) as i32,
            mouse_btn: false,
            prev_mouse_btn: false,
            drag_active: false,
            cursor: Cursor::Default,
            cursor_visible: true,
            cursor_alpha: 255,
            cursor_blink_tick: 0,
            resize_win: None,
            resize_edges: 0,
            resize_rect: Rect::new(0, 0, 0, 0),
            last_click_time: 0,
            last_click_pos: Point::new(0, 0),
            double_click: false,
            desktop_icons: DesktopIcons::new(),
            theme_svc: crate::core::theme_service::ThemeService::new(),
            damage: DamageTracker::new(),
            clock_cache: crate::render::clock::ClockCache::new(),
            tiling_mode: TilingMode::Floating,
            prev_tiling_geos: alloc::vec::Vec::new(),
            focus_history: VecDeque::new(),
            switcher_active: false,
            switcher_idx: 0,
            app_reg: crate::util::app_catalog::AppCatalog::new(),
            session: crate::service::session::SessionManager::new(64),
            services: crate::service::service_manager::ServiceManager::new(),
            tray: SystemTray::new(),
            settings: crate::core::settings::SettingsState::new(),
            task_manager: crate::apps::task_manager::TaskManagerState::new(),
            about_state: crate::apps::about::AboutState::new(),
            settings_app: crate::apps::settings::SettingsAppState::new(),
            explorers: alloc::vec::Vec::new(),
            a11y_tree: crate::sec::a11y::A11yTree::new(),
            focus: crate::sec::a11y::FocusManager::new(),
            tooltips: crate::apps::tooltip::TooltipManager::new(),
            focus_visible: false,
            menu_focus_app: None,
            pending_window_focus: None,
            tooltip_hover_ticks: 0,
            tooltip_last_hover: None,
            system_menu_for: None,
            ipc_server: crate::ipc::IpcServer::new(),
            ipc_transport: crate::ipc::transport::IpcTransport::new(),
            service_registry: {
                let mut reg = crate::ipc::ServiceRegistry::new();
                reg.register_defaults();
                reg
            },
            permissions: crate::sec::perms::PermissionManager::new(),
            profiler: Profiler::new(),
            logger: Logger::new(),
            debug_overlay: false,
        }
    }
    pub fn taskbar_y(&self) -> u32 {
        self.screen_h - layout::TASKBAR_H
    }

    pub fn advance_clock(&mut self) {
        self.clock_ticks += 1;
    }

    pub fn tick(&mut self) {
        self.profiler.frame_timer.start(self.clock_ticks);
        self.advance_clock();
        // Breathing cursor: smooth alpha blink over 30 ticks
        self.cursor_blink_tick = (self.cursor_blink_tick + 1) % 30;
        if self.cursor_blink_tick < 15 {
            self.cursor_alpha = 255 - (self.cursor_blink_tick * 17);
        } else {
            self.cursor_alpha = (self.cursor_blink_tick - 15) * 17;
        }
        if self.cursor_alpha == 0 {
            self.cursor_visible = false;
            self.damage.mark_full();
        } else if !self.cursor_visible {
            self.cursor_visible = true;
            self.damage.mark_full();
        }
        if self.session.reap(
            &mut self.wm,
            &mut self.services,
            &mut self.permissions,
            &mut self.ipc_transport,
            self.clock_ticks,
        ) {
            self.damage.mark_full();
        }
        let reqs = self.ipc_transport.ingest();
        for req in reqs {
            self.ipc_server.submit_request(req);
        }
        self.process_ipc();
        let responses = self.ipc_server.drain_responses();
        self.ipc_transport.deliver(responses);
        let mut anim_active = false;
        for w in self.wm.iter_mut() {
            if w.flags.opacity < 255 {
                w.flags.opacity = w.flags.opacity.saturating_add(25);
            }
            if w.flags.opacity == 255 {
                w.anim_opacity = 255;
            }
            if w.tick_animation() {
                anim_active = true;
            }
        }
        if anim_active {
            self.damage.mark_full();
        }
        // Process closing windows (remove after shrink animation)
        if !self.wm.process_closing().is_empty() {
            self.damage.mark_full();
        }
        self.services.tick(self.clock_ticks);
        self.a11y_tree = crate::sec::a11y::build_tree(self);
        // Tooltip fades (in/out) animate frame by frame: the manager and the
        // hover path report every visual change so the damage-gated render
        // loop repaints during the fade instead of snapping at the end.
        if self.tooltips.tick() || self.tick_tooltip_hover() {
            self.damage.mark_full();
        }
        self.pump_terminals();
        self.profiler.frame_timer.stop(self.clock_ticks);
        if self.clock_ticks.is_multiple_of(1000) {
            self.logger.info(self.clock_ticks, "tick");
        }
    }

    /// Pump pty output from terminal windows into their text surfaces.
    /// Parsing/cursor state lives in the window's `TextSurface`
    /// (`consume_pty_bytes`), so escape sequences split across reads are
    /// handled correctly.
    fn pump_terminals(&mut self) {
        for i in 0..self.wm.len() {
            let Some(id) = self.wm.id_at(i) else { continue };
            let Some(fd) = self.wm.lookup(id).and_then(|w| w.pty_fd()) else {
                continue;
            };
            let mut pfds = [libsarga::net::PollFd {
                fd,
                events: libsarga::net::POLLIN,
                revents: 0,
            }];
            if libsarga::net::poll(&mut pfds, 0).unwrap_or(0) <= 0 {
                continue;
            }
            if pfds[0].revents & libsarga::net::POLLIN == 0 {
                continue;
            }
            let mut buf = [0u8; 256];
            let n = match libsarga::io::read(fd, &mut buf) {
                Ok(n) => n,
                Err(_) => continue, // slave side closed (sash exited)
            };
            if n == 0 {
                continue;
            }
            let mut changed = false;
            if let Some(w) = self.wm.lookup_mut(id) {
                changed = w.surface_mut().consume_pty_bytes(&buf[..n]);
                w.surface_mut().truncate(500);
            }
            if changed {
                self.damage.mark_full();
            }
        }
    }
    /// True if the tooltip's visible state changed this frame (show, hide
    /// started, keep-alive cancelled a fade) so the caller can repaint.
    fn tick_tooltip_hover(&mut self) -> bool {
        // A modal overlay swallows the pointer (same set as `hover_target`),
        // so no tooltips while one is up: with `hover_target()` None the
        // Close/Minimize/taskbar arms never fire and the owner fallback
        // would leak a plain title under the overlay. An already-visible
        // tooltip (shown before the overlay opened) dismisses too. The hide
        // fires only on the transition (tracked by `tooltip_last_hover` —
        // Some only while the pointer tracks a node): a per-tick hide would
        // keep restarting the fade and leave an invisible zombie tooltip.
        // The START MENU is deliberately NOT in this set: its rows must keep
        // showing their tooltips (StartApp descriptions), so menu-open
        // leaves the fallback arm reachable for out-of-menu pointers — a
        // known, narrower instance of the same leak, scoped out here.
        if self.overlay_open() {
            // Copy the owner out first: `hide` needs &mut self.tooltips,
            // so it cannot run inside the `active.as_ref()` borrow.
            let changed = if self.tooltip_last_hover.is_some() {
                match self.tooltips.active.as_ref().map(|t| t.owner) {
                    Some(owner) => self.tooltips.hide(owner),
                    None => false,
                }
            } else {
                false
            };
            self.tooltip_last_hover = None;
            return changed;
        }
        let hovered = self.a11y_tree.node_at(self.mouse_x, self.mouse_y);
        let hover_id = hovered.map(|n| n.id);
        if hover_id != self.tooltip_last_hover {
            // Pointer moved to a different surface (or off a surface): begin
            // the delayed dismiss of the tooltip owned by the *previous*
            // node. The fade-out is a few ticks long, so a quick return to
            // the same node cancels it (see keep_alive below).
            let changed = if let Some(prev) = self.tooltip_last_hover {
                self.tooltips
                    .hide(crate::apps::tooltip::TooltipOwner::Pointer(prev))
            } else {
                false
            };
            self.tooltip_hover_ticks = 0;
            self.tooltip_last_hover = hover_id;
            return changed;
        }
        // Same node as last frame: refresh its tooltip so it never expires
        // mid-hover (kills the old show→timeout→re-show flicker every ~2s),
        // and cancel any fade-out started by a one-frame gap in the pointer
        // tracking. A foreign owner is ignored by both calls.
        if let Some(id) = hover_id {
            if self
                .tooltips
                .keep_alive(crate::apps::tooltip::TooltipOwner::Pointer(id))
            {
                return true;
            }
        }
        if self.tooltips.active.is_some() {
            return false;
        }
        if let Some(id) = hover_id {
            self.tooltip_hover_ticks = self.tooltip_hover_ticks.saturating_add(1);
            if self.tooltip_hover_ticks >= 30 {
                if let Some(n) = self.a11y_tree.nodes.iter().find(|n| n.id == id) {
                    // All hover text comes from the single formatter in the
                    // a11y tree builder (`tooltip_label`): it resolves the
                    // unified hover target (Close/Minimize distinction,
                    // taskbar buttons, start-menu rows) and the owner/label
                    // fallback, so no label logic lives in the coordinator.
                    let text = crate::sec::a11y::tooltip_label(self, n, self.hover_target());
                    if !text.is_empty() {
                        let tx = self.mouse_x + 12;
                        let ty = self.mouse_y;
                        return self.tooltips.show(
                            crate::apps::tooltip::TooltipOwner::Pointer(id),
                            &text,
                            tx,
                            ty,
                            120,
                        );
                    }
                }
            }
        }
        false
    }

    fn save_geometries(&mut self) {
        self.prev_tiling_geos.clear();
        for w in self.wm.iter() {
            self.prev_tiling_geos.push(Rect::new(w.x, w.y, w.w, w.h));
        }
    }

    fn restore_geometries(&mut self) {
        for (i, &geo) in self.prev_tiling_geos.iter().enumerate() {
            if let Some(wid) = self.wm.id_at(i) {
                if let Some(aw) = self.wm.lookup_mut(wid) {
                    aw.x = geo.x;
                    aw.y = geo.y;
                    aw.w = geo.w;
                    aw.h = geo.h;
                }
            }
        }
        self.prev_tiling_geos.clear();
    }

    fn apply_tile(&mut self) {
        self.save_geometries();
        let sw = self.screen_w;
        let th = self.taskbar_y();
        let n = self.wm.len();
        if n == 0 {
            return;
        }
        if n == 1 {
            if let Some(wid) = self.wm.id_at(0) {
                if let Some(w) = self.wm.lookup_mut(wid) {
                    w.x = 0;
                    w.y = 0;
                    w.w = sw;
                    w.h = th;
                }
            }
            return;
        }
        let master_w = sw * 6 / 10;
        let stack_w = sw - master_w;
        let stack_h = th / (n as u32 - 1);
        for i in 0..n {
            if let Some(wid) = self.wm.id_at(i) {
                if let Some(w) = self.wm.lookup_mut(wid) {
                    if i == 0 {
                        w.x = 0;
                        w.y = 0;
                        w.w = master_w;
                        w.h = th;
                    } else {
                        w.x = master_w as i32;
                        w.y = ((i as u32 - 1) * stack_h) as i32;
                        w.w = stack_w;
                        w.h = stack_h;
                    }
                }
            }
        }
        self.tiling_mode = TilingMode::Tile;
        self.damage.mark_full();
    }

    fn apply_monocle(&mut self) {
        self.save_geometries();
        let sw = self.screen_w;
        let th = self.taskbar_y();
        for i in 0..self.wm.len() {
            if let Some(wid) = self.wm.id_at(i) {
                if let Some(w) = self.wm.lookup_mut(wid) {
                    w.x = 0;
                    w.y = 0;
                    w.w = sw;
                    w.h = th;
                }
            }
        }
        self.tiling_mode = TilingMode::Monocle;
        self.damage.mark_full();
    }

    fn set_floating(&mut self) {
        self.restore_geometries();
        self.tiling_mode = TilingMode::Floating;
        self.damage.mark_full();
    }

    fn record_current_focus(&mut self) {
        if let Some(id) = self.wm.active() {
            self.focus_history.retain(|&i| i != id.0);
            self.focus_history.push_back(id.0);
            if self.focus_history.len() > 20 {
                self.focus_history.pop_front();
            }
        }
    }

    fn cycle_window(&mut self) {
        if self.wm.len() < 2 {
            return;
        }
        if !self.switcher_active {
            let current = self
                .wm
                .active()
                .and_then(|id| self.wm.position_of(id))
                .unwrap_or(0);
            self.switcher_idx = current;
            self.switcher_active = true;
        } else {
            self.switcher_idx = (self.switcher_idx + 1) % self.wm.len();
        }
        self.damage.mark_full();
    }

    fn cycle_tiling(&mut self) {
        match self.tiling_mode {
            TilingMode::Floating => self.apply_tile(),
            TilingMode::Tile => self.apply_monocle(),
            TilingMode::Monocle => self.set_floating(),
        }
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => {
                if !self.handle_a11y_key(key) {
                    self.handle_key(key);
                }
            }
            Event::MouseClick(p) => {
                self.focus_visible = false;
                self.handle_click(p.x, p.y);
            }
            Event::MouseMiddle(p) => {
                self.focus_visible = false;
                self.handle_middle_click(p.x, p.y);
            }
            Event::MouseRight(p) => {
                self.focus_visible = false;
                self.handle_right_click(p.x, p.y);
            }
            Event::MouseDrag(p) => self.handle_drag(p.x, p.y),
            Event::Scroll(delta) => self.handle_scroll(delta),
            Event::MouseRelease => self.release_drag(),
        }
    }

    fn handle_a11y_key(&mut self, key: u16) -> bool {
        // A key carrying modifier bits is not an a11y key: it must fall
        // through to the keymap routing, or the future Ctrl+Alt+Backspace
        // chord (0x308) would be swallowed here before `resolve` ever sees
        // it. Plain keys (Ctrl+letter folds, arrows, Esc, Enter) have zero
        // high bits and behave exactly as before.
        if key & 0xFF00 != 0 {
            return false;
        }
        let key = (key & 0xFF) as u8;
        match key {
            keys::SCAN_UP | keys::SCAN_DOWN => {
                // Up / Down arrows
                self.focus_visible = true;
                let dir = if key == keys::SCAN_UP {
                    crate::sec::a11y::FocusDirection::Up
                } else {
                    crate::sec::a11y::FocusDirection::Down
                };
                self.focus.move_focus(dir, &self.a11y_tree);
                self.sync_menu_focus_intent();
                self.damage.mark_full();
                true
            }
            keys::SCAN_LEFT | keys::SCAN_RIGHT => {
                // Left / Right arrows
                self.focus_visible = true;
                let dir = if key == keys::SCAN_LEFT {
                    crate::sec::a11y::FocusDirection::Left
                } else {
                    crate::sec::a11y::FocusDirection::Right
                };
                self.focus.move_focus(dir, &self.a11y_tree);
                self.sync_menu_focus_intent();
                self.damage.mark_full();
                true
            }
            keys::KEY_ENTER | keys::SCAN_ENTER => {
                // Enter (ASCII or scan code). An open overlay consumes the
                // activation first — mouse-click semantics: `handle_click`
                // checks the overlays before the taskbar and windows, so a
                // click on a taskbar button or Close control with a modal up
                // only dismisses the modal. Keyboard activation mirrors
                // that: the first Enter closes the overlay, the next Enter
                // acts on the still-focused node.
                //
                // EXCEPT the start menu: Enter launches instead of
                // dismissing. A row focused under the ring launches that
                // row's app (`menu_row_app` resolves it by bounds — the
                // keyboard equivalent of clicking the row); otherwise a
                // TYPED search launches the highlighted app. Both are the
                // keyboard equivalents of clicking a menu row, and both
                // would be swallowed by the `dismiss_overlays` branch
                // below, making menu launch dead on the real event path
                // (`handle_key`'s Enter arm is only reachable via
                // synthetic `handle_key_event`). An EMPTY search with no
                // row focused keeps the dismiss behavior — Enter on Start
                // with no search closes the menu, pinned by
                // `test_a11y_close_button`.
                self.focus_visible = true;
                if self.start_menu.open {
                    let row_app = match self.focus.focused() {
                        Some(fid) => self.menu_row_app(fid),
                        None => None,
                    };
                    if let Some(app_id) = row_app {
                        self.launch_app(app_id);
                        self.damage.mark_full();
                        return true;
                    }
                    if !self.start_menu.search.is_empty() {
                        if let Some(app_id) = self.start_menu.selected_app() {
                            self.launch_app(app_id);
                            self.damage.mark_full();
                            return true;
                        }
                    }
                }
                if self.dismiss_overlays() {
                    self.damage.mark_full();
                    return true;
                }
                if let Some(fid) = self.focus.focused() {
                    self.activate_a11y_node(fid);
                }
                self.damage.mark_full();
                true
            }
            keys::KEY_ESC | keys::SCAN_ESC => {
                // Escape (ASCII or scan code): dismiss the focus ring and any
                // open overlay — the same set `dismiss_overlays` closes for
                // Enter (start menu, context menu, settings, settings app,
                // task manager, about). With the ring and overlays clear, a
                // fullscreen window exits fullscreen — the behavior the
                // keymap Escape grab used to carry, now here
                // because that grab was unreachable from the real event path
                // (this arm consumes Esc before `handle_key` ever runs).
                // When NOTHING is open — no ring, no fullscreen, no windows,
                // no overlays, no switcher, no drag — Esc is the
                // byte-deliverable session-end path: 0x1B is the one distinct
                // control byte the key stream actually carries, so a hardware
                // Esc on an empty desktop reaches userspace today (the
                // Ctrl+Alt+Backspace chord stays the other path but is
                // kernel-gated on Alt delivery — docs/session-lifecycle.md,
                // Phase C). This arm is the single home of Escape: a keymap
                // grab would be dead code.
                //
                // Terminal guard: with NO overlay, NO fullscreen, and the a11y
                // ring NOT active, a focused terminal window forwards 0x1B to
                // the shell instead of being swallowed here. This closes the
                // Phase C gap the keymap router's terminal block documents
                // (`handle_a11y_key` consumed Esc before `handle_key` could
                // reach its pty write), so hardware Esc reaches sash (vi,
                // readline, menus) on the real byte path. The ring-active
                // check is the modality guard: Esc with the ring up dismisses
                // the ring — it must NOT leak 0x1B into the shell.
                let ring_was_active = self.focus_visible;
                self.focus_visible = false;
                self.focus.blur();
                if !self.dismiss_overlays() {
                    // No overlay was dismissed. A fullscreen window exits
                    // fullscreen (the session-end check requires an empty
                    // window list, so the two never conflict). Otherwise,
                    // with the ring NOT up (a ring-up press already did its
                    // dismiss above — the first Esc with the ring active
                    // must not leak 0x1B into a shell nor end the session):
                    // a focused terminal forwards Esc to its shell; a truly
                    // empty desktop — no windows, no switcher, no drag in
                    // progress — ends the session.
                    let fullscreen_id = self.wm.active().filter(|&id| {
                        self.wm
                            .lookup(id)
                            .is_some_and(|w| w.state == WindowState::Fullscreen)
                    });
                    match fullscreen_id {
                        Some(id) => {
                            self.wm.toggle_fullscreen(id, self.screen_w, self.screen_h);
                        }
                        None => {
                            if !ring_was_active {
                                if self.focused_has_pty() {
                                    if let Some(fd) = self.wm.focused_mut().and_then(|w| w.pty_fd())
                                    {
                                        let _ = libsarga::io::write(fd, &[keys::KEY_ESC]);
                                    }
                                } else if self.wm.is_empty()
                                    && !self.switcher_active
                                    && !self.drag_active
                                    && self.resize_win.is_none()
                                {
                                    self.session.request_end();
                                }
                            }
                        }
                    }
                }
                self.damage.mark_full();
                true
            }
            _ => false,
        }
    }

    /// Dismiss whichever overlay is up — the same overlay set `handle_click`
    /// checks before the taskbar and windows (start menu, context menu,
    /// legacy settings panel, settings app, task manager, about). Only one is
    /// normally open at a time, so the relative order matters only in the
    /// rare double-overlay case (deliberately not matched arm-for-arm to
    /// `handle_click`, which checks `settings` first and the context menu
    /// after the taskbar). Returns true if something was dismissed — the
    /// caller consumes the activation, exactly like a mouse click that lands
    /// on the modal instead of the surface beneath it.
    fn dismiss_overlays(&mut self) -> bool {
        if self.start_menu.open {
            self.start_menu.open = false;
            return true;
        }
        if self.context_menu.is_some() {
            self.context_menu = None;
            return true;
        }
        if self.settings.open {
            self.settings.open = false;
            return true;
        }
        if self.settings_app.open {
            self.settings_app.open = false;
            return true;
        }
        if self.task_manager.open {
            self.task_manager.open = false;
            return true;
        }
        if self.about_state.open {
            self.about_state.open = false;
            return true;
        }
        false
    }

    fn activate_a11y_node(&mut self, id: u32) {
        let node = match self.a11y_tree.nodes.iter().find(|n| n.id == id) {
            Some(n) => n.clone(),
            None => return,
        };
        match node.role {
            crate::sec::a11y::A11yRole::Window => {
                // bring window to front (the owner stamp replaces the old
                // title-as-index parse, which always no-oped on real titles).
                // Record the activation intent: the bring-to-front reorders
                // `wm`, and on the next rebuild every window-surface node id
                // shifts (ids are positional) — `build_tree` consumes this
                // to keep the ring on THIS window instead of letting
                // `validate` re-sync it to a sibling taskbar button.
                if let Some(wid) = node.owner {
                    self.wm.bring_to_front(wid);
                    self.pending_window_focus = Some(wid);
                }
            }
            crate::sec::a11y::A11yRole::Button => {
                // Button semantics come from the tree structure, not the
                // label: the Start button (sentinel owner) toggles the start
                // menu; a Button child of a Window node is that window's
                // chrome control (Close or Minimize — discriminated by the
                // stamped label, since parent role alone cannot tell the
                // pair apart); a Button child of the Taskbar node is a
                // taskbar window button (bring the owner to front, restoring
                // it first if minimized — mirroring a taskbar mouse click).
                // The parent-role guard keeps chrome and taskbar buttons
                // distinct even if a window title is literally "Close" or
                // "Minimize".
                if node.owner == Some(crate::core::window::START_BUTTON_OWNER) {
                    // Keyboard users open the menu the way the mouse does.
                    // Closing is handled by `dismiss_overlays` — an Enter
                    // with the menu open is consumed before this arm runs —
                    // so activation only ever opens it here.
                    self.start_menu.toggle(&self.app_reg);
                    return;
                }
                let parent_role = node.parent.and_then(|p| {
                    self.a11y_tree
                        .nodes
                        .iter()
                        .find(|n| n.id == p)
                        .map(|n| n.role)
                });
                if parent_role == Some(crate::sec::a11y::A11yRole::StartMenu) {
                    // Start-menu app row: launch the app the row maps to
                    // (`menu_row_app` resolves it by bounds), exactly like a
                    // mouse click on the row — `launch_app` closes the menu.
                    // Only reachable via direct activation; the Enter arm
                    // intercepts the row case before `dismiss_overlays`, so
                    // this arm is the semantic home of "a StartMenu-child
                    // Button launches its row".
                    if let Some(app_id) = self.menu_row_app(node.id) {
                        self.launch_app(app_id);
                    }
                    return;
                }
                if parent_role == Some(crate::sec::a11y::A11yRole::Window) {
                    if let Some(wid) = node.owner {
                        // Chrome controls are discriminated by the stamped
                        // label (via the shared `window_button_from_label`
                        // — the same reverse map the render snapshot's
                        // focused-control resolution uses, so a label
                        // rename can't drift between activation and the
                        // focus light). Explicit match, NOT a label check
                        // with a close fall-through: an unknown chrome
                        // button (a future Maximize node) must no-op,
                        // never destructively close its window.
                        match crate::core::window::window_button_from_label(node.label.as_str()) {
                            Some(crate::core::window::WindowButton::Minimize) => {
                                // Mirror a mouse click on the min button. A
                                // minimized window's chrome stays in the
                                // tree, so re-activation is gated: re-minim-
                                // izing would re-run the slide animation.
                                // The window stays in the wm, so the focused
                                // node stays valid — no re-sync (and none
                                // wanted: the ring should stay on the control
                                // the user just used).
                                if !self
                                    .wm
                                    .lookup(wid)
                                    .is_some_and(|w| w.state == WindowState::Minimized)
                                {
                                    self.wm.minimize(wid, self.screen_w, self.taskbar_y());
                                }
                            }
                            Some(crate::core::window::WindowButton::Close) => {
                                self.wm.close(wid);
                                // The close is animated: the window's nodes
                                // stay in the tree until `process_closing`
                                // removes it, so a focused Close id would go
                                // stale mid-settle. Re-sync focus to the next
                                // visible focusable node not owned by the
                                // closing window.
                                self.focus.resync_after_close(&self.a11y_tree, wid);
                            }
                            None => {}
                        }
                    }
                } else if let Some(wid) = node.owner {
                    if self
                        .wm
                        .lookup(wid)
                        .is_some_and(|w| w.state == WindowState::Minimized)
                    {
                        self.wm.restore(wid);
                    }
                    self.wm.bring_to_front(wid);
                    // Same activation intent as the Window-node arm: the
                    // ring follows the window the user just raised, not a
                    // taskbar sibling the reorder makes `validate` fall
                    // back to.
                    self.pending_window_focus = Some(wid);
                }
            }
            crate::sec::a11y::A11yRole::Icon => {
                // find and open the icon
                let name = node.label.clone();
                for ic in &self.desktop_icons.icons {
                    if ic.name == name {
                        self.exec_context_action("open");
                        break;
                    }
                }
            }
            crate::sec::a11y::A11yRole::Taskbar => {
                // The taskbar's one interactive action is the start menu —
                // same toggle as the Start button (the old mouse-position
                // gate was arbitrary for a keyboard activation).
                self.start_menu.toggle(&self.app_reg);
            }
            _ => {}
        }
    }

    /// Resolve the app id a focused start-menu row node maps to. A row node
    /// is a Button child of the StartMenu node whose bounds equal the shared
    /// `layout::menu_item_rect` for its row index — resolved by geometry, not
    /// label, so renamed apps and duplicate names stay correct. Returns None
    /// for non-row nodes (the Enter arm then falls through to dismiss).
    fn menu_row_app(&self, fid: u32) -> Option<AppId> {
        self.menu_row_index(fid)
            .map(|i| self.start_menu.filtered[i])
    }

    /// The `HoverTarget` of the focused a11y node, if it is an interactive
    /// surface — the single focus-resolution every draw site shares through
    /// the render snapshot's `focused` field (the same payloads each surface
    /// already compares for its `hover` light, so the draws just union
    /// `hover || focused`). A focused Button resolves by parent role: a
    /// Taskbar child to the Start button (sentinel owner) or a taskbar
    /// window button (owner); a StartMenu child to its app row (the same
    /// bounds equality `menu_row_app` uses); a Window child to its
    /// Close/Minimize control (chrome label); a TrayPanel child to its
    /// tray entry; a Desktop child to its notification row. Anything else
    /// — Window nodes, the StartMenu container, icons, the tray panel —
    /// resolves to None: focus may rest there (the ring still draws) but
    /// no surface light applies.
    pub(crate) fn focused_target(&self, fid: u32) -> Option<crate::core::window::HoverTarget> {
        use crate::sec::a11y::{focused_button_under_role, A11yRole};
        // The three surface families share one parent-role + focus-id lookup
        // (`focused_button_under_role`) and differ only in how they resolve
        // the node: taskbar buttons by owner, start-menu rows by bounds,
        // window chrome by label. A node has exactly one parent, so at most
        // one family matches.
        if let Some(node) = focused_button_under_role(&self.a11y_tree, fid, A11yRole::Taskbar) {
            return match node.owner {
                Some(crate::core::window::START_BUTTON_OWNER) => {
                    Some(crate::core::window::HoverTarget::StartButton)
                }
                Some(wid) => Some(crate::core::window::HoverTarget::TaskbarButton(wid)),
                None => None,
            };
        }
        if focused_button_under_role(&self.a11y_tree, fid, A11yRole::StartMenu).is_some() {
            // StartMenu children are all Buttons, discriminated by geometry
            // (the shared rect each surface draws): an app row -> StartApp,
            // a sidebar category -> StartCategory, a recent tile ->
            // StartRecent. The rects never collide, so order is irrelevant
            // — and the focused light matches the hover light on every
            // surface, so the draw just unions `hover || focused`.
            if let Some(i) = self.menu_row_index(fid) {
                return Some(crate::core::window::HoverTarget::StartApp(i));
            }
            if let Some(i) = self.menu_category_index(fid) {
                return Some(crate::core::window::HoverTarget::StartCategory(i));
            }
            if let Some(ri) = self.menu_recent_index(fid) {
                return Some(crate::core::window::HoverTarget::StartRecent(ri));
            }
        }
        if let Some(node) = focused_button_under_role(&self.a11y_tree, fid, A11yRole::Window) {
            let btn = crate::core::window::window_button_from_label(node.label.as_str())?;
            return Some(crate::core::window::HoverTarget::Window {
                win: node.owner?,
                btn,
            });
        }
        if let Some(node) = focused_button_under_role(&self.a11y_tree, fid, A11yRole::TrayPanel) {
            // Tray entry: resolve the index by the same bounds equality the
            // draw and hover use (each entry's node carries the drawn
            // `tray_entry_rect`), so the focused light lands on exactly the
            // entry under the ring.
            let ty = self.taskbar_y();
            let tray_len = self.tray.entries.len() as u32;
            for i in 0..tray_len as usize {
                if layout::tray_entry_rect(i, ty, self.screen_w, tray_len) == node.bounds {
                    return Some(crate::core::window::HoverTarget::Tray(i));
                }
            }
            return None;
        }
        if let Some(node) = focused_button_under_role(&self.a11y_tree, fid, A11yRole::Desktop) {
            // Notification rows are the only Button children of Desktop
            // (the overlay is a desktop-level surface), so the Desktop
            // parent role discriminates them; the index resolves by bounds
            // equality over the drawn `notification_rect`.
            let notifs = self.services.notifications.visible_notifications();
            for (i, _) in notifs.iter().take(layout::NOTIF_MAX_VISIBLE).enumerate() {
                if layout::notification_rect(self.screen_w, i) == node.bounds {
                    return Some(crate::core::window::HoverTarget::Notification(i));
                }
            }
            return None;
        }
        None
    }

    /// The filtered row index of a focused start-menu row node: the same
    /// bounds-equality resolution `menu_row_app` uses to launch, exposed
    /// separately so the render snapshot can light the focused row exactly
    /// like its hover target (`HoverTarget::StartApp(i)`) — one geometry
    /// resolution, two consumers (Enter-launch and focus feedback).
    pub(crate) fn menu_row_index(&self, fid: u32) -> Option<usize> {
        if !self.start_menu.open {
            return None;
        }
        let node = crate::sec::a11y::focused_button_under_role(
            &self.a11y_tree,
            fid,
            crate::sec::a11y::A11yRole::StartMenu,
        )?;
        let menu_r = layout::menu_rect(self.taskbar_y());
        let (start, end, _) = self.start_menu.visible_range(menu_r);
        (start..end).find(|&i| layout::menu_item_rect(menu_r, i, start) == node.bounds)
    }

    /// The sidebar-category index of a focused start-menu category node:
    /// the same bounds-equality resolution `menu_row_index` uses for app
    /// rows, over the shared `menu_category_rect` geometry (subject to the
    /// same sidebar-bottom cap the tree, draw, and hover use). Returns None
    /// for non-category nodes.
    fn menu_category_index(&self, fid: u32) -> Option<usize> {
        if !self.start_menu.open {
            return None;
        }
        let node = crate::sec::a11y::focused_button_under_role(
            &self.a11y_tree,
            fid,
            crate::sec::a11y::A11yRole::StartMenu,
        )?;
        let menu_r = layout::menu_rect(self.taskbar_y());
        let sidebar_r = layout::menu_sidebar_rect(menu_r);
        (0..crate::util::app_catalog::CATEGORIES.len()).find(|&i| {
            let cat_r = layout::menu_category_rect(menu_r, i);
            cat_r.y + cat_r.h as i32 <= sidebar_r.y + sidebar_r.h as i32 && cat_r == node.bounds
        })
    }

    /// The recent-strip index of a focused start-menu recent node: bounds
    /// equality over the shared `menu_recent_rect` geometry, with the same
    /// cap and right-reserve break the tree, draw, and hover use. Returns
    /// None for non-recent nodes.
    fn menu_recent_index(&self, fid: u32) -> Option<usize> {
        if !self.start_menu.open {
            return None;
        }
        let node = crate::sec::a11y::focused_button_under_role(
            &self.a11y_tree,
            fid,
            crate::sec::a11y::A11yRole::StartMenu,
        )?;
        let menu_r = layout::menu_rect(self.taskbar_y());
        let mut rx = layout::menu_recent_x0(menu_r);
        let recent_n = self.app_reg.recent.len().min(layout::MENU_RECENT_MAX);
        for ri in 0..recent_n {
            let idx = self.app_reg.recent[ri];
            if idx >= self.app_reg.apps.len() {
                continue;
            }
            if rx + layout::MENU_RECENT_PITCH as i32
                > menu_r.x + layout::MENU_W as i32 - layout::MENU_RECENT_RIGHT_RESERVE as i32
            {
                break;
            }
            if layout::menu_recent_rect(menu_r, rx) == node.bounds {
                return Some(ri);
            }
            rx += layout::MENU_RECENT_PITCH as i32;
        }
        None
    }

    /// Keep `menu_focus_app` in lockstep with the ring after generic tree
    /// navigation (arrows and FocusFirst): resolve the focused node to its
    /// start-menu row and record the durable app id the ring intends to be
    /// on, so `build_tree` can clamp the scroll window and re-land the ring
    /// on that row even when a scroll/filter change renumbers its node.
    pub(crate) fn sync_menu_focus_intent(&mut self) {
        self.menu_focus_app = self
            .focus
            .focused()
            .and_then(|fid| self.menu_row_index(fid))
            .and_then(|i| self.start_menu.filtered.get(i).copied());
    }

    fn exec_context_action(&mut self, action: &str) {
        match action {
            "terminal" => self.spawn_app("/bin/sash", "Terminal"),
            "arrange" => {
                let positions: &[Point] = &[
                    Point::new(30, 80),
                    Point::new(30, 180),
                    Point::new(30, 280),
                    Point::new(30, 380),
                    Point::new(30, 480),
                ];
                for (i, ic) in self.desktop_icons.icons.iter_mut().enumerate() {
                    if i < positions.len() {
                        ic.x = positions[i].x;
                        ic.y = positions[i].y;
                    }
                }
            }
            "refresh" => {
                self.damage.mark_full();
            }
            "settings" => {
                self.settings.toggle();
                self.context_menu = None;
            }
            "open" => {
                let mut to_launch: Vec<(String, String)> = Vec::new();
                for ic in &self.desktop_icons.icons {
                    if ic.selected {
                        let path = match ic.name.as_str() {
                            "Terminal" => "/bin/sash",
                            "Files" | "File Browser" => "/bin/skyfiles",
                            "SkyStore" => "/bin/skystore",
                            "SkyEdit" => "/bin/skyedit",
                            "Calc" | "Calculator" => "/bin/calculator",
                            _ => "",
                        };
                        if !path.is_empty() {
                            to_launch.push((ic.name.clone(), String::from(path)));
                        }
                    }
                }
                for (name, path) in to_launch {
                    self.spawn_app(&path, &name);
                }
            }
            "delete" => {
                self.desktop_icons.icons.retain(|ic| !ic.selected);
            }
            // Window system menu actions
            "restore" => {
                if let Some(wi) = self.system_menu_for {
                    self.wm.restore(wi);
                }
                self.system_menu_for = None;
            }
            "move" => {
                if let Some(wi) = self.system_menu_for {
                    if self.wm.lookup(wi).is_some() {
                        self.wm.begin_drag(wi, self.mouse_x, self.mouse_y);
                    }
                }
                self.system_menu_for = None;
            }
            "size" => {
                if let Some(wi) = self.system_menu_for {
                    if let Some(w) = self.wm.lookup(wi) {
                        self.resize_win = Some(wi);
                        self.resize_edges = 4;
                        self.resize_rect = Rect::new(w.x, w.y, w.w, w.h);
                    }
                }
                self.system_menu_for = None;
            }
            "minimize" => {
                if let Some(wi) = self.system_menu_for {
                    self.wm.minimize(wi, self.screen_w, self.taskbar_y());
                }
                self.system_menu_for = None;
            }
            "maximize" => {
                if let Some(wi) = self.system_menu_for {
                    self.wm.toggle_maximize(wi, self.screen_w, self.taskbar_y());
                }
                self.system_menu_for = None;
            }
            "close" => {
                if let Some(wi) = self.system_menu_for {
                    self.wm.close(wi);
                }
                self.system_menu_for = None;
            }
            _ => {}
        }
    }

    fn launch_app(&mut self, app_id: AppId) {
        self.start_menu.open = false;
        let app = match self.app_reg.get(app_id) {
            Some(a) => *a,
            None => {
                self.damage.mark_full();
                return;
            }
        };
        if let crate::util::app_catalog::StartupMode::Singleton = app.startup_mode {
            if app.name == "Settings" {
                self.settings_app.open = true;
            }
            self.damage.mark_full();
            return;
        }
        if app.executable.is_empty() || app.name == "About SARGA" || app.name == "About SARGA OS" {
            self.about_state.open = true;
            self.damage.mark_full();
            return;
        }
        if app.executable == "/bin/skyfiles" && app.name.starts_with("File") {
            self.spawn_explorer();
            return;
        }
        crate::core::launcher::spawn_app_from_registry(self, &app);
        // Positive serial confirmation that a window launch actually
        // happened — the QEMU harness (qemu_gui_login.exp) waits for
        // `[ade] launched <name>` after the keyboard chain (Tab → Enter →
        // type → Enter) so the window-open leg is provable on real input,
        // not just inferred from silence.
        libsarga::io::print_str(&alloc::format!("[ade] launched {}\n", app.name));
        self.damage.mark_full();
    }

    /// Keyboard routing — decode the raw byte into a `KeyEvent` and apply
    /// the keymap routing table (`crate::input`). Contextual states (start
    /// menu, window switcher, terminal focus) are routing rules here, not
    /// magic constants; the old `DESKTOP_KEYS` list now lives in the table
    /// as the `desktop` binding flag.
    fn handle_key(&mut self, key: u16) {
        // Decode the packed kernel value (low byte = char, bits 8..10 =
        // alt/ctrl/shift). The pty write path keeps the plain low byte, so
        // Ctrl+C still reaches the shell as 0x03.
        let ev = KeyEvent::from_raw(key);
        self.handle_key_event_raw(ev, (key & 0xFF) as u8);
    }

    /// Direct `KeyEvent` entry — for tests and future structured input
    /// producers that can express chords (Ctrl+Alt+Backspace) the byte
    /// stream cannot. The raw byte defaults to the canonical code, which is
    /// only wrong for forwarded control bytes; a chord never reaches the
    /// terminal-forward path (it is a desktop grab).
    pub(crate) fn handle_key_event(&mut self, ev: KeyEvent) {
        self.handle_key_event_raw(ev, ev.code);
    }

    fn handle_key_event_raw(&mut self, ev: KeyEvent, raw: u8) {
        let terminal_focused = self.focused_has_pty();

        // Global grabs fire before any contextual state (historical
        // precedence), but yield to a focused terminal.
        if !terminal_focused {
            // NOTE: no Escape grab here — it would be
            // unreachable. `handle_a11y_key` consumes Esc (ASCII or scan
            // code) before `handle_key` ever runs, so Escape's dismiss +
            // fullscreen-exit + empty-desktop session-end behavior lives
            // entirely in that arm. There is deliberately no Escape row in
            // `input::BINDINGS` (it could never fire from the byte path —
            // see the NOTE at the table); the contextual arms below are
            // reachable only through synthetic `handle_key_event`.
            if let Some(KeyAction::ToggleDebugOverlay) = resolve(ev) {
                self.debug_overlay = !self.debug_overlay;
                self.damage.mark_full();
                return;
            }
        }

        if self.start_menu.open {
            match resolve(ev) {
                // NOTE: no Escape arm here — Esc never resolves (there is no
                // Escape binding row; the a11y arm consumes it first). The
                // start menu is closed by that arm's `dismiss_overlays`.
                Some(KeyAction::Enter) => {
                    if let Some(app_id) = self.start_menu.selected_app() {
                        self.launch_app(app_id);
                        self.damage.mark_full();
                    }
                }
                Some(KeyAction::FocusNext) => {
                    // Tab → next category
                    self.start_menu.cat_idx =
                        (self.start_menu.cat_idx + 1) % crate::util::app_catalog::CATEGORIES.len();
                    self.start_menu.selected = 0;
                    self.start_menu.scroll = 0;
                    self.start_menu.rebuild_filter(&self.app_reg);
                    self.damage.mark_full();
                }
                Some(KeyAction::Backspace) => {
                    self.start_menu.search.pop();
                    self.start_menu.rebuild_filter(&self.app_reg);
                    self.damage.mark_full();
                }
                _ => {}
            }
            if let Some(ch) = ev.text() {
                // printable ASCII → search
                self.start_menu.search.push(ch as u8);
                self.start_menu.rebuild_filter(&self.app_reg);
                self.damage.mark_full();
            }
            return;
        }

        if self.switcher_active {
            match resolve(ev) {
                Some(KeyAction::FocusNext) => {
                    // Tab → next window
                    self.switcher_idx = (self.switcher_idx + 1) % self.wm.len();
                    self.damage.mark_full();
                }
                Some(KeyAction::Enter) => {
                    // Enter → confirm selection (Escape is handled by the
                    // a11y arm; there is no Escape binding to resolve here)
                    if let Some(wid) = self.wm.id_at(self.switcher_idx) {
                        self.wm.bring_to_front(wid);
                    }
                    self.switcher_active = false;
                    self.damage.mark_full();
                }
                _ => {}
            }
            return;
        }

        // Terminal window focused: keys go to the pty master, not the desktop
        // (Ctrl+C to the shell, Tab for completion). The keymap decides what
        // stays desktop-side: Ctrl+W closes the terminal (killing the shell),
        // Ctrl+T/Ctrl+E/etc. keep their desktop meaning, and the
        // Ctrl+Alt+Backspace logout chord stays a grab so it works from a
        // terminal too. Esc reaches the shell via the a11y Esc arm's terminal
        // guard (no overlay/fullscreen/ring -> 0x1B to the pty); this block's
        // raw-byte Esc write is the synthetic `handle_key_event` twin of that
        // guard, so both the real and synthetic paths deliver Esc to sash
        // (docs/session-lifecycle.md, Phase C).
        if terminal_focused && !is_desktop_shortcut(ev) {
            if let Some(last) = self.wm.focused_mut() {
                if let Some(fd) = last.pty_fd() {
                    let byte = if raw == keys::KEY_LF || raw == keys::KEY_ENTER {
                        keys::KEY_ENTER
                    } else {
                        raw
                    };
                    let _ = libsarga::io::write(fd, &[byte]);
                    return;
                }
            }
        }

        if let Some(action) = resolve(ev) {
            match action {
                KeyAction::Quit => {
                    // Session end — the Ctrl+Alt+Backspace chord, only with no
                    // windows open (with any window open it is a deliberate
                    // no-op, so it can't trip the logout loop mid-work). Esc
                    // on an empty desktop is the second session-end path,
                    // handled in the a11y Esc arm before this router runs.
                    // `request_end()` (not `process::exit`) lets the main
                    // loop unwind and print the `[ade] session ended` marker
                    // before returning the exit code.
                    if self.wm.is_empty() {
                        self.session.request_end();
                    }
                    return;
                }
                KeyAction::CloseFocused => {
                    if let Some(id) = self.wm.active() {
                        self.wm.close(id);
                        self.damage.mark_full();
                    }
                    return;
                }
                KeyAction::CycleTiling => {
                    self.cycle_tiling();
                    return;
                }
                KeyAction::CycleWindow => {
                    self.cycle_window();
                    return;
                }
                KeyAction::ClipboardPanel => {
                    self.damage.mark_full();
                    // Cross-world clipboard probe (facility audit F1): the
                    // kernel store (SYS_CLIPBOARD=125) is the canonical
                    // clipboard, so printing it here makes a console-sash
                    // yank observable on serial when the panel opens
                    // (tests/qemu_clipboard_probe.exp greps this line).
                    let len = libsarga::io::clipboard_len();
                    if len > 0 {
                        let mut buf = alloc::vec![0u8; len];
                        let n = libsarga::io::clipboard_read(&mut buf);
                        let text = core::str::from_utf8(&buf[..n]).unwrap_or("(non-utf8)");
                        libsarga::io::print_str(&alloc::format!(
                            "[clip] kernel store: {}\n",
                            if text.is_empty() {
                                "(empty read)"
                            } else {
                                text
                            },
                        ));
                    } else {
                        libsarga::io::print_str("[clip] kernel store: (empty)\n");
                    }
                    return;
                }
                KeyAction::ToggleAot => {
                    if let Some(id) = self.wm.active() {
                        if let Some(w) = self.wm.lookup_mut(id) {
                            w.always_on_top = !w.always_on_top;
                        }
                        self.damage.mark_full();
                    }
                    return;
                }
                KeyAction::DemoNotification => {
                    self.services.notify(
                        "Demo",
                        "This is a test notification",
                        1,
                        120,
                        self.clock_ticks,
                    );
                    self.damage.mark_full();
                    return;
                }
                KeyAction::DismissNotification => {
                    let visible = self.services.notifications.visible_notifications();
                    if let Some(last) = visible.last() {
                        self.services.notifications.dismiss(last.id);
                        self.damage.mark_full();
                    }
                    return;
                }
                KeyAction::ClearNotifications => {
                    self.services.notifications.dismiss_all();
                    self.damage.mark_full();
                    return;
                }
                KeyAction::OpenSettings => {
                    self.settings_app.open = !self.settings_app.open;
                    if self.settings_app.open {
                        self.settings_app.current_page =
                            crate::apps::settings::SettingsPage::Appearance;
                    }
                    self.damage.mark_full();
                    return;
                }
                KeyAction::OpenTaskManager => {
                    self.task_manager.open = !self.task_manager.open;
                    self.damage.mark_full();
                    return;
                }
                KeyAction::ClearTerminal => {
                    // Ctrl+L = clear terminal
                    if let Some(last) = self.wm.focused_mut() {
                        if last.focused {
                            last.surface_mut().clear();
                            self.damage.mark_full();
                        }
                    }
                    return;
                }
                KeyAction::Backspace => {
                    // Delete/Backspace → delete selected icons; if nothing was
                    // selected, fall through so the typing path pops a char.
                    let before = self.desktop_icons.icons.len();
                    self.desktop_icons.icons.retain(|ic| !ic.selected);
                    if self.desktop_icons.icons.len() < before {
                        self.damage.mark_full();
                        return;
                    }
                }
                KeyAction::FocusNext => {
                    // Tab → a11y focus ring. `wm.focus_next()` cycles window
                    // focus and returns false on an empty desktop — leaving
                    // the ring visible but orphaned (no focused node, so a
                    // following Enter could never activate anything). Fall
                    // back to First so Tab starts the ring on the first
                    // focusable node (the Taskbar), the keyboard entry point
                    // to the start menu. This is what makes the QEMU
                    // window-open leg driveable on today's kernel: arrows
                    // (E0-extended) are dropped, so Tab+Enter+type+Enter is
                    // the only keyboard path that opens a window.
                    if !self.wm.focus_next() {
                        self.focus
                            .move_focus(crate::sec::a11y::FocusDirection::First, &self.a11y_tree);
                    }
                    self.focus_visible = true;
                    self.damage.mark_full();
                    return;
                }
                // Handled by the global grab / menu / switcher blocks above
                // (unreachable here: the grabs return first, and a focused
                // terminal sends them to the pty).
                KeyAction::ToggleDebugOverlay => {}
                // Deliberate fall-through: Enter reaches the typing path so
                // it can emit a newline into the focused window.
                KeyAction::Enter => {}
            }
        }

        // No keymap action: Ctrl+Q, plain 'q', Ctrl+Backspace etc. are
        // deliberately unbound — the ONLY session-end path is the
        // Ctrl+Alt+Backspace chord (KeyAction::Quit above). Everything
        // reaching this point is either text or a silent no-op.

        self.damage.mark_full();
        if let Some(last) = self.wm.focused_mut() {
            if last.focused && last.x > -100 {
                if let Some(ch) = ev.text() {
                    last.surface_mut().push_char(ch);
                } else if ev.code == keys::KEY_ENTER {
                    let cmd = last.surface().last_line().cloned().unwrap_or_default();
                    last.surface_mut().push_line(alloc::format!("$ {}", cmd));
                    last.surface_mut().truncate(500);
                } else if ev.code == keys::KEY_BACKSPACE {
                    last.surface_mut().pop_char();
                }
            }
        }
    }

    pub(crate) fn spawn_app(&mut self, path: &str, title: &str) {
        crate::core::launcher::spawn_app(self, path, title);
    }

    pub(crate) fn spawn_terminal(&mut self) {
        crate::core::launcher::spawn_terminal(self);
    }

    /// True when the focused window hosts a terminal (pty master).
    pub(crate) fn focused_has_pty(&self) -> bool {
        self.wm
            .active()
            .and_then(|id| self.wm.lookup(id))
            .is_some_and(|w| w.pty_fd().is_some())
    }

    pub(crate) fn spawn_explorer(&mut self) {
        crate::core::launcher::spawn_explorer(self);
    }

    pub fn update_mouse(&mut self, mx: i32, my: i32, btn: bool) -> (bool, bool, bool) {
        self.mouse_x = mx;
        self.mouse_y = my;
        let just_pressed = btn && !self.mouse_btn;
        let just_released = !btn && self.mouse_btn;
        self.prev_mouse_btn = self.mouse_btn;
        self.mouse_btn = btn;

        if just_pressed {
            let dt = self.clock_ticks.saturating_sub(self.last_click_time);
            let dist = (mx - self.last_click_pos.x).abs() + (my - self.last_click_pos.y).abs();
            self.double_click = dt < 25 && dist < 6;
            self.last_click_time = self.clock_ticks;
            self.last_click_pos = Point::new(mx, my);
            self.drag_active = false;
        }
        if self.mouse_btn && !self.drag_active {
            let dx = mx - self.last_click_pos.x;
            let dy = my - self.last_click_pos.y;
            self.drag_active = dx.abs() + dy.abs() > 4;
        } else if just_released {
            self.drag_active = false;
        }

        self.update_cursor();
        (just_pressed, just_released, self.drag_active)
    }

    /// Apply a theme by darkness — the single code path behind both settings
    /// UIs' Dark Theme toggles (legacy panel + full settings app).
    fn toggle_theme(&mut self, dark: bool) {
        self.theme_svc.set(if dark {
            libsarga::theme::Theme::dark()
        } else {
            libsarga::theme::Theme::light()
        });
    }

    pub(crate) fn handle_click(&mut self, mx: i32, my: i32) {
        self.damage.mark_full();
        if self.settings.open {
            match self.settings.hit_test_action(mx, my, &self.snapshot()) {
                Some(crate::apps::AppAction::Close) => {
                    self.settings.open = false;
                    self.context_menu = None;
                }
                Some(crate::apps::AppAction::SetTheme(dark)) => {
                    self.settings.theme_dark = dark;
                    self.toggle_theme(dark);
                }
                Some(crate::apps::AppAction::ToggleSound) => {
                    self.settings.sound_on = !self.settings.sound_on;
                }
                // Not produced by this panel's hit_test_action.
                Some(_) => {}
                None => self.settings.open = false,
            }
            self.damage.mark_full();
            return;
        }
        self.record_current_focus();
        let taskbar_y = self.taskbar_y() as i32;

        if self.start_menu.open {
            // modern start menu click handling — row geometry lives in one
            // place (`start_menu::menu_hover_at`, shared with the draw and
            // hover); this block maps the hit row to its click action.
            let menu_rect = layout::menu_rect(taskbar_y as u32);

            if !menu_rect.hit_test(Point::new(mx, my)) {
                self.start_menu.open = false;
                return;
            }

            // search bar click
            if layout::menu_search_rect(menu_rect).hit_test(Point::new(mx, my)) {
                return; // focus search (keyboard will handle input)
            }

            let pt = Point::new(mx, my);
            match crate::core::start_menu::menu_hover_at(
                &self.start_menu,
                &self.app_reg,
                pt,
                taskbar_y as u32,
            ) {
                // Switch category — the same selection/scroll reset the old
                // inline loop performed.
                Some(crate::core::window::HoverTarget::StartCategory(i)) => {
                    self.start_menu.cat_idx = i;
                    self.start_menu.selected = 0;
                    self.start_menu.scroll = 0;
                    self.start_menu.rebuild_filter(&self.app_reg);
                }
                // Launch the tapped app-row.
                Some(crate::core::window::HoverTarget::StartApp(i)) => {
                    if i < self.start_menu.filtered.len() {
                        let app_id = self.start_menu.filtered[i];
                        self.launch_app(app_id);
                    }
                }
                // Launch the tapped recent tile.
                Some(crate::core::window::HoverTarget::StartRecent(ri)) => {
                    if ri < self.app_reg.recent.len() {
                        let idx = self.app_reg.recent[ri];
                        if idx < self.app_reg.apps.len() {
                            self.launch_app(AppId(idx));
                        }
                    }
                }
                // Power buttons have no click action yet (keyboard nav +
                // hover only) — clicking one is a no-op, like before.
                Some(crate::core::window::HoverTarget::StartPower(_)) => {}
                // Not a start-menu row (search bar, empty menu area):
                // no-op, menu stays open.
                _ => {}
            }
            self.damage.mark_full();
            return;
        }

        if self.settings_app.open {
            match self.settings_app.hit_test_action(mx, my, &self.snapshot()) {
                Some(crate::apps::AppAction::Close) => self.settings_app.open = false,
                Some(crate::apps::AppAction::SelectPage(page)) => {
                    self.settings_app.current_page = page;
                }
                Some(crate::apps::AppAction::SetTheme(dark)) => {
                    self.settings_app.app = dark;
                    self.toggle_theme(dark);
                }
                // Not produced by this app's hit_test_action.
                Some(_) => {}
                None => self.settings_app.open = false,
            }
            self.damage.mark_full();
            return;
        }
        if self.task_manager.open {
            match self.task_manager.hit_test_action(mx, my, &self.snapshot()) {
                Some(crate::apps::AppAction::FocusWindow(idx)) => {
                    self.task_manager.selected = idx;
                    if let Some(wid) = self.wm.id_at(idx) {
                        self.wm.bring_to_front(wid);
                    }
                }
                // Not produced by the task manager's hit_test_action.
                Some(_) => {}
                None => self.task_manager.open = false,
            }
            self.damage.mark_full();
            return;
        }
        // About is dismiss-only (no hit regions), so it closes on any click
        // without an action round-trip.
        if self.about_state.open {
            self.about_state.open = false;
            self.damage.mark_full();
            return;
        }

        // Context menu — the last of the overlay set, and like the panels
        // above it owns the pointer: it is checked BEFORE the taskbar and
        // windows, so a click anywhere outside the menu (taskbar included)
        // only dismisses it and never acts beneath — the same modal
        // semantics `dismiss_overlays` gives the keyboard (an a11y Enter on
        // a taskbar node with the menu up only dismisses it). This block
        // historically ran AFTER the taskbar, so a taskbar click with the
        // menu open brought a window to front and left the menu up — mouse
        // and keyboard disagreed (pinned by
        // test_a11y_overlay_mouse_keyboard_parity).
        if let Some(cm) = self.context_menu {
            let mw = 150u32;
            let mh = cm.items.len() as u32 * 28 + 10;
            if Rect::new(cm.x, cm.y, mw, mh).hit_test(Point::new(mx, my)) {
                let idx = ((my - cm.y - 5) / 28) as usize;
                if idx < cm.items.len() {
                    let action = cm.items[idx].action;
                    self.exec_context_action(action);
                }
            }
            self.context_menu = None;
            self.damage.mark_full();
            return;
        }
        if my >= taskbar_y {
            if layout::start_btn_rect(taskbar_y as u32).hit_test(Point::new(mx, my)) {
                self.start_menu.open_with(&self.app_reg);
                return;
            }
            for i in 0..self.wm.len() {
                if layout::taskbar_btn_rect(i, taskbar_y as u32).hit_test(Point::new(mx, my)) {
                    let is_min = {
                        let s = self.wm.iter();
                        s[i].state == WindowState::Minimized
                    };
                    if let Some(wid) = self.wm.id_at(i) {
                        if is_min {
                            self.wm.restore(wid);
                        }
                        self.wm.bring_to_front(wid);
                    }
                    return;
                }
            }
            return;
        }

        let pt = Point::new(mx, my);

        // icon click
        if let Some(idx) = self.desktop_icons.icon_at(mx, my) {
            self.desktop_icons.toggle_icon(idx);
            return;
        }

        for i in (0..self.wm.len()).rev() {
            let (x, y, w, h) = {
                let s = self.wm.iter();
                (s[i].x, s[i].y, s[i].w, s[i].h)
            };
            let wid = match self.wm.id_at(i) {
                Some(wid) => wid,
                None => continue,
            };
            match layout::hit_window(x, y, w, h, pt) {
                WindowHit::Titlebar => {
                    if self.double_click {
                        self.wm
                            .toggle_maximize(wid, self.screen_w, self.taskbar_y());
                        return;
                    }
                    self.wm.bring_to_front(wid);
                    self.wm.begin_drag(wid, mx, my);
                    return;
                }
                WindowHit::Close => {
                    self.wm.close(wid);
                    return;
                }
                WindowHit::Minimize => {
                    self.wm.minimize(wid, self.screen_w, self.taskbar_y());
                    return;
                }
                WindowHit::ResizeEdge(edges) => {
                    self.resize_win = Some(wid);
                    self.resize_edges = edges;
                    self.resize_rect = Rect::new(x, y, w, h);
                    self.wm.bring_to_front(wid);
                    return;
                }
                WindowHit::Content => {
                    // Explorer content click
                    if let Some(exp_id) = self.wm.iter()[i].explorer_id {
                        if let Some(exp_state) = self.explorers.iter_mut().find(|e| e.id == exp_id)
                        {
                            let aw_ref = &self.wm.iter()[i];
                            crate::util::explorer::handle_explorer_click(
                                exp_state,
                                mx,
                                my,
                                aw_ref,
                                self.double_click,
                            );
                        }
                        self.wm.bring_to_front(wid);
                        return;
                    }
                    self.wm.bring_to_front(wid);
                    return;
                }
                WindowHit::Outside => {}
            }
        }

        // desktop click → deselect icons, start rubber band
        self.desktop_icons.click_empty(mx, my);
    }

    fn handle_right_click(&mut self, mx: i32, my: i32) {
        self.context_menu = None;
        let pt = Point::new(mx, my);
        let ty = self.taskbar_y() as i32;

        // taskbar right-click
        if my >= ty {
            return;
        }

        // icon right-click
        if let Some(_idx) = self.desktop_icons.icon_at(mx, my) {
            self.context_menu = Some(ContextMenu {
                x: mx,
                y: my,
                items: ICON_MENU,
            });
            self.damage.mark_full();
            return;
        }

        // Window titlebar right-click → system menu. Tested directly against
        // the full strip (not `hit_window`, which now returns Close/Minimize
        // over the buttons) so a right-click on a control button still opens
        // the window's system menu.
        for i in (0..self.wm.len()).rev() {
            let (x, y, w, _h) = {
                let s = self.wm.iter();
                (s[i].x, s[i].y, s[i].w, s[i].h)
            };
            if layout::titlebar_rect(x, y, w).hit_test(pt) {
                if let Some(wid) = self.wm.id_at(i) {
                    self.system_menu_for = Some(wid);
                    self.context_menu = Some(ContextMenu {
                        x: mx,
                        y: my,
                        items: SYSTEM_MENU,
                    });
                }
                self.damage.mark_full();
                return;
            }
        }

        // desktop right-click
        self.context_menu = Some(ContextMenu {
            x: mx,
            y: my,
            items: DESKTOP_MENU,
        });
        self.damage.mark_full();
    }

    fn handle_middle_click(&mut self, mx: i32, my: i32) {
        let taskbar_y = self.taskbar_y() as i32;
        let pt = Point::new(mx, my);
        if my >= taskbar_y {
            for i in 0..self.wm.len() {
                if layout::taskbar_btn_rect(i, taskbar_y as u32).hit_test(pt) {
                    if let Some(wid) = self.wm.id_at(i) {
                        self.wm.close(wid);
                    }
                    self.damage.mark_full();
                    return;
                }
            }
        }
        // middle-click on the full titlebar strip (control buttons
        // included) → close window
        for i in (0..self.wm.len()).rev() {
            let (x, y, w, _h) = {
                let s = self.wm.iter();
                (s[i].x, s[i].y, s[i].w, s[i].h)
            };
            if layout::titlebar_rect(x, y, w).hit_test(pt) {
                if let Some(wid) = self.wm.id_at(i) {
                    self.wm.close(wid);
                }
                self.damage.mark_full();
                return;
            }
        }
    }

    fn handle_scroll(&mut self, delta: i8) {
        self.damage.mark_full();
        if let Some(w) = self.wm.focused_mut() {
            w.surface_mut().scroll_by(delta);
        }
    }

    /// The interactive surface under the pointer, computed once per frame.
    /// One hit-test pass replaces the per-surface hit tests that used to run
    /// in every draw: window control buttons (topmost first, same
    /// `layout::hit_window` table as `handle_click`), taskbar buttons, tray
    /// entries, start-menu rows, and clipboard rows. Surfaces take priority
    /// in the order they are drawn (overlay panels first, then the start
    /// menu, then the taskbar, then windows), so hover feedback always
    /// matches what a click at that point would actually hit. Modal overlays
    /// swallow the pointer exactly like clicks do.
    /// pub(crate) so `RenderSnapshot::from` can read it.
    /// Any modal overlay is up (context menu, settings panel, settings app,
    /// task manager, about). These swallow the pointer — the same set
    /// `handle_click` checks before the taskbar and windows — so hover
    /// feedback AND tooltips are suppressed while one is open (visibility
    /// always matches what a click would actually hit).
    fn overlay_open(&self) -> bool {
        self.context_menu.is_some()
            || self.settings.open
            || self.settings_app.open
            || self.task_manager.open
            || self.about_state.open
    }

    /// Hover targets for the open modal panels (legacy settings, settings
    /// app, task manager) — the topmost surfaces while open, so their rows
    /// report hover before anything beneath (same priority as `handle_click`
    /// checking the overlays first). Row geometry is the `layout::*_rect`
    /// the draws share, so hover always matches the lit row. A pointer on
    /// an open panel's background (or outside it) returns `None`, and the
    /// `overlay_open()` guard then silences everything beneath — matching
    /// the click semantics where an empty hit closes the panel. The task
    /// manager's row count is capped the same way the draw caps it.
    fn panel_hover(&self) -> Option<crate::core::window::HoverTarget> {
        let pt = Point::new(self.mouse_x, self.mouse_y);
        // Drawn order in the Overlay layer is settings, settings_app, task
        // manager, about — later is topmost, so check in reverse.
        if self.task_manager.open {
            let panel = layout::task_manager_panel_rect(self.screen_w, self.screen_h);
            let n = self.wm.len().min(layout::task_manager_max_visible(panel));
            for i in 0..n {
                if layout::task_manager_row_rect(panel, i).hit_test(pt) {
                    return Some(crate::core::window::HoverTarget::TaskManagerRow(i));
                }
            }
        }
        if self.settings_app.open {
            let panel = layout::settings_app_panel_rect(self.screen_w, self.screen_h);
            // Only the Appearance page draws the toggle; on any other page
            // the toggle rect is empty space, so no hover there.
            if self.settings_app.current_page == crate::apps::settings::SettingsPage::Appearance
                && layout::settings_app_toggle_rect(panel).hit_test(pt)
            {
                return Some(crate::core::window::HoverTarget::SettingsAppRow(0));
            }
        }
        if self.settings.open {
            let panel = layout::settings_panel_rect(self.screen_w, self.screen_h);
            for i in 0..2 {
                if layout::settings_row_rect(panel, i).hit_test(pt) {
                    return Some(crate::core::window::HoverTarget::SettingsRow(i));
                }
            }
            if layout::settings_close_rect(panel).hit_test(pt) {
                return Some(crate::core::window::HoverTarget::SettingsRow(2));
            }
        }
        None
    }

    pub(crate) fn hover_target(&self) -> Option<crate::core::window::HoverTarget> {
        // Same drag guards as handle_click/update_cursor so hover feedback
        // matches what a click would actually hit.
        if self.drag_active || self.resize_win.is_some() {
            return None;
        }
        // The open modal panels own the pointer (see `panel_hover`).
        if let Some(h) = self.panel_hover() {
            return Some(h);
        }
        // Any other open overlay (context menu, about) or a panel whose
        // pointer is off its rows: hover feedback AND tooltips are
        // suppressed while one is up (visibility always matches what a
        // click would actually hit).
        if self.overlay_open() {
            return None;
        }
        let pt = Point::new(self.mouse_x, self.mouse_y);

        // Notification overlay (top-right) — drawn AFTER the clipboard in
        // the Overlay layer, so per the "priority in the order they are
        // drawn" rule its rows are checked first. Same panel geometry the
        // draw uses.
        let notifs = self.services.notifications.visible_notifications();
        for (i, _) in notifs.iter().take(layout::NOTIF_MAX_VISIBLE).enumerate() {
            if layout::notification_rect(self.screen_w, i).hit_test(pt) {
                return Some(crate::core::window::HoverTarget::Notification(i));
            }
        }

        // Clipboard panel (drawn beneath the notifications in the Overlay
        // layer) — rows take hover priority over everything below them.
        let cb = &self.services.clipboard;
        if !cb.is_empty() {
            let n = cb.history().len();
            let panel = layout::clipboard_panel_rect(self.screen_w, self.screen_h, n);
            for i in 0..n {
                let row = layout::clipboard_row_rect(panel, i);
                if row.y + layout::CLIPBOARD_ROW_INNER_H as i32 > panel.y + panel.h as i32 {
                    break;
                }
                if row.hit_test(pt) {
                    return Some(crate::core::window::HoverTarget::ClipboardRow(i));
                }
            }
        }

        // Start menu (Popups layer, above the taskbar) — when open it owns
        // the pointer: its rows hover, and everything beneath goes quiet
        // (clicking anywhere outside a row closes the menu).
        if self.start_menu.open {
            return crate::core::start_menu::menu_hover_at(
                &self.start_menu,
                &self.app_reg,
                pt,
                self.taskbar_y(),
            );
        }

        // Taskbar surface: start button, then window buttons, then tray.
        let ty = self.taskbar_y();
        if pt.y >= ty as i32 {
            if layout::start_btn_rect(ty).hit_test(pt) {
                return Some(crate::core::window::HoverTarget::StartButton);
            }
            for i in 0..self.wm.len() {
                if layout::taskbar_btn_rect(i, ty).hit_test(pt) {
                    if let Some(wid) = self.wm.id_at(i) {
                        return Some(crate::core::window::HoverTarget::TaskbarButton(wid));
                    }
                }
            }
            let tray_len = self.tray.entries.len() as u32;
            // Same full panel rect the taskbar draws — the pointer must be
            // inside the tray panel (entries + clock) before any entry can
            // hover, so hover geometry matches the drawn panel exactly.
            if layout::tray_panel_rect(ty, self.screen_w, tray_len).hit_test(pt) {
                for i in 0..self.tray.entries.len() {
                    if layout::tray_entry_rect(i, ty, self.screen_w, tray_len).hit_test(pt) {
                        return Some(crate::core::window::HoverTarget::Tray(i));
                    }
                }
            }
            return None;
        }

        // Window control buttons, topmost window first (same reverse
        // iteration and hit table as handle_click).
        for i in (0..self.wm.len()).rev() {
            let (x, y, w, h) = {
                let s = self.wm.iter();
                (s[i].x, s[i].y, s[i].w, s[i].h)
            };
            let wid = match self.wm.id_at(i) {
                Some(wid) => wid,
                None => continue,
            };
            match layout::hit_window(x, y, w, h, pt) {
                layout::WindowHit::Close => {
                    return Some(crate::core::window::HoverTarget::Window {
                        win: wid,
                        btn: WindowButton::Close,
                    });
                }
                layout::WindowHit::Minimize => {
                    return Some(crate::core::window::HoverTarget::Window {
                        win: wid,
                        btn: WindowButton::Minimize,
                    });
                }
                // Topmost window owns the pointer: a titlebar/content/edge
                // hit on it swallows the click, so stop the scan (matches
                // handle_click stopping at the first hit). Only Outside
                // keeps looking at lower windows.
                layout::WindowHit::Titlebar
                | layout::WindowHit::ResizeEdge(_)
                | layout::WindowHit::Content => return None,
                layout::WindowHit::Outside => {}
            }
        }
        None
    }

    fn update_cursor(&mut self) {
        if self.resize_win.is_some() || self.drag_active {
            return;
        }
        let pt = Point::new(self.mouse_x, self.mouse_y);
        for i in (0..self.wm.len()).rev() {
            let (x, y, w, h) = {
                let s = self.wm.iter();
                (s[i].x, s[i].y, s[i].w, s[i].h)
            };
            let edges = layout::hit_window_edge(x, y, w, h, pt);
            if edges != 0 {
                self.cursor = match edges {
                    1 | 2 => Cursor::ResizeH,
                    4 => Cursor::ResizeV,
                    3 => Cursor::ResizeDiagonal,
                    _ => Cursor::ResizeDiagonal,
                };
                return;
            }
            if layout::titlebar_rect(x, y, w).hit_test(pt) {
                self.cursor = Cursor::Move;
                return;
            }
        }
        // icon hover → Hand cursor
        if self.desktop_icons.icon_at(pt.x, pt.y).is_some() {
            self.cursor = Cursor::Hand;
            return;
        }
        self.cursor = Cursor::Default;
    }

    pub(crate) fn handle_drag(&mut self, mx: i32, my: i32) {
        self.damage.mark_full();
        if self.desktop_icons.drag_icon {
            let dx = mx - self.last_click_pos.x;
            let dy = my - self.last_click_pos.y;
            self.desktop_icons.move_selected(dx, dy);
            return;
        }
        if self.desktop_icons.rubber.is_some() {
            self.desktop_icons.update_rubber(mx, my);
            return;
        }
        if let Some(id) = self.resize_win {
            let dx = mx - self.last_click_pos.x;
            let dy = my - self.last_click_pos.y;
            self.wm
                .resize_drag(id, self.resize_rect, self.resize_edges, dx, dy);
        } else {
            self.wm.update_drag(mx, my);
            self.wm
                .show_snap_preview(mx, my, self.screen_w, self.screen_h, self.taskbar_y());
        }
    }

    pub(crate) fn release_drag(&mut self) {
        self.wm.clear_snap_preview();
        if self.desktop_icons.rubber.is_some() {
            let sel = self.desktop_icons.end_select();
            for i in sel {
                if i < self.desktop_icons.icons.len() {
                    self.desktop_icons.icons[i].selected = true;
                }
            }
            self.damage.mark_full();
            return;
        }
        self.desktop_icons.drag_icon = false;
        if let Some(_id) = self.resize_win {
            self.resize_win = None;
            self.resize_edges = 0;
            self.wm.end_drag();
            return;
        }
        let id = self.wm.active();
        self.wm.end_drag();
        if let Some(id) = id {
            let mx = self.mouse_x;
            let my = self.mouse_y;
            if let Some(region) =
                layout::snap_region_at(mx, my, self.screen_w as i32, self.taskbar_y() as i32)
            {
                self.wm
                    .snap_to_region(id, region, self.screen_w, self.screen_h, self.taskbar_y());
            }
        }
        self.resize_win = None;
        self.resize_edges = 0;
    }

    pub(crate) fn cursor_alpha(&self) -> u8 {
        self.cursor_alpha
    }

    /// Whether the primary mouse button is currently held down. Read by the
    /// render snapshot so surfaces (window control buttons) can show a
    /// pressed state while the pointer is down on them.
    pub(crate) fn mouse_btn(&self) -> bool {
        self.mouse_btn
    }

    pub fn render_snap_preview(&self) -> Option<Rect> {
        self.wm
            .snap_preview
            .as_ref()
            .filter(|sp| sp.active)
            .map(|sp| Rect::new(sp.x, sp.y, sp.w, sp.h))
    }

    pub fn prepare_clock(&mut self) -> alloc::string::String {
        alloc::string::String::from(crate::render::clock::format_time(
            self.clock_ticks,
            &mut self.clock_cache,
        ))
    }

    pub(crate) fn permission_check(
        &self,
        app: crate::ipc::ApplicationId,
        perm: crate::ipc::permission::AppPermission,
    ) -> bool {
        self.permissions.check(app.0, perm)
    }

    /// Drains pending IPC service requests, gates each on the service's required
    /// permissions for the caller, and dispatches allowed ones through the
    /// security portal. Runs once per frame from `tick()`.
    pub fn process_ipc(&mut self) {
        // ponytail: soft-real-time ceiling — never stall a frame on a huge
        // queue; leftovers drain next frame. Load-bearing once a real IPC
        // transport lets external processes enqueue requests.
        const MAX_REQUESTS_PER_FRAME: usize = 64;
        let mut requests = self.ipc_server.drain_requests();
        if requests.len() > MAX_REQUESTS_PER_FRAME {
            self.ipc_server.pending_requests = requests.split_off(MAX_REQUESTS_PER_FRAME);
        }
        for req in requests {
            let app = req.sender;
            let granted = self.permissions.granted(app.0);
            let allowed = self
                .service_registry
                .find(req.service)
                .map(|_| granted.is_some_and(|g| g.contains(req.service.required_permission())))
                .unwrap_or(false);
            let resp = if allowed {
                crate::sec::portal::dispatch(self, app, &req)
            } else {
                crate::ipc::ServiceResponse {
                    request_id: req.request_id,
                    success: false,
                    data: alloc::vec::Vec::new(),
                    recipient: app,
                }
            };
            self.ipc_server.submit_response(resp);
        }
    }

    /// Read-only capture of the desktop's renderable state — a one-line
    /// delegate to `RenderSnapshot::from`, which lives in render/snapshot.rs.
    pub fn snapshot(&self) -> RenderSnapshot<'_> {
        RenderSnapshot::from(self)
    }
}
