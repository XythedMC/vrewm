use std::{collections::HashMap, ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Tiling,
    TreeView,
}

/// A window with its position on the infinite canvas and its place in the window tree.
pub struct CanvasWindow {
    pub id: u32,
    pub window: Window,
    /// Current animated position (lerped each frame toward target).
    pub canvas_x: f64,
    pub canvas_y: f64,
    /// Destination position set by layout functions.
    pub target_x: f64,
    pub target_y: f64,
    /// Snapshot of canvas position when the current animation began.
    pub(crate) anim_start_x: f64,
    pub(crate) anim_start_y: f64,
    /// Last known canvas position while in tree view (free-form). Restored on re-entry.
    pub tree_saved_x: Option<f64>,
    pub tree_saved_y: Option<f64>,
    /// None means this window is a tree root.
    pub parent_id: Option<u32>,
    /// IDs of direct children, in open order.
    pub children: Vec<u32>,
}

pub struct Treewm {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub windows: Vec<CanvasWindow>,
    pub loop_signal: LoopSignal,

    pub viewport_x: f64,
    pub viewport_y: f64,
    /// Animated viewport destination.
    pub viewport_target_x: f64,
    pub viewport_target_y: f64,
    pub(crate) viewport_anim_start_x: f64,
    pub(crate) viewport_anim_start_y: f64,
    /// When Some, a 200 ms linear animation is in progress.
    pub anim_start: Option<Instant>,

    pub next_window_id: u32,
    /// The ID of the window that currently holds keyboard focus.
    pub focused_window_id: Option<u32>,
    pub view_mode: ViewMode,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Treewm>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,

    pub seat: Seat<Self>,
}

