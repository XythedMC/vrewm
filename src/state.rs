use std::{collections::HashMap, ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    desktop::{PopupManager, Space, Window, WindowSurfaceType, LayerSurface},
    input::{Seat, SeatState, pointer::CursorImageStatus},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle, backend::{ClientData, ClientId, DisconnectReason}, protocol::wl_surface::WlSurface
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState}, cursor_shape::CursorShapeManagerState, dmabuf::{DmabufState, ImportNotifier}, fractional_scale::FractionalScaleManagerState, output::OutputManagerState, selection::{
            data_device::DataDeviceState,
            primary_selection::PrimarySelectionState,
        }, shell::{wlr_layer::{WlrLayerShellState}, xdg::{XdgShellState, decoration::XdgDecorationState}}, shm::ShmState, socket::ListeningSocketSource, viewporter::ViewporterState, xdg_activation::XdgActivationState,
    },
};

use crate::handlers::config::TreeWMConfig;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Tiling,
    TreeView,
}
#[derive(Clone, Copy)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Super,
    Shift,
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
    /// None means this window is a tree root.
    pub parent_id: Option<u32>,
    /// IDs of direct children, in open order.
    pub children: Vec<u32>,
    /// Manually saved positions for TreeView mode.
    pub tree_x: Option<f64>,
    pub tree_y: Option<f64>,
    pub base_width: i32,
    pub base_height: i32,

    pub is_fullscreen: bool,
    pub pre_fullscreen_x: f64,
    pub pre_fullscreen_y: f64,
    pub pre_fullscreen_width: i32,
    pub pre_fullscreen_height: i32,
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

    pub zoom_anim_start: f64,
    pub zoom_target: f64,
    pub zoom_returning: bool,
    pub zoom_animating: bool,

    pub(crate) viewport_anim_start_x: f64,
    pub(crate) viewport_anim_start_y: f64,
    /// When Some, a 300 ms cubic ease-out animation is in progress.
    pub anim_start: Option<Instant>,

    pub next_window_id: u32,
    /// The ID of the window that currently holds keyboard focus.
    pub focused_window_id: Option<u32>,
    pub tiling_root_id: Option<u32>,
    pub view_mode: ViewMode,
    pub zoom: f64,
    pub gap: f64,
    pub config: TreeWMConfig,
    pub cursor_icon: CursorImageStatus,
    pub layer_surfaces: Vec<LayerSurface>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub cursor_shape_manager_state: CursorShapeManagerState,
    pub wlr_layer_shell_state: WlrLayerShellState,
    pub decoration_state: XdgDecorationState,
    pub activation_state: XdgActivationState,
    pub viewporter_state: ViewporterState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub dmabuf_state: DmabufState,
    pub primary_selection_state: PrimarySelectionState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Treewm>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,

    pub seat: Seat<Self>,
    pub event_tx: Option<tokio::sync::broadcast::Sender<crate::ipc::IpcEvent>>,

    pub main_modifier: ModifierKey,
    /// DMABuf buffers waiting to be imported by the renderer on the next frame.
    pub pending_dmabufs: Vec<(Dmabuf, ImportNotifier)>,
}

