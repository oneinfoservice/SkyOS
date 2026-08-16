//! Explorer — file manager application.

use crate::sys::vfs::VfsEntry;
use alloc::string::String;
use alloc::vec::Vec;
use libsarga::theme::Theme;

#[derive(Clone)]
pub(crate) struct ExplorerTab {
    pub path: String,
    pub entries: Vec<VfsEntry>,
    pub sort_by: u8, // 0=name,1=size,2=date,3=type
    pub sort_desc: bool,
    pub view_mode: u8, // 0=list,1=grid,2=icon,3=details
    pub scroll: u32,
    pub sel_idx: Option<usize>,
}

pub(crate) struct ExplorerState {
    pub id: u32,
    pub tabs: Vec<ExplorerTab>,
    pub active_tab: usize,
    pub history: Vec<String>,
    pub history_idx: i32,
    pub path: String,
    pub search_query: String,
    pub search_results: Vec<VfsEntry>,
    pub search_active: bool,
    pub favorites: Vec<String>,
    pub show_preview: bool,
    pub preview_content: String,
}

impl ExplorerState {
    pub fn new(id: u32, start_path: &str) -> Self {
        let path = String::from(start_path);
        let history = alloc::vec![path.clone()];
        let tab = ExplorerTab {
            path: path.clone(),
            entries: Vec::new(),
            sort_by: 0,
            sort_desc: false,
            view_mode: 0,
            scroll: 0,
            sel_idx: None,
        };
        ExplorerState {
            id,
            tabs: alloc::vec![tab],
            active_tab: 0,
            history,
            history_idx: 0,
            path,
            search_query: String::new(),
            search_results: Vec::new(),
            search_active: false,
            favorites: Vec::new(),
            show_preview: false,
            preview_content: String::new(),
        }
    }

    pub fn active_ref(&self) -> &ExplorerTab {
        &self.tabs[self.active_tab]
    }

    pub fn refresh(&mut self) {
        let path = self.tabs[self.active_tab].path.clone();
        let mut entries = crate::sys::vfs::VfsContext::list_dir_static(&path);
        sort_entries(
            &mut entries,
            self.tabs[self.active_tab].sort_by,
            self.tabs[self.active_tab].sort_desc,
        );
        self.tabs[self.active_tab].entries = entries;
        self.tabs[self.active_tab].sel_idx = None;
    }

    pub fn set_sort(&mut self, by: u8) {
        let tab = &mut self.tabs[self.active_tab];
        if tab.sort_by == by {
            tab.sort_desc = !tab.sort_desc;
        } else {
            tab.sort_by = by;
            tab.sort_desc = false;
        }
        let desc = tab.sort_desc;
        sort_entries(&mut tab.entries, by, desc);
    }

    pub fn navigate(&mut self, path: &str) {
        let p = String::from(path);
        self.path = p.clone();
        self.tabs[self.active_tab].path = p.clone();
        self.history.truncate((self.history_idx + 1) as usize);
        self.history.push(p);
        self.history_idx = self.history.len() as i32 - 1;
        self.refresh();
    }

    pub fn go_back(&mut self) {
        if self.history_idx > 0 {
            self.history_idx -= 1;
            let p = self.history[self.history_idx as usize].clone();
            self.path = p.clone();
            self.tabs[self.active_tab].path = p;
            self.refresh();
        }
    }

    pub fn go_forward(&mut self) {
        if (self.history_idx as usize) < self.history.len() - 1 {
            self.history_idx += 1;
            let p = self.history[self.history_idx as usize].clone();
            self.path = p.clone();
            self.tabs[self.active_tab].path = p;
            self.refresh();
        }
    }