impl Treewm {
    pub fn new(event_loop: &mut EventLoop<Self>, display: Display<Self>) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        seat.add_pointer();

        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        Self {
            start_time,
            display_handle: dh,
            space,
            windows: Vec::new(),
            loop_signal,
            viewport_x: 0.0,
            viewport_y: 0.0,
            viewport_target_x: 0.0,
            viewport_target_y: 0.0,
            viewport_anim_start_x: 0.0,
            viewport_anim_start_y: 0.0,
            anim_start: None,
            next_window_id: 0,
            focused_window_id: None,
            view_mode: ViewMode::Tiling,
            socket_name,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            seat,
        }
    }

    fn init_wayland_listener(
        display: Display<Treewm>,
        event_loop: &mut EventLoop<Self>,
    ) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init wayland socket");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    // ── IDs ────────────────────────────────────────────────────────────────

    pub fn alloc_id(&mut self) -> u32 {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    // ── Focus ──────────────────────────────────────────────────────────────

    /// Set keyboard focus to the window with this ID and update our tracking field.
    pub fn focus_by_id(&mut self, id: u32) {
        self.focused_window_id = Some(id);
        let serial = SERIAL_COUNTER.next_serial();
        let surface = self
            .windows
            .iter()
            .find(|cw| cw.id == id)
            .and_then(|cw| cw.window.toplevel().map(|t| t.wl_surface().clone()));
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, surface, serial);
    }

    /// Clear keyboard focus.
    pub fn focus_clear(&mut self) {
        self.focused_window_id = None;
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
    }

    // ── Tree queries ───────────────────────────────────────────────────────

    /// Siblings of `id` in tree order (includes `id` itself).
    /// For roots the siblings are all other roots.
    pub fn siblings_of(&self, id: u32) -> Vec<u32> {
        let parent_id = self
            .windows
            .iter()
            .find(|cw| cw.id == id)
            .and_then(|cw| cw.parent_id);

        match parent_id {
            Some(pid) => self
                .windows
                .iter()
                .find(|cw| cw.id == pid)
                .map(|cw| cw.children.clone())
                .unwrap_or_default(),
            None => self
                .windows
                .iter()
                .filter(|cw| cw.parent_id.is_none())
                .map(|cw| cw.id)
                .collect(),
        }
    }

    // ── Layout ────────────────────────────────────────────────────────────

    pub fn apply_layout(&mut self) {
        match self.view_mode {
            ViewMode::Tiling => self.layout_tiling(),
            ViewMode::TreeView => self.layout_tree(),
        }
    }

    fn output_size(&self) -> (f64, f64) {
        self.space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| (g.size.w as f64, g.size.h as f64))
            .unwrap_or((1920.0, 1080.0))
    }

    fn layout_tiling(&mut self) {
        let Some(focused_id) = self.focused_window_id else { return };
        let (w, h) = self.output_size();

        let children_ids: Vec<u32> = self
            .windows
            .iter()
            .find(|cw| cw.id == focused_id)
            .map(|cw| cw.children.clone())
            .unwrap_or_default();

        let has_children = !children_ids.is_empty();
        let full_w = w as i32;
        let full_h = h as i32;
        let left_w = full_w / 2;
        let right_w = full_w - left_w;
        let n = children_ids.len().max(1);
        let child_h = full_h / n as i32;

        // Collect geometry for each window (two-pass to satisfy borrow checker).
        struct Slot {
            target_x: f64,
            target_y: f64,
            size: Option<(i32, i32)>,
        }
        let slots: Vec<(u32, Slot)> = self
            .windows
            .iter()
            .map(|cw| {
                let slot = if cw.id == focused_id {
                    let w = if has_children { left_w } else { full_w };
                    Slot { target_x: 0.0, target_y: 0.0, size: Some((w, full_h)) }
                } else if let Some(idx) = children_ids.iter().position(|&c| c == cw.id) {
                    Slot {
                        target_x: left_w as f64,
                        target_y: (child_h * idx as i32) as f64,
                        size: Some((right_w, child_h)),
                    }
                } else {
                    Slot { target_x: -10_000.0, target_y: 0.0, size: None }
                };
                (cw.id, slot)
            })
            .collect();

        self.viewport_target_x = 0.0;
        self.viewport_target_y = 0.0;

        // Resize windows (two-pass: collect toplevels, then configure).
        let resize_ops: Vec<(u32, i32, i32)> = slots
            .iter()
            .filter_map(|(id, slot)| slot.size.map(|(sw, sh)| (*id, sw, sh)))
            .collect();
        for (id, sw, sh) in resize_ops {
            let toplevel = self
                .windows
                .iter()
                .find(|cw| cw.id == id)
                .and_then(|cw| cw.window.toplevel());
            if let Some(tl) = toplevel {
                tl.with_pending_state(|s| { s.size = Some((sw, sh).into()); });
                tl.send_configure();
            }
        }

        // Set animation targets.
        for (id, slot) in slots {
            if let Some(cw) = self.windows.iter_mut().find(|cw| cw.id == id) {
                cw.target_x = slot.target_x;
                cw.target_y = slot.target_y;
            }
        }

        self.begin_animation();
    }

    fn layout_tree(&mut self) {
        const WIN_W: f64 = 800.0;
        const WIN_H: f64 = 600.0;
        const LEVEL_H: f64 = 400.0;
        const GAP: f64 = 80.0;

        let widths = self.compute_subtree_widths();

        let roots: Vec<u32> = self
            .windows
            .iter()
            .filter(|cw| cw.parent_id.is_none())
            .map(|cw| cw.id)
            .collect();

        if roots.is_empty() {
            return;
        }

        let total_w: f64 = roots.iter().map(|&id| widths[&id]).sum::<f64>()
            + GAP * (roots.len() - 1) as f64;

        let mut alloc_x = -total_w / 2.0;
        let mut positions: Vec<(u32, f64, f64)> = Vec::new();

        for &root in &roots {
            self.collect_positions(root, alloc_x, 0.0, LEVEL_H, GAP, WIN_W, &widths, &mut positions);
            alloc_x += widths[&root] + GAP;
        }

        // Set animation targets.
        for (id, cx, cy) in positions {
            if let Some(cw) = self.windows.iter_mut().find(|cw| cw.id == id) {
                cw.target_x = cx;
                cw.target_y = cy;
            }
        }

        // Resize every window to the default tree size.
        let toplevels: Vec<_> = self
            .windows
            .iter()
            .filter_map(|cw| cw.window.toplevel().cloned())
            .collect();
        for tl in toplevels {
            tl.with_pending_state(|s| {
                s.size = Some((WIN_W as i32, WIN_H as i32).into());
            });
            tl.send_configure();
        }

        // Set viewport target to center on focused window.
        if let Some(fid) = self.focused_window_id {
            if let Some(cw) = self.windows.iter().find(|cw| cw.id == fid) {
                let (sw, sh) = self.output_size();
                self.viewport_target_x = cw.target_x - sw / 2.0 + WIN_W / 2.0;
                self.viewport_target_y = cw.target_y - sh / 2.0 + WIN_H / 2.0;
            }
        }

        self.begin_animation();
    }

    fn compute_subtree_widths(&self) -> HashMap<u32, f64> {
        const WIN_W: f64 = 800.0;
        const GAP: f64 = 80.0;

        let mut widths: HashMap<u32, f64> = HashMap::new();
        let mut remaining: Vec<u32> = self.windows.iter().map(|cw| cw.id).collect();

        loop {
            let before = remaining.len();
            remaining.retain(|&id| {
                let Some(cw) = self.windows.iter().find(|cw| cw.id == id) else {
                    return false;
                };
                if !cw.children.iter().all(|c| widths.contains_key(c)) {
                    return true;
                }
                let children_w: f64 = cw.children.iter().map(|c| widths[c]).sum();
                let gaps = if cw.children.len() > 1 {
                    GAP * (cw.children.len() - 1) as f64
                } else {
                    0.0
                };
                widths.insert(id, (children_w + gaps).max(WIN_W));
                false
            });
            if remaining.is_empty() || remaining.len() == before {
                for &id in &remaining {
                    widths.entry(id).or_insert(WIN_W);
                }
                break;
            }
        }

        widths
    }

    fn collect_positions(
        &self,
        id: u32,
        alloc_x: f64,
        y: f64,
        level_h: f64,
        gap: f64,
        win_w: f64,
        widths: &HashMap<u32, f64>,
        out: &mut Vec<(u32, f64, f64)>,
    ) {
        let subtree_w = widths.get(&id).copied().unwrap_or(win_w);
        // Center the window within its allocated width.
        out.push((id, alloc_x + (subtree_w - win_w) / 2.0, y));

        let children = self
            .windows
            .iter()
            .find(|cw| cw.id == id)
            .map(|cw| cw.children.clone())
            .unwrap_or_default();

        let mut child_x = alloc_x;
        for child_id in children {
            let cw = widths.get(&child_id).copied().unwrap_or(win_w);
            self.collect_positions(child_id, child_x, y + level_h, level_h, gap, win_w, widths, out);
            child_x += cw + gap;
        }
    }

    // ── Canvas / viewport ─────────────────────────────────────────────────

    pub fn viewport_center(&self) -> (f64, f64) {
        let (w, h) = self
            .space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| (g.size.w as f64, g.size.h as f64))
            .unwrap_or((800.0, 600.0));
        (self.viewport_x + w / 2.0, self.viewport_y + h / 2.0)
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.viewport_x += dx;
        self.viewport_y += dy;
        // Keep target in sync so any in-progress viewport animation doesn't fight pan.
        self.viewport_target_x = self.viewport_x;
        self.viewport_target_y = self.viewport_y;
        self.viewport_anim_start_x = self.viewport_x;
        self.viewport_anim_start_y = self.viewport_y;
        self.sync_window_positions();
    }

    pub fn sync_window_positions(&mut self) {
        let updates: Vec<(Window, i32, i32)> = self
            .windows
            .iter()
            .map(|cw| {
                let sx = (cw.canvas_x - self.viewport_x) as i32;
                let sy = (cw.canvas_y - self.viewport_y) as i32;
                (cw.window.clone(), sx, sy)
            })
            .collect();

        for (window, sx, sy) in updates {
            self.space.map_element(window, (sx, sy), false);
        }
    }

    // ── Animation ─────────────────────────────────────────────────────────

    /// Snapshot current canvas and viewport positions as animation start, begin 200 ms animation.
    pub fn begin_animation(&mut self) {
        for cw in &mut self.windows {
            cw.anim_start_x = cw.canvas_x;
            cw.anim_start_y = cw.canvas_y;
        }
        self.viewport_anim_start_x = self.viewport_x;
        self.viewport_anim_start_y = self.viewport_y;
        self.anim_start = Some(Instant::now());
    }

    /// Lerp all canvas and viewport positions toward their targets. Call once per frame.
    pub fn tick_animation(&mut self) {
        let Some(start) = self.anim_start else { return };
        const DURATION: f64 = 0.2;
        let t = (start.elapsed().as_secs_f64() / DURATION).min(1.0);

        for cw in &mut self.windows {
            cw.canvas_x = cw.anim_start_x + (cw.target_x - cw.anim_start_x) * t;
            cw.canvas_y = cw.anim_start_y + (cw.target_y - cw.anim_start_y) * t;
        }
        self.viewport_x = self.viewport_anim_start_x
            + (self.viewport_target_x - self.viewport_anim_start_x) * t;
        self.viewport_y = self.viewport_anim_start_y
            + (self.viewport_target_y - self.viewport_anim_start_y) * t;

        if t >= 1.0 {
            self.anim_start = None;
        }

        self.sync_window_positions();
    }

    /// Animate the viewport to center on the focused window's target position (tree view).
    pub fn center_viewport_on_focused(&mut self) {
        const WIN_W: f64 = 800.0;
        const WIN_H: f64 = 600.0;
        let fid = match self.focused_window_id {
            Some(id) => id,
            None => return,
        };
        let (tx, ty) = match self.windows.iter().find(|cw| cw.id == fid) {
            Some(cw) => (cw.target_x, cw.target_y),
            None => return,
        };
        let (sw, sh) = self.output_size();
        self.viewport_target_x = tx - sw / 2.0 + WIN_W / 2.0;
        self.viewport_target_y = ty - sh / 2.0 + WIN_H / 2.0;
        self.viewport_anim_start_x = self.viewport_x;
        self.viewport_anim_start_y = self.viewport_y;
        // Window positions are already at targets; only viewport moves.
        // Re-use anim_start (or start fresh if none in progress).
        self.anim_start = Some(Instant::now());
    }

    /// Animate viewport so focused window + its immediate children fill the screen (tree view).
    pub fn focus_zoom(&mut self) {
        const WIN_W: f64 = 800.0;
        const WIN_H: f64 = 600.0;
        let fid = match self.focused_window_id {
            Some(id) => id,
            None => return,
        };
        let (ftx, fty, children) = match self.windows.iter().find(|cw| cw.id == fid) {
            Some(cw) => (cw.target_x, cw.target_y, cw.children.clone()),
            None => return,
        };

        let mut min_x = ftx;
        let mut min_y = fty;
        let mut max_x = ftx + WIN_W;
        let mut max_y = fty + WIN_H;

        for &child_id in &children {
            if let Some(child) = self.windows.iter().find(|cw| cw.id == child_id) {
                min_x = min_x.min(child.target_x);
                min_y = min_y.min(child.target_y);
                max_x = max_x.max(child.target_x + WIN_W);
                max_y = max_y.max(child.target_y + WIN_H);
            }
        }

        let (sw, sh) = self.output_size();
        self.viewport_target_x = (min_x + max_x) / 2.0 - sw / 2.0;
        self.viewport_target_y = (min_y + max_y) / 2.0 - sh / 2.0;
        self.viewport_anim_start_x = self.viewport_x;
        self.viewport_anim_start_y = self.viewport_y;
        self.anim_start = Some(Instant::now());
    }

    /// Animate viewport to show bounding box of all root windows (tree view Ctrl+Home).
    pub fn snap_to_roots(&mut self) {
        const WIN_W: f64 = 800.0;
        const WIN_H: f64 = 600.0;
        let root_targets: Vec<(f64, f64)> = self
            .windows
            .iter()
            .filter(|cw| cw.parent_id.is_none())
            .map(|cw| (cw.target_x, cw.target_y))
            .collect();

        if root_targets.is_empty() {
            return;
        }

        let min_x = root_targets.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let min_y = root_targets.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let max_x = root_targets.iter().map(|p| p.0 + WIN_W).fold(f64::NEG_INFINITY, f64::max);
        let max_y = root_targets.iter().map(|p| p.1 + WIN_H).fold(f64::NEG_INFINITY, f64::max);

        let (sw, sh) = self.output_size();
        self.viewport_target_x = (min_x + max_x) / 2.0 - sw / 2.0;
        self.viewport_target_y = (min_y + max_y) / 2.0 - sh / 2.0;
        self.viewport_anim_start_x = self.viewport_x;
        self.viewport_anim_start_y = self.viewport_y;
        self.anim_start = Some(Instant::now());
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    // ── Debug output ───────────────────────────────────────────────────────

    pub fn print_tree(&self) {
        eprintln!("=== Window Tree ===");
        let roots: Vec<u32> = self
            .windows
            .iter()
            .filter(|cw| cw.parent_id.is_none())
            .map(|cw| cw.id)
            .collect();

        if roots.is_empty() {
            eprintln!("  (empty)");
        } else {
            for root_id in roots {
                self.print_subtree(root_id, 0);
            }
        }
    }

    fn print_subtree(&self, id: u32, depth: usize) {
        let Some(cw) = self.windows.iter().find(|cw| cw.id == id) else {
            return;
        };
        let indent = "  ".repeat(depth);
        let parent_str = match cw.parent_id {
            Some(p) => format!("parent={}", p),
            None => "root".to_string(),
        };
        let focus_marker = if self.focused_window_id == Some(id) {
            " ◀ focused"
        } else {
            ""
        };
        eprintln!("{}[{}] {}{}", indent, id, parent_str, focus_marker);
        for &child_id in &cw.children {
            self.print_subtree(child_id, depth + 1);
        }
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