impl Treewm {
    pub fn new(event_loop: &mut EventLoop<Self>, display: Display<Self>, config: TreeWMConfig) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let cursor_shape_manager_state = CursorShapeManagerState::new::<Self>(&dh);
        let wlr_layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let decoration_state = XdgDecorationState::new::<Self>(&dh);
        let activation_state = XdgActivationState::new::<Self>(&dh);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&dh);
        let dmabuf_state = DmabufState::new();
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");
        seat.add_keyboard(Default::default(), 200, 25).expect("Keyboard not found while trying to add it");
        seat.add_pointer();

        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        let main_modifier = match config.main_modifier.as_str() {
            "Ctrl" => ModifierKey::Ctrl,
            "Super" => ModifierKey::Super,
            "Shift" => ModifierKey::Shift,
            "Alt" => ModifierKey::Alt,
            _ => panic!("Main modifier from config file isnt allowed, options are: Ctrl, Super, Shift and Alt")
        };
        let cursor_icon = CursorImageStatus::default_named();
        let layer_surfaces = Vec::new();
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
            zoom_anim_start: 1.0,
            zoom_target: 1.0,
            zoom_returning: false,
            zoom_animating: false,
            anim_start: None,
            next_window_id: 0,
            focused_window_id: None,
            tiling_root_id: None,
            view_mode: ViewMode::Tiling,
            zoom: 1.0,
            gap: config.gap,
            config,
            cursor_icon,
            layer_surfaces,
            socket_name,
            compositor_state,
            xdg_shell_state,
            cursor_shape_manager_state,
            wlr_layer_shell_state,
            decoration_state,
            activation_state,
            viewporter_state,
            fractional_scale_state,
            dmabuf_state,
            primary_selection_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            seat,
            event_tx: None,
            main_modifier,
            pending_dmabufs: Vec::new(),
        }
    }

    fn init_wayland_listener(
        display: Display<Treewm>,
        event_loop: &mut EventLoop<Self>,
    ) -> OsString {
        let listening_socket = ListeningSocketSource::with_name("wayland-treewm").expect("Couldn't initialize wayland socket because all sockets were already taken");
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .expect("Couldn't insert client as it was invalid by insertion time");
            })
            .expect("Failed to init wayland socket");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    unsafe {
                        display.get_mut().dispatch_clients(state).expect("Failed to dispatch clients");
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to insert display into the loop");

        "wayland-treewm".into()
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
        if self.focused_window_id != Some(id) {
            self.focused_window_id = Some(id);
            self.emit_event(crate::ipc::IpcEvent::FocusChanged { id: Some(id.to_string()) });
        }
        let serial = SERIAL_COUNTER.next_serial();
        let surface = self
            .windows
            .iter()
            .find(|cw| cw.id == id)
            .and_then(|cw| cw.window.toplevel().map(|t| t.wl_surface().clone()));
        let keyboard = self.seat.get_keyboard().expect("Keyboard not found - this is a bug");
        keyboard.set_focus(self, surface, serial);
    }

    /// Clear keyboard focus.
    pub fn focus_clear(&mut self) {
        if self.focused_window_id.is_some() {
            self.focused_window_id = None;
            self.emit_event(crate::ipc::IpcEvent::FocusChanged { id: None });
        }
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().expect("Keyboard not found - this is a bug");
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
        self.emit_event(crate::ipc::IpcEvent::LayoutChanged);
    }

    fn output_size(&self) -> (f64, f64) {
        let (w, h) = self.space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| (g.size.w as f64, g.size.h as f64))
            .unwrap_or((1920.0, 1080.0));
        
        if w <= 0.0 || h <= 0.0 {
            (800.0, 600.0)
        } else {
            (w, h)
        }
    }

    pub fn toggle_fullscreen(&mut self) {
        let (sw, sh) = self.output_size();
        let cw = self
            .windows
            .iter_mut()
            .find(|cw| Some(cw.id) == self.focused_window_id);
        if let Some(window) = cw {
            if window.is_fullscreen == false {
                window.pre_fullscreen_x = window.canvas_x;                                                                            
                window.pre_fullscreen_y = window.canvas_y;                                                                            
                window.pre_fullscreen_width = window.base_width;                                                                      
                window.pre_fullscreen_height = window.base_height;
                
                (window.target_x, window.target_y) = (self.viewport_x, self.viewport_y);
                (window.base_width, window.base_height) = (sw as i32, sh as i32);

                if let Some(tl) = window.window.toplevel() {
                    tl.with_pending_state(|s| s.size = Some((sw as i32, sh as i32).into()));
                    tl.send_pending_configure();
                }
                window.is_fullscreen = true;
            } else {
                window.canvas_x = window.pre_fullscreen_x;                                                                            
                window.canvas_y = window.pre_fullscreen_y;                                                                            
                window.base_width = window.pre_fullscreen_width;                                                                      
                window.base_height = window.pre_fullscreen_height;
                if let Some(tl) = window.window.toplevel() {
                    tl.with_pending_state(|s| s.size = Some((window.pre_fullscreen_width, window.pre_fullscreen_height).into()));
                    tl.send_pending_configure();
                }
                window.is_fullscreen = false;
            }
        }
    }

    fn layout_tiling(&mut self) {
        let Some(tiling_root) = self.tiling_root_id.or(self.focused_window_id) else { return };
        let (w, h) = self.output_size();

        let mut slots = HashMap::new();
        self.layout_node_bsp(tiling_root, (0.0, 0.0, w, h), &mut slots);

        self.viewport_target_x = 0.0;
        self.viewport_target_y = 0.0;

        // Resize windows (two-pass: collect toplevels, then configure).
        let resize_ops: Vec<(u32, i32, i32)> = slots
            .iter()
            .map(|(&id, &(_, _, sw, sh))| (id, sw as i32, sh as i32))
            .collect();
        for (id, sw, sh) in resize_ops {                                                                                                               
            if let Some(cw) = self.windows.iter_mut().find(|cw| cw.id == id) {                                                                         
                cw.base_width = sw;                                                                                                                    
                cw.base_height = sh;                                                                                                                   
                if let Some(tl) = cw.window.toplevel() {
                    tl.with_pending_state(|s| { s.size = Some((sw, sh).into()); });                                                                    
                    tl.send_pending_configure();                                                                                                       
                }
            }                                                                                                                                          
        }  

        // Set animation targets.
        for cw in &mut self.windows {
            if let Some(&(tx, ty, _, _)) = slots.get(&cw.id) {
                cw.target_x = tx;
                cw.target_y = ty;
            } else {
                self.space.unmap_elem(&cw.window);
            }
        }

        self.begin_animation();
    }

    fn layout_node_bsp(&self, node_id: u32, rect: (f64, f64, f64, f64), out: &mut HashMap<u32, (f64, f64, f64, f64)>) {
        let children = self
            .windows
            .iter()
            .find(|cw| cw.id == node_id)
            .map(|cw| cw.children.clone())
            .unwrap_or_default();

        if children.is_empty() {
            out.insert(node_id, rect);
        } else {
            let (x, y, w, h) = rect;
            let left_w = w / 2.0;
            let right_w = w - left_w;
            out.insert(node_id, (x, y, left_w, h));
            self.layout_siblings_bsp(&children, (x + left_w, y, right_w, h), out);
        }
    }

    fn layout_siblings_bsp(&self, siblings: &[u32], rect: (f64, f64, f64, f64), out: &mut HashMap<u32, (f64, f64, f64, f64)>) {
        if siblings.is_empty() {
            return;
        }
        if siblings.len() == 1 {
            self.layout_node_bsp(siblings[0], rect, out);
        } else {
            let mid = siblings.len() / 2;
            let (x, y, w, h) = rect;
            let top_h = h / 2.0;
            let bottom_h = h - top_h;
            self.layout_siblings_bsp(&siblings[..mid], (x, y, w, top_h), out);
            self.layout_siblings_bsp(&siblings[mid..], (x, y + top_h, w, bottom_h), out);
        }
    }

    fn layout_tree(&mut self) {
        const LEVEL_H: f64 = 400.0;
        const WIN_W: f64 = 800.0;

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
            + self.gap * (roots.len() - 1) as f64;

        let mut alloc_x = -total_w / 2.0;
        let mut positions: Vec<(u32, f64, f64)> = Vec::new();

        for &root in &roots {
            self.collect_positions(root, alloc_x, 0.0, LEVEL_H, self.gap, WIN_W, &widths, &mut positions);
            alloc_x += widths[&root] + self.gap;
        }

        // Set animation targets.
        for (id, cx, cy) in positions {
            if let Some(cw) = self.windows.iter_mut().find(|cw| cw.id == id) {
                cw.target_x = cw.tree_x.unwrap_or(cx);
                cw.target_y = cw.tree_y.unwrap_or(cy);
            }
        }

        // Resize every window to its base size.
        let toplevel_ops: Vec<(u32, i32, i32)> = self.windows.iter().map(|cw| (cw.id, cw.base_width, cw.base_height)).collect();
        for (id, bw, bh) in toplevel_ops {
            if let Some(cw) = self.windows.iter().find(|cw| cw.id == id) {
                if let Some(tl) = cw.window.toplevel() {
                    tl.with_pending_state(|s| {
                        s.size = Some((bw, bh).into());
                    });
                    tl.send_pending_configure();
                }
            }
        }

        // Set viewport target to center on focused window.
        if let Some(fid) = self.focused_window_id {
            if let Some(cw) = self.windows.iter().find(|cw| cw.id == fid) {
                let (sw, sh) = self.output_size();
                self.viewport_target_x = cw.target_x - sw / 2.0 + cw.base_width as f64 / 2.0;
                self.viewport_target_y = cw.target_y - sh / 2.0 + cw.base_height as f64 / 2.0;
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
        self.emit_event(crate::ipc::IpcEvent::ViewportChanged { x: self.viewport_x, y: self.viewport_y });
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
        const DURATION: f64 = 0.3;
        let t = (start.elapsed().as_secs_f64() / DURATION).min(1.0);

        // Cubic ease-out for smoother deceleration
        let ease_t = 1.0 - (1.0 - t).powi(3);

        for cw in &mut self.windows {
            cw.canvas_x = cw.anim_start_x + (cw.target_x - cw.anim_start_x) * ease_t;
            cw.canvas_y = cw.anim_start_y + (cw.target_y - cw.anim_start_y) * ease_t;
        }
        self.viewport_x = self.viewport_anim_start_x
            + (self.viewport_target_x - self.viewport_anim_start_x) * ease_t;
        self.viewport_y = self.viewport_anim_start_y
            + (self.viewport_target_y - self.viewport_anim_start_y) * ease_t;
        self.zoom = self.zoom_anim_start + (self.zoom_target - self.zoom_anim_start) * ease_t;
        if t >= 1.0 && self.zoom_returning == false && self.zoom_animating {
            self.zoom_target = 1.0;
            self.zoom_anim_start = self.zoom;
            self.zoom_returning = true;
            self.anim_start = Some(Instant::now());
        }
        else if self.zoom_returning == true && t >= 1.0 { self.zoom_animating = false; }

        self.sync_window_positions();
    }

    /// Animate the viewport to center on the focused window's target position (tree view).
    pub fn center_viewport_on_focused(&mut self) {
        let fid = match self.focused_window_id {
            Some(id) => id,
            None => return,
        };
        let (tx, ty, ww, wh) = match self.windows.iter().find(|cw| cw.id == fid) {
            Some(cw) => (cw.target_x, cw.target_y, cw.base_width as f64, cw.base_height as f64),
            None => return,
        };
        let (sw, sh) = self.output_size();
        self.viewport_target_x = tx - sw / 2.0 + ww / 2.0;
        self.viewport_target_y = ty - sh / 2.0 + wh / 2.0;
        self.viewport_anim_start_x = self.viewport_x;
        self.viewport_anim_start_y = self.viewport_y;
        self.anim_start = Some(Instant::now());
    }

    /// Animate viewport so focused window + its immediate children fill the screen (tree view).
    pub fn focus_zoom(&mut self) {
        let fid = match self.focused_window_id {
            Some(id) => id,
            None => return,
        };
        let (ftx, fty, fww, fwh, children) = match self.windows.iter().find(|cw| cw.id == fid) {
            Some(cw) => (cw.target_x, cw.target_y, cw.base_width as f64, cw.base_height as f64, cw.children.clone()),
            None => return,
        };

        let mut min_x = ftx;
        let mut min_y = fty;
        let mut max_x = ftx + fww;
        let mut max_y = fty + fwh;

        for &child_id in &children {
            if let Some(child) = self.windows.iter().find(|cw| cw.id == child_id) {
                min_x = min_x.min(child.target_x);
                min_y = min_y.min(child.target_y);
                max_x = max_x.max(child.target_x + child.base_width as f64);
                max_y = max_y.max(child.target_y + child.base_height as f64);
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
        let root_bounds: Vec<(f64, f64, f64, f64)> = self
            .windows
            .iter()
            .filter(|cw| cw.parent_id.is_none())
            .map(|cw| (cw.target_x, cw.target_y, cw.base_width as f64, cw.base_height as f64))
            .collect();

        if root_bounds.is_empty() {
            return;
        }

        let min_x = root_bounds.iter().map(|p| p.0      ).fold(f64::INFINITY,     f64::min);
        let min_y = root_bounds.iter().map(|p| p.1      ).fold(f64::INFINITY,     f64::min);
        let max_x = root_bounds.iter().map(|p| p.0 + p.2).fold(f64::NEG_INFINITY, f64::max);
        let max_y = root_bounds.iter().map(|p| p.1 + p.3).fold(f64::NEG_INFINITY, f64::max);

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

    // ── IPC ───────────────────────────────────────────────────────────────
    pub fn emit_event(&self, event: crate::ipc::IpcEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }

    pub fn handle_ipc_cmd(&mut self, cmd: crate::ipc::InternalCommand) {
        use crate::ipc::{InternalCommand, TreeWindow, TreeViewport, TreeResponse, IpcEvent};
        match cmd {
            InternalCommand::GetTree { reply_to } => {
                let windows = self.windows.iter().map(|cw| {
                    let title = cw.window.toplevel().map(|t| {
                        smithay::wayland::compositor::with_states(t.wl_surface(), |states| {
                            states.data_map.get::<std::sync::Mutex<smithay::wayland::shell::xdg::XdgToplevelSurfaceRoleAttributes>>()
                                .and_then(|attr| attr.lock().unwrap_or_else(|e| {
                                    tracing::warn!("Mutex poisoned getting window title: {e}");
                                    e.into_inner()
                                }).title.clone())
                        })
                    }).flatten().unwrap_or_default();
                    let geo = cw.window.geometry();
                    let (width, height) = (geo.size.w, geo.size.h);
                    TreeWindow {
                        id: cw.id.to_string(),
                        title,
                        parent: cw.parent_id.map(|id| id.to_string()),
                        children: cw.children.iter().map(|id| id.to_string()).collect(),
                        canvas_x: cw.canvas_x,
                        canvas_y: cw.canvas_y,
                        width,
                        height,
                        focused: self.focused_window_id == Some(cw.id),
                    }
                }).collect();
                let mode = match self.view_mode {
                    ViewMode::Tiling => "tiling".to_string(),
                    ViewMode::TreeView => "tree".to_string(),
                };
                let resp = TreeResponse {
                    windows,
                    viewport: TreeViewport { x: self.viewport_x, y: self.viewport_y },
                    mode,
                };
                let _ = reply_to.send(serde_json::to_string(&resp).expect("Tree Responses implementation of Serialize failed, or tree response contains a map with non-string keys"));
            }
            InternalCommand::Focus { id } => {
                if let Ok(id) = id.parse::<u32>() {
                    self.focus_by_id(id);
                    self.tiling_root_id = Some(id);
                    match self.view_mode {
                        ViewMode::Tiling => self.apply_layout(),
                        ViewMode::TreeView => self.center_viewport_on_focused(),
                    }
                }
            }
            InternalCommand::Pan { dx, dy } => {
                self.pan(dx, dy);
                self.emit_event(IpcEvent::ViewportChanged {
                    x: self.viewport_x,
                    y: self.viewport_y,
                });
            }
            InternalCommand::SetMode { mode } => {
                let new_mode = if mode == "tiling" {
                    ViewMode::Tiling
                } else if mode == "tree" {
                    ViewMode::TreeView
                } else {
                    return;
                };
                if self.view_mode != new_mode {
                    self.view_mode = new_mode;
                    if new_mode == ViewMode::Tiling {
                        self.tiling_root_id = self.focused_window_id;
                        self.zoom = 1.0;
                        if let Some(output) = self.space.outputs().next() {
                            output.change_current_state(None, None, Some(smithay::output::Scale::Fractional(self.zoom)), None);
                        }
                    }
                    self.apply_layout();
                    self.emit_event(IpcEvent::ModeChanged { mode: mode.clone() });
                }
            }
        }
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
    fn initialized(&self, client_id: ClientId) {
        tracing::info!("client connected: {:?}", client_id);
    }
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        tracing::info!("client disconnected: {:?} reason={:?}", client_id, reason);
    }
}