    pub fn go_up(&mut self) {
        let cur = &self.tabs[self.active_tab].path;
        if cur == "/" {
            return;
        }
        let parent = if cur.ends_with('/') {
            let trimmed = cur.trim_end_matches('/');
            let slash = trimmed.rfind('/').unwrap_or(0);
            if slash == 0 {
                String::from("/")
            } else {
                String::from(&trimmed[..slash])
            }
        } else {
            let slash = cur.rfind('/').unwrap_or(0);
            if slash == 0 {
                String::from("/")
            } else {
                String::from(&cur[..slash])
            }
        };
        self.navigate(&parent);
    }

    pub fn home(&mut self) {
        self.navigate("/home");
    }

    pub fn status_text(&self) -> String {
        let tab = self.active_tab;
        let count = if self.search_active {
            self.search_results.len()
        } else {
            self.tabs[tab].entries.len()
        };
        let sel = self.tabs[tab].sel_idx.map(|_| "1 selected").unwrap_or("");
        alloc::format!("{} items  {}", count, sel)
    }
}

pub(crate) fn sort_entries(entries: &mut [VfsEntry], by: u8, desc: bool) {
    fn cmp_name(a: &VfsEntry, b: &VfsEntry) -> core::cmp::Ordering {
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    }
    match by {
        0 => entries.sort_by(|a, b| if desc { cmp_name(b, a) } else { cmp_name(a, b) }),
        1 => entries.sort_by(|a, b| {
            let ord = a.size.cmp(&b.size);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        }),
        2 => entries.sort_by(|a, b| {
            let ord = a.modified.cmp(&b.modified);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        }),
        3 => entries.sort_by(|a, b| {
            let ext_a = a.name.rfind('.').map(|i| &a.name[i..]).unwrap_or("");
            let ext_b = b.name.rfind('.').map(|i| &b.name[i..]).unwrap_or("");
            let ord = ext_a.cmp(ext_b);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        }),
        _ => {}
    }
    // dirs before files
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir));
}

fn format_size(s: u64) -> alloc::string::String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if s >= GB {
        alloc::format!("{}.{} GB", s / GB, (s % GB) / (GB / 10))
    } else if s >= MB {
        alloc::format!("{}.{} MB", s / MB, (s % MB) / (MB / 10))
    } else if s >= KB {
        alloc::format!("{}.{} KB", s / KB, (s % KB) / (KB / 10))
    } else {
        alloc::format!("{} B", s)
    }
}

fn format_date(ts: u64) -> alloc::string::String {
    let days = ts / 86400;
    let (y, m, d) = libsarga::time::civil_from_days(days);
    alloc::format!("{}-{:02}-{:02}", y, m, d)
}

fn icon_for_entry(e: &VfsEntry) -> &'static str {
    if e.is_dir {
        return "\u{25B6}";
    } // ▶
    if let Some(dot) = e.name.rfind('.') {
        let ext = &e.name[dot..];
        match ext {
            ".txt" | ".md" | ".rs" | ".c" | ".h" | ".py" | ".js" | ".toml" => "\u{1F4C4}",
            ".png" | ".jpg" | ".jpeg" | ".gif" | ".bmp" | ".svg" => "\u{1F5BC}",
            ".mp3" | ".wav" | ".flac" | ".ogg" => "\u{1F3B5}",
            ".mp4" | ".avi" | ".mkv" => "\u{1F3AC}",
            ".zip" | ".tar" | ".gz" | ".7z" | ".rar" => "\u{1F4E6}",
            ".pdf" => "\u{1F4D5}",
            _ => "\u{1F4C4}",
        }
    } else {
        "\u{1F4C4}"
    }
}

