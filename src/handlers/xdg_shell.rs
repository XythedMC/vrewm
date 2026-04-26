use smithay::{
    delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, PopupKind, PopupManager, Space, Window,
    },
    input::{
        pointer::{Focus, GrabStartData as PointerGrabStartData},
        Seat,
    },
    reexports::wayland_server::{protocol::wl_seat, Resource},
    utils::Serial,
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{grabs::MoveSurfaceGrab, state::{CanvasWindow, ViewMode}, Treewm};

impl XdgShellHandler for Treewm {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.size = Some((800, 600).into());
        });

        let id = self.alloc_id();

        // Parent = currently focused window; None means new tree root.
        let parent_id = self.focused_window_id;

        let (cx, cy) = self.viewport_center();
        let canvas_x = cx - 400.0;
        let canvas_y = cy - 300.0;

        let window = Window::new_wayland_window(surface);
        let screen_x = (canvas_x - self.viewport_x) as i32;
        let screen_y = (canvas_y - self.viewport_y) as i32;
        self.space.map_element(window.clone(), (screen_x, screen_y), true);

        // Register this window as a child of its parent.
        if let Some(pid) = parent_id {
            if let Some(parent) = self.windows.iter_mut().find(|cw| cw.id == pid) {
                parent.children.push(id);
            }
        }

        self.windows.push(CanvasWindow {
            id,
            window,
            canvas_x,
            canvas_y,
            target_x: canvas_x,
            target_y: canvas_y,
            anim_start_x: canvas_x,
            anim_start_y: canvas_y,
            parent_id,
            children: Vec::new(),
            tree_x: None,
            tree_y: None,
        });

        self.focus_by_id(id);
        self.print_tree();
        // In tree view, other windows are free-form — don't reposition them for a new window.
        // The new window is already placed at viewport center above.
        if self.view_mode == ViewMode::Tiling {
            self.apply_layout();
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let dead_surf = surface.wl_surface();

        let Some(pos) = self.windows.iter().position(|cw| {
            cw.window
                .toplevel()
                .map_or(false, |t| t.wl_surface() == dead_surf)
        }) else {
            return;
        };

        let dead_id = self.windows[pos].id;
        let dead_parent_id = self.windows[pos].parent_id;
        let children: Vec<u32> = self.windows[pos].children.clone();

        // Re-parent orphans: they inherit the dead window's parent (or become roots).
        for &child_id in &children {
            if let Some(child) = self.windows.iter_mut().find(|cw| cw.id == child_id) {
                child.parent_id = dead_parent_id;
            }
        }

        // Update grandparent's children list: swap dead for its orphans.
        if let Some(pid) = dead_parent_id {
            if let Some(parent) = self.windows.iter_mut().find(|cw| cw.id == pid) {
                let insert_pos = parent.children.iter().position(|&id| id == dead_id);
                parent.children.retain(|&id| id != dead_id);
                if let Some(pos) = insert_pos {
                    for (i, &child_id) in children.iter().enumerate() {
                        parent.children.insert(pos + i, child_id);
                    }
                } else {
                    parent.children.extend_from_slice(&children);
                }
            }
        }
        // If dead window was a root, its orphans already have parent_id = None → they are roots.

        self.windows.remove(pos);

        // Update focus.
        if self.focused_window_id == Some(dead_id) {
            let new_focus = dead_parent_id
                .filter(|&pid| self.windows.iter().any(|cw| cw.id == pid))
                .or_else(|| self.windows.last().map(|cw| cw.id));

            match new_focus {
                Some(fid) => self.focus_by_id(fid),
                None => self.focus_clear(),
            }
        }

        if self.tiling_root_id == Some(dead_id) {
            self.tiling_root_id = self.focused_window_id;
        }

        self.print_tree();
        if self.view_mode == ViewMode::Tiling {
            self.apply_layout();
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap();
        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let Some(cw) = self
                .windows
                .iter()
                .find(|cw| cw.window.toplevel().map_or(false, |t| t.wl_surface() == wl_surface))
            else {
                return;
            };

            let grab = MoveSurfaceGrab {
                start_data,
                window: cw.window.clone(),
                window_surface: wl_surface.clone(),
                initial_canvas_x: cw.canvas_x,
                initial_canvas_y: cw.canvas_y,
            };

            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

delegate_xdg_shell!(Treewm);

fn check_grab(
    seat: &Seat<Treewm>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<Treewm>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

pub fn handle_commit(
    popups: &mut PopupManager,
    space: &Space<Window>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
        }
    }

    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match popup {
            PopupKind::Xdg(ref xdg) => {
                if !xdg.is_initial_configure_sent() {
                    xdg.send_configure().expect("initial configure failed");
                }
            }
            PopupKind::InputMethod(_) => {}
        }
    }
}

impl Treewm {
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &root)
        else {
            return;
        };

        let output = self.space.outputs().next().unwrap();
        let output_geo = self.space.output_geometry(output).unwrap();
        let window_geo = self.space.element_geometry(window).unwrap();

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
