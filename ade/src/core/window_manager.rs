//! Window manager — ordered window list, focus, drag, minimize, close.

use crate::core::geometry::Rect;
use crate::core::window::{AppWindow, WindowId, WindowState};
use crate::layout::{self, SnapRegion, SNAP_MARGIN};
use alloc::vec::Vec;

pub(crate) struct SnapPreview {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub active: bool,
}

// WindowManager API v1.0 — STABLE
pub struct WindowManager {
    windows: Vec<AppWindow>,
    next_id: u64,
    focused: Option<u64>,
    dragging: Option<u64>,
    pub(crate) snap_preview: Option<SnapPreview>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            focused: None,
            dragging: None,
            snap_preview: None,
        }
    }

    /// Resolve a stable WindowId to the current position in the window list.
    fn find_index(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id.0)
    }

    /// WindowManager API v1.0
    pub fn create(&mut self, mut window: AppWindow) -> WindowId {
        window.id = self.next_id;
        let id = WindowId(window.id);
        self.next_id += 1;
        self.windows.push(window);
        self.focused = Some(id.0);
        id
    }

    /// WindowManager API v1.0
    pub fn close(&mut self, id: WindowId) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            if !w.closing {
                w.closing = true;
                w.animate_close();
                // Terminal windows: kill the shell and free the master fd —
                // otherwise sash orphans forever on the slave side. The
                // surface stays on the window so the shrink animation still
                // draws the terminal's last text.
                if let Some(fd) = w.detach_terminal_fd() {
                    if let Some(pid) = w.pid {
                        let _ = libsarga::process::kill(pid as i64, 9);
                    }
                    let _ = libsarga::io::close(fd);
                }
            }
        }
    }

    pub fn close_by_pid(&mut self, pid: u64) {
        if let Some(pos) = self.windows.iter().position(|w| w.pid == Some(pid)) {
            // App already exited (reaped): free the pty master fd.
            if let Some(fd) = self.windows[pos].take_terminal() {
                let _ = libsarga::io::close(fd);
            }
            let removed = self.windows.remove(pos).id;
            self.clear_refs(removed);
        }
    }

    fn clear_refs(&mut self, removed_id: u64) {
        if self.focused == Some(removed_id) {
            self.focused = None;
        }
        if self.dragging == Some(removed_id) {
            self.dragging = None;
        }
    }

    /// Remove windows whose close animation has completed.
    pub fn process_closing(&mut self) -> Vec<WindowId> {
        let mut closed = Vec::new();
        let mut i = self.windows.len();
        while i > 0 {
            i -= 1;
            if self.windows[i].closing && self.windows[i].anim.is_none() {
                let id = self.windows[i].id;
                closed.push(WindowId(id));
                self.windows.remove(i);
                self.clear_refs(id);
            }
        }
        closed
    }

    /// WindowManager API v1.0
    pub fn bring_to_front(&mut self, id: WindowId) {
        if let Some(i) = self.find_index(id) {
            let mut w = self.windows.remove(i);
            w.focused = true;
            for other in &mut self.windows {
                other.focused = false;
            }
            self.windows.push(w);
            self.focused = Some(id.0);
        }
    }

    /// WindowManager API v1.0
    pub fn minimize(&mut self, id: WindowId, _screen_w: u32, taskbar_h: u32) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            w.prev_state = w.state;
            w.state = WindowState::Minimized;
            let tab_x = layout::taskbar_btn_x(i);
            w.animate_to(
                tab_x as i32,
                (taskbar_h - layout::TASKBAR_BTN_H) as i32,
                layout::TASKBAR_BTN_W,
                layout::TASKBAR_BTN_H,
            );
        }
    }

    /// WindowManager API v1.0
    pub fn toggle_maximize(&mut self, id: WindowId, screen_w: u32, taskbar_h: u32) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            match w.state {
                WindowState::Maximized => {
                    w.animate_to(w.prev_x, w.prev_y, w.prev_w, w.prev_h);
                    w.state = WindowState::Normal;
                }
                _ => {
                    w.prev_x = w.x;
                    w.prev_y = w.y;
                    w.prev_w = w.w;
                    w.prev_h = w.h;
                    w.animate_to(0, 0, screen_w, taskbar_h);
                    w.state = WindowState::Maximized;
                }
            }
        }
    }

    /// WindowManager API v1.0
    pub fn toggle_fullscreen(&mut self, id: WindowId, screen_w: u32, screen_h: u32) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            match w.state {
                WindowState::Fullscreen => {
                    w.animate_to(w.prev_x, w.prev_y, w.prev_w, w.prev_h);
                    w.state = WindowState::Normal;
                }
                _ => {
                    w.prev_x = w.x;
                    w.prev_y = w.y;
                    w.prev_w = w.w;
                    w.prev_h = w.h;
                    w.animate_to(0, 0, screen_w, screen_h);
                    w.state = WindowState::Fullscreen;
                }
            }
        }
    }

    pub(crate) fn snap_to_region(
        &mut self,
        id: WindowId,
        region: SnapRegion,
        sw: u32,
        _sh: u32,
        tb_h: u32,
    ) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            let half_w = sw / 2;
            let half_h = tb_h / 2;
            let (tx, ty, tw, th) = match region {
                SnapRegion::Left => (0, 0, half_w, tb_h),
                SnapRegion::Right => (half_w as i32, 0, half_w, tb_h),
                SnapRegion::Top => (0, 0, sw, half_h),
                SnapRegion::Bottom => (0, half_h as i32, sw, half_h),
                SnapRegion::TopLeft => (0, 0, half_w, half_h),
                SnapRegion::TopRight => (half_w as i32, 0, half_w, half_h),
                SnapRegion::BottomLeft => (0, half_h as i32, half_w, half_h),
                SnapRegion::BottomRight => (half_w as i32, half_h as i32, half_w, half_h),
            };
            self.snap_preview = Some(SnapPreview {
                x: tx,
                y: ty,
                w: tw,
                h: th,
                active: true,
            });
            w.prev_x = w.x;
            w.prev_y = w.y;
            w.prev_w = w.w;
            w.prev_h = w.h;
            w.animate_to(tx, ty, tw, th);
            if w.state == WindowState::Maximized || w.state == WindowState::Fullscreen {
                w.state = WindowState::Normal;
            }
        }
    }

    /// Resize a window mid-drag. `origin` is the geometry at drag start,
    /// `edges` the edge flags being dragged (1 = left, 2 = right, 4 = bottom),
    /// and `(dx, dy)` the delta from the drag start point.
    pub fn resize_drag(&mut self, id: WindowId, origin: Rect, edges: u8, dx: i32, dy: i32) {
        let Some(w) = self.lookup_mut(id) else {
            return;
        };
        let (ox, oy, ow, oh) = (origin.x, origin.y, origin.w, origin.h);
        let mut nx = ox;
        let mut nw = ow;
        let mut nh = oh;
        if edges & 1 != 0 {
            nx = ox + dx;
            nw = (ow as i32 - dx) as u32;
        }
        if edges & 2 != 0 {
            nw = (ow as i32 + dx) as u32;
        }
        if edges & 4 != 0 {
            nh = (oh as i32 + dy) as u32;
        }
        if nw < layout::MIN_WIN_W {
            nw = layout::MIN_WIN_W;
            nx = ox + ow as i32 - layout::MIN_WIN_W as i32;
        }
        if nh < layout::MIN_WIN_H {
            nh = layout::MIN_WIN_H;
        }
        w.x = nx;
        w.y = oy;
        w.w = nw;
        w.h = nh;
    }

    pub fn show_snap_preview(&mut self, mx: i32, my: i32, sw: u32, _sh: u32, tb_h: u32) {
        let swi = sw as i32;
        let tb_hi = tb_h as i32;
        let half_w = sw / 2;
        let half_h = tb_h / 2;
        if mx < SNAP_MARGIN && my < SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: 0,
                y: 0,
                w: half_w,
                h: half_h,
                active: true,
            });
        } else if mx > swi - SNAP_MARGIN && my < SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: half_w as i32,
                y: 0,
                w: half_w,
                h: half_h,
                active: true,
            });
        } else if mx < SNAP_MARGIN && my > tb_hi - SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: 0,
                y: half_h as i32,
                w: half_w,
                h: half_h,
                active: true,
            });
        } else if mx > swi - SNAP_MARGIN && my > tb_hi - SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: half_w as i32,
                y: half_h as i32,
                w: half_w,
                h: half_h,
                active: true,
            });
        } else if mx < SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: 0,
                y: 0,
                w: half_w,
                h: tb_h,
                active: true,
            });
        } else if mx > swi - SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: half_w as i32,
                y: 0,
                w: half_w,
                h: tb_h,
                active: true,
            });
        } else if my < SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: 0,
                y: 0,
                w: sw,
                h: half_h,
                active: true,
            });
        } else if my > tb_hi - SNAP_MARGIN {
            self.snap_preview = Some(SnapPreview {
                x: 0,
                y: half_h as i32,
                w: sw,
                h: half_h,
                active: true,
            });
        } else {
            self.snap_preview = None;
        }
    }

    pub fn clear_snap_preview(&mut self) {
        self.snap_preview = None;
    }

    /// WindowManager API v1.0
    pub fn restore(&mut self, id: WindowId) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            let target = w.prev_state;
            if target == WindowState::Maximized {
                w.state = WindowState::Maximized;
            } else if target == WindowState::Fullscreen {
                w.state = WindowState::Fullscreen;
            } else {
                w.state = WindowState::Normal;
            }
        }
    }

    /// WindowManager API v1.0
    pub fn begin_drag(&mut self, id: WindowId, mx: i32, my: i32) {
        if let Some(i) = self.find_index(id) {
            let w = &mut self.windows[i];
            w.anim = None;
            w.dragging = true;
            w.drag_ox = mx - w.x;
            w.drag_oy = my - w.y;
            self.dragging = Some(id.0);
        }
    }

    /// WindowManager API v1.0
    pub fn update_drag(&mut self, mx: i32, my: i32) {
        if let Some(i) = self.dragging.and_then(|id| self.find_index(WindowId(id))) {
            if let Some(w) = self.windows.get_mut(i) {
                w.x = mx - w.drag_ox;
                w.y = my - w.drag_oy;
            }
        }
    }

    /// WindowManager API v1.0
    pub fn end_drag(&mut self) {
        if let Some(i) = self.dragging.and_then(|id| self.find_index(WindowId(id))) {
            if let Some(w) = self.windows.get_mut(i) {
                w.dragging = false;
            }
        }
        self.dragging = None;
    }

    /// WindowManager API v1.0
    pub fn iter(&self) -> &[AppWindow] {
        &self.windows
    }

    pub fn iter_mut(&mut self) -> &mut [AppWindow] {
        &mut self.windows
    }

    /// WindowManager API v1.0
    pub fn lookup(&self, id: WindowId) -> Option<&AppWindow> {
        self.find_index(id).and_then(|i| self.windows.get(i))
    }

    /// WindowManager API v1.0
    pub fn lookup_mut(&mut self, id: WindowId) -> Option<&mut AppWindow> {
        self.find_index(id).and_then(|i| self.windows.get_mut(i))
    }

    /// WindowManager API v1.0
    pub fn active(&self) -> Option<WindowId> {
        self.focused.map(WindowId)
    }

    /// Stable id of the window at list position `i` (for positional loops).
    pub fn id_at(&self, i: usize) -> Option<WindowId> {
        self.windows.get(i).map(|w| WindowId(w.id))
    }

    /// Current list position of a window by stable id.
    pub fn position_of(&self, id: WindowId) -> Option<usize> {
        self.find_index(id)
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn focused_mut(&mut self) -> Option<&mut AppWindow> {
        self.focused
            .and_then(|id| self.find_index(WindowId(id)))
            .and_then(|i| self.windows.get_mut(i))
    }

    pub fn focus_next(&mut self) -> bool {
        if self.windows.is_empty() {
            return false;
        }
        for w in &mut self.windows {
            w.focused = false;
        }
        let idx = self
            .focused
            .and_then(|id| self.find_index(WindowId(id)))
            .unwrap_or(0);
        let n = (idx + 1) % self.windows.len();
        self.windows[n].focused = true;
        self.focused = Some(self.windows[n].id);
        true
    }
}