pub(crate) fn draw_explorer_content(
    canvas: &mut crate::render::compositor::Canvas,
    theme: &Theme,
    aw: &crate::core::window::AppWindow,
    explorers: &[ExplorerState],
    exp_id: u32,
) {
    let state = match explorers.iter().find(|e| e.id == exp_id) {
        Some(s) => s,
        None => return,
    };
    let tab = state.active_ref();

    let mut y = aw.y as u32 + 29;
    let bottom = aw.y as u32 + aw.h - 4;
    let area_x = aw.x as u32 + 2;
    let area_w = aw.w - 4;

    // Toolbar with nav buttons + address bar
    canvas.fill_rect(area_x, y, area_w, 26, theme.bg_elevated);
    // Nav buttons
    canvas.draw_string(area_x + 2, y + 5, "<", theme.text, 0);
    canvas.draw_string(area_x + 20, y + 5, ">", theme.text, 0);
    canvas.draw_string(area_x + 38, y + 5, "^", theme.text, 0);
    canvas.draw_string(area_x + 56, y + 5, "~", theme.text, 0);
    // Address bar
    let addr_x = area_x + 80;
    let addr_w = area_w - 160;
    canvas.draw_rounded_rect(addr_x, y + 3, addr_w, 20, 4, theme.bg_surface);
    let display_path = if tab.path.len() > (addr_w / 8) as usize {
        &tab.path[tab.path.len().saturating_sub((addr_w / 8) as usize)..]
    } else {
        &tab.path
    };
    canvas.draw_string(addr_x + 4, y + 6, display_path, theme.text_secondary, 0);
    // Sort type view control text
    let view_names = ["List", "Grid", "Icon", "Detl"];
    canvas.draw_string(
        addr_x + addr_w + 30,
        y + 5,
        view_names[tab.view_mode as usize],
        theme.text_secondary,
        0,
    );
    y += 28;

    // Search bar (when search is active)
    if state.search_active {
        canvas.fill_rect(area_x, y, area_w, 20, theme.bg_surface);
        canvas.draw_string(
            area_x + 4,
            y + 4,
            &alloc::format!(
                "Search: {} ({} results)",
                state.search_query,
                state.search_results.len()
            ),
            theme.accent,
            0,
        );
        y += 22;
    }

    let fav_width = 100u32; // favorites sidebar
    let preview_width = if state.show_preview { area_w / 3 } else { 0 };
    let main_width = if area_w > fav_width + preview_width {
        area_w - fav_width - preview_width
    } else {
        area_w
    };

    // Favorites sidebar
    let fav_x = area_x;
    if !state.favorites.is_empty() {
        canvas.fill_rect(fav_x, y, fav_width, bottom - y, theme.bg_surface);
        canvas.draw_string(fav_x + 4, y + 2, "Favorites", theme.accent, 0);
        let mut fy = y + 20;
        for fav in &state.favorites {
            let short = if fav.len() > 12 {
                &fav[fav.len() - 12..]
            } else {
                fav
            };
            if fy < bottom {
                canvas.draw_string(fav_x + 6, fy, short, theme.text_secondary, 0);
                fy += 14;
            }
        }
    }
    let content_x = fav_x + fav_width;
    let content_width = main_width;

    // Column headers (list/details views)
    if tab.view_mode == 0 || tab.view_mode == 3 {
        let cols = match tab.view_mode {
            0 => &["Name", "Size", "Date", "Type"][..],
            _ => &["Name", "Size", "Date", "Type", "Modified", "Path"][..],
        };
        let col_ws = [
            content_width * 35 / 100,
            content_width * 18 / 100,
            content_width * 20 / 100,
            content_width * 27 / 100,
        ];
        canvas.fill_rect(content_x, y, content_width, 20, theme.bg_surface);
        let mut cx = content_x + 6;
        for (i, (col, cw)) in cols.iter().zip(col_ws.iter()).enumerate() {
            let is_sort = (tab.sort_by as usize) == i;
            canvas.draw_string(
                cx,
                y + 4,
                col,
                if is_sort {
                    theme.accent
                } else {
                    theme.text_secondary
                },
                0,
            );
            cx += cw;
        }
        y += 22;
    }

    // entries (search results or dir listing)
    let show = if state.search_active {
        &state.search_results[..]
    } else {
        &tab.entries[..]
    };
    let item_h = if tab.view_mode == 1 {
        64u32
    } else if tab.view_mode == 2 {
        36u32
    } else {
        18u32
    };
    let available_h = bottom.saturating_sub(y);
    if !show.is_empty() && item_h > 0 {
        let max_vis = (available_h / item_h) as usize;
        let start_idx = tab.scroll as usize;
        let end = core::cmp::min(start_idx + max_vis, show.len());
        let visible = &show[start_idx..end];

        match tab.view_mode {
            0 => draw_list(
                canvas,
                theme,
                aw,
                content_x,
                content_width,
                y,
                item_h,
                visible,
                start_idx,
                tab,
            ),
            1 => draw_grid(
                canvas,
                theme,
                aw,
                content_x,
                content_width,
                y,
                item_h,
                visible,
                start_idx,
                tab,
            ),
            2 => draw_icon(
                canvas,
                theme,
                aw,
                content_x,
                content_width,
                y,
                item_h,
                visible,
                start_idx,
                tab,
            ),
            3 => draw_details(
                canvas,
                theme,
                aw,
                content_x,
                content_width,
                y,
                item_h,
                visible,
                start_idx,
                tab,
            ),
            _ => draw_list(
                canvas,
                theme,
                aw,
                content_x,
                content_width,
                y,
                item_h,
                visible,
                start_idx,
                tab,
            ),
        }
    }

    // Preview panel
    if state.show_preview && !state.preview_content.is_empty() {
        let pv_x = content_x + content_width;
        let pv_w = preview_width;
        canvas.fill_rect(
            pv_x,
            aw.y as u32 + 57,
            pv_w,
            bottom - aw.y as u32 - 57,
            theme.bg_surface,
        );
        canvas.draw_string(pv_x + 4, aw.y as u32 + 60, "Preview", theme.accent, 0);
        let mut py = aw.y as u32 + 72;
        for line in state.preview_content.lines().take(20) {
            if py >= bottom {
                break;
            }
            let display = if line.len() > (pv_w as usize / 2) {
                &line[..(pv_w as usize / 2)]
            } else {
                line
            };
            canvas.draw_string(pv_x + 4, py, display, theme.text_secondary, 0);
            py += 14;
        }
    }

    // Status bar
    let status_str = state.status_text();
    let sb_y = bottom - 16;
    canvas.fill_rect(area_x, sb_y, area_w, 16, theme.bg_elevated);
    canvas.draw_string(area_x + 4, sb_y + 2, &status_str, theme.text_secondary, 0);
}

