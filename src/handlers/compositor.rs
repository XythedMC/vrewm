use crate::{state::ClientState, Treewm};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        shm::{ShmHandler, ShmState},
    },
};

use super::xdg_shell;

impl CompositorHandler for Treewm {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            let mut position_changed = false;
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .space
                .elements()
                .find(|w| w.toplevel().map_or(false, |t| t.wl_surface() == &root))
            {
                window.on_commit();
            }
            if let Some(cw) = self
                .windows
                .iter_mut()
                .find(|w| w.window.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)) 
            {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
                if cw.resize_edge != ResizeEdge::None {
                    let current_w = cw.window.geometry().size.w;
                    let current_h = cw.window.geometry().size.h;

                    match cw.resize_edge {
                        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
                            let expected_x = cw.resize_initial_x + (cw.resize_initial_w - current_w) as f64;
                            if cw.canvas_x != expected_x {
                                cw.canvas_x = expected_x;
                                cw.target_x = expected_x;
                                position_changed = true;
                            }
                        }
                        _ => {}
                    }
                    match cw.resize_edge {
                        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
                            let expected_y = cw.resize_initial_y + (cw.resize_initial_h - current_h) as f64;
                            if cw.canvas_y != expected_y {
                                cw.canvas_y = expected_y;
                                cw.target_y = expected_y;
                                position_changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if position_changed { self.sync_window_positions(); }
        }

        xdg_shell::handle_commit(&mut self.popups, &self.space, surface);
    }
}

impl BufferHandler for Treewm {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Treewm {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Treewm);
delegate_shm!(Treewm);