/// Handle mouse click on an explorer window's content area.
/// Returns true if the click was consumed.
pub(crate) fn handle_explorer_click(
    state: &mut ExplorerState,
    mx: i32,
    my: i32,
    aw: &crate::core::window::AppWindow,
    double_click: bool,
) -> bool {
    let area_x = aw.x as u32 + 2;
    let area_w = aw.w - 4;
    let area_x_i = area_x as i32;

    // Toolbar area: nav buttons
    let toolbar_top = aw.y + 29;
    let toolbar_bot = toolbar_top + 26;
    if my >= toolbar_top && my < toolbar_bot {
        if mx >= area_x_i && mx < area_x_i + 24 {
            state.go_back();
            return true;
        }
        if mx >= area_x_i + 24 && mx < area_x_i + 48 {
            state.go_forward();
            return true;
        }
        if mx >= area_x_i + 48 && mx < area_x_i + 72 {
            state.go_up();
            return true;
        }
        if mx >= area_x_i + 72 && mx < area_x_i + 96 {
            state.home();
            return true;
        }
        if mx >= area_x_i + 96 && mx < area_x_i + 120 {
            state.refresh();
            return true;
        }
        return false;
    }

    // Column headers (list/details mode only)
    let header_top = toolbar_bot;
    let header_bot = header_top + 20;
    let is_list_like = state.active_ref().view_mode == 0 || state.active_ref().view_mode == 3;
    if is_list_like && my >= header_top && my < header_bot {
        let col_ws = [
            area_w * 35 / 100,
            area_w * 18 / 100,
            area_w * 20 / 100,
            area_w * 27 / 100,
        ];
        let mut cx = area_x;
        for col_idx in 0..4u8 {
            cx += col_ws[col_idx as usize];
            if mx < cx as i32 {
                state.set_sort(col_idx);
                return true;
            }
        }
        return false;
    }

    // Entry list area
    let list_top = if is_list_like {
        header_bot
    } else {
        toolbar_bot
    };
    let item_h = if state.active_ref().view_mode == 1 {
        64u32
    } else if state.active_ref().view_mode == 2 {
        36u32
    } else {
        18u32
    };
    if my >= list_top && item_h > 0 {
        let rel_y = (my - list_top) as u32;
        let idx = (rel_y / item_h) as usize;
        let tab_idx = state.active_tab;
        if idx < state.tabs[tab_idx].entries.len() {
            let actual_idx = idx + state.tabs[tab_idx].scroll as usize;
            if actual_idx < state.tabs[tab_idx].entries.len() {
                if double_click {
                    let is_dir = state.tabs[tab_idx].entries[actual_idx].is_dir;
                    let path = state.tabs[tab_idx].entries[actual_idx].path.clone();
                    if is_dir {
                        state.navigate(&path);
                    }
                } else {
                    state.tabs[tab_idx].sel_idx = Some(actual_idx);
                }
                return true;
            }
        }
        // Click in empty area — deselect
        state.tabs[tab_idx].sel_idx = None;
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments)] // draw helper; param shape fixed by all call sites
fn draw_list(
    canvas: &mut crate::render::compositor::Canvas,
    theme: &Theme,
    _aw: &crate::core::window::AppWindow,
    area_x: u32,
    area_w: u32,
    y: u32,
    item_h: u32,
    visible: &[VfsEntry],
    start_idx: usize,
    tab: &ExplorerTab,
) {
    let col_ws = [
        area_w * 35 / 100,
        area_w * 18 / 100,
        area_w * 20 / 100,
        area_w * 27 / 100,
    ];
    for (i, entry) in visible.iter().enumerate() {
        let abs_idx = start_idx + i;
        let ey = y + (i as u32) * item_h;
        if Some(abs_idx) == tab.sel_idx {
            canvas.fill_rect(
                area_x,
                ey,
                area_w,
                item_h,
                (theme.accent & 0x00FFFFFF) | 0x40000000,
            );
        }
        canvas.draw_string(area_x + 4, ey + 1, icon_for_entry(entry), theme.text, 0);
        let name = if entry.name.len() > 28 {
            &entry.name[..28]
        } else {
            &entry.name
        };
        canvas.draw_string(
            area_x + 16,
            ey + 1,
            name,
            if entry.is_dir {
                theme.accent
            } else {
                theme.text
            },
            0,
        );
        let size_str = format_size(entry.size);
        canvas.draw_string(
            area_x + col_ws[0] + 4,
            ey + 1,
            &size_str,
            theme.text_secondary,
            0,
        );
        let date_str = format_date(entry.modified);
        canvas.draw_string(
            area_x + col_ws[0] + col_ws[1] + 4,
            ey + 1,
            &date_str,
            theme.text_secondary,
            0,
        );
    }
}

#[allow(clippy::too_many_arguments)] // draw helper; param shape fixed by all call sites
fn draw_grid(
    canvas: &mut crate::render::compositor::Canvas,
    theme: &Theme,
    _aw: &crate::core::window::AppWindow,
    area_x: u32,
    area_w: u32,
    y: u32,
    item_h: u32,
    visible: &[VfsEntry],
    start_idx: usize,
    tab: &ExplorerTab,
) {
    let cols = (area_w / 80).max(1);
    let cell_w = area_w / cols;
    for (i, entry) in visible.iter().enumerate() {
        let abs_idx = start_idx + i;
        let col = i as u32 % cols;
        let row = i as u32 / cols;
        let gx = area_x + col * cell_w + 2;
        let gy = y + row * item_h;
        if Some(abs_idx) == tab.sel_idx {
            canvas.fill_rect(
                gx,
                gy,
                cell_w - 4,
                item_h - 2,
                (theme.accent & 0x00FFFFFF) | 0x40000000,
            );
        }
        canvas.draw_string(gx + 4, gy + 4, icon_for_entry(entry), theme.text, 0);
        let name = if entry.name.len() > 10 {
            &entry.name[..10]
        } else {
            &entry.name
        };
        canvas.draw_string(gx + 4, gy + 22, name, theme.text_secondary, 0);
    }
}

#[allow(clippy::too_many_arguments)] // draw helper; param shape fixed by all call sites
fn draw_icon(
    canvas: &mut crate::render::compositor::Canvas,
    theme: &Theme,
    _aw: &crate::core::window::AppWindow,
    area_x: u32,
    area_w: u32,
    y: u32,
    item_h: u32,
    visible: &[VfsEntry],
    start_idx: usize,
    tab: &ExplorerTab,
) {
    let col_ws = [
        area_w * 35 / 100,
        area_w * 18 / 100,
        area_w * 20 / 100,
        area_w * 27 / 100,
    ];
    for (i, entry) in visible.iter().enumerate() {
        let abs_idx = start_idx + i;
        let ey = y + (i as u32) * item_h;
        if Some(abs_idx) == tab.sel_idx {
            canvas.fill_rect(
                area_x,
                ey,
                area_w,
                item_h,
                (theme.accent & 0x00FFFFFF) | 0x40000000,
            );
        }
        canvas.draw_string(area_x + 4, ey + 6, icon_for_entry(entry), theme.text, 0);
        let name = if entry.name.len() > 28 {
            &entry.name[..28]
        } else {
            &entry.name
        };
        canvas.draw_string(
            area_x + 28,
            ey + 6,
            name,
            if entry.is_dir {
                theme.accent
            } else {
                theme.text
            },
            0,
        );
        let size_str = format_size(entry.size);
        canvas.draw_string(
            area_x + col_ws[0] + 4,
            ey + 6,
            &size_str,
            theme.text_secondary,
            0,
        );
        let date_str = format_date(entry.modified);
        canvas.draw_string(
            area_x + col_ws[0] + col_ws[1] + 4,
            ey + 6,
            &date_str,
            theme.text_secondary,
            0,
        );
    }
}

#[allow(clippy::too_many_arguments)] // draw helper; param shape fixed by all call sites
fn draw_details(
    canvas: &mut crate::render::compositor::Canvas,
    theme: &Theme,
    _aw: &crate::core::window::AppWindow,
    area_x: u32,
    area_w: u32,
    y: u32,
    item_h: u32,
    visible: &[VfsEntry],
    start_idx: usize,
    tab: &ExplorerTab,
) {
    let col_ws = [
        area_w * 30 / 100,
        area_w * 12 / 100,
        area_w * 14 / 100,
        area_w * 14 / 100,
        area_w * 14 / 100,
        area_w * 16 / 100,
    ];
    for (i, entry) in visible.iter().enumerate() {
        let abs_idx = start_idx + i;
        let ey = y + (i as u32) * item_h;
        if Some(abs_idx) == tab.sel_idx {
            canvas.fill_rect(
                area_x,
                ey,
                area_w,
                item_h,
                (theme.accent & 0x00FFFFFF) | 0x40000000,
            );
        }
        let mut cx = area_x + 4;
        canvas.draw_string(cx, ey + 1, icon_for_entry(entry), theme.text, 0);
        let name = if entry.name.len() > 22 {
            &entry.name[..22]
        } else {
            &entry.name
        };
        canvas.draw_string(cx + 14, ey + 1, name, theme.text, 0);
        cx += col_ws[0];
        let size_str = format_size(entry.size);
        canvas.draw_string(cx, ey + 1, &size_str, theme.text_secondary, 0);
        cx += col_ws[1];
        let date_str = format_date(entry.modified);
        canvas.draw_string(cx, ey + 1, &date_str, theme.text_secondary, 0);
        cx += col_ws[2];
        // type
        let ext = entry
            .name
            .rfind('.')
            .map(|i| &entry.name[i..])
            .unwrap_or("");
        canvas.draw_string(cx, ey + 1, ext, theme.text_secondary, 0);
        cx += col_ws[3];
        // modified time
        canvas.draw_string(cx, ey + 1, &date_str, theme.text_secondary, 0);
        cx += col_ws[4];
        // brief path
        if let Some(slash) = entry.path.rfind('/') {
            let parent = &entry.path[..slash];
            let short = if parent.len() > 12 {
                &parent[parent.len() - 12..]
            } else {
                parent
            };
            canvas.draw_string(cx, ey + 1, short, theme.text_disabled, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: u64, modified: u64) -> VfsEntry {
        VfsEntry {
            name: String::from(name),
            path: String::from(name),
            is_dir,
            size,
            modified,
        }
    }

    #[test]
    fn sort_entries_dirs_first_then_name() {
        let mut v = vec![
            entry("b.txt", false, 1, 1),
            entry("a_dir", true, 9, 9),
            entry("A.txt", false, 2, 2),
        ];
        sort_entries(&mut v, 0, false);
        assert!(v[0].is_dir);
        assert_eq!(v[1].name, "A.txt"); // case-insensitive: a.txt before b.txt
        assert_eq!(v[2].name, "b.txt");
    }

    #[test]
    fn sort_entries_by_size_and_date() {
        let mut v = vec![
            entry("big", false, 300, 100),
            entry("small", false, 1, 900),
            entry("mid", false, 200, 500),
        ];
        sort_entries(&mut v, 1, false);
        assert_eq!(v[0].name, "small");
        assert_eq!(v[2].name, "big");
        sort_entries(&mut v, 1, true);
        assert_eq!(v[0].name, "big");
        sort_entries(&mut v, 2, true);
        assert_eq!(v[0].name, "small"); // newest modified first
    }

    #[test]
    fn sort_entries_by_extension() {
        let mut v = vec![
            entry("a.rs", false, 0, 0),
            entry("b.txt", false, 0, 0),
            entry("c.rs", false, 0, 0),
        ];
        sort_entries(&mut v, 3, false);
        assert_eq!(v[0].name, "a.rs");
        assert_eq!(v[1].name, "c.rs");
        assert_eq!(v[2].name, "b.txt");
    }

    #[test]
    fn sort_entries_unknown_mode_is_noop() {
        let mut v = vec![entry("b", false, 0, 0), entry("a", false, 0, 0)];
        sort_entries(&mut v, 99, false);
        assert_eq!(v[0].name, "b"); // unchanged (dirs-first is still applied, both files)
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(
            format_size(1024 * 1024 * 1024 + 150 * 1024 * 1024),
            "1.1 GB"
        );
    }

    #[test]
    fn format_date_epoch_and_day() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(86400), "1970-01-02");
    }

    #[test]
    fn status_text_empty_and_selected() {
        let mut st = ExplorerState::new(1, "/home");
        assert_eq!(st.status_text(), "0 items  ");
        st.tabs[st.active_tab].sel_idx = Some(0);
        assert_eq!(st.status_text(), "0 items  1 selected");
        st.search_active = true;
        st.search_results = vec![entry("x", false, 1, 1)];
        assert_eq!(st.status_text(), "1 items  1 selected");
    }
}
