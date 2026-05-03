mod compositor;
mod xdg_shell;
pub mod config;
pub mod cursor_shape;
pub mod tablet;
use crate::Treewm;

use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Serial;
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};
use smithay::wayland::selection::primary_selection::{PrimarySelectionHandler, PrimarySelectionState, set_primary_focus};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};
use smithay::wayland::fractional_scale::FractionalScaleHandler;
use smithay::wayland::dmabuf::{DmabufHandler, DmabufState, ImportNotifier};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::{delegate_data_device, delegate_output, delegate_seat, delegate_primary_selection};
use smithay::{delegate_xdg_decoration, delegate_xdg_activation, delegate_viewporter, delegate_fractional_scale, delegate_dmabuf};

impl SeatHandler for Treewm {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Treewm> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.cursor_icon = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }
}

delegate_seat!(Treewm);

impl SelectionHandler for Treewm {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Treewm {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl DndGrabHandler for Treewm {}

impl WaylandDndGrabHandler for Treewm {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        match type_ {
            GrabType::Pointer => {
                let ptr = seat.get_pointer().unwrap();
                let start_data = ptr.grab_start_data().unwrap();
                let grab =
                    DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                ptr.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                source.cancel();
            }
        }
    }
}

delegate_data_device!(Treewm);

impl OutputHandler for Treewm {}
delegate_output!(Treewm);

impl PrimarySelectionHandler for Treewm {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}
delegate_primary_selection!(Treewm);

impl XdgDecorationHandler for Treewm {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
        });
        toplevel.send_pending_configure();
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: zxdg_toplevel_decoration_v1::Mode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
        });
        toplevel.send_pending_configure();
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
        });
        toplevel.send_pending_configure();
    }
}
delegate_xdg_decoration!(Treewm);

impl XdgActivationHandler for Treewm {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }
    fn request_activation(&mut self, _token: XdgActivationToken, _data: XdgActivationTokenData, _surface: WlSurface) {}
}
delegate_xdg_activation!(Treewm);

delegate_viewporter!(Treewm);

impl FractionalScaleHandler for Treewm {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        smithay::wayland::compositor::with_states(&surface, |states| {
            smithay::wayland::fractional_scale::with_fractional_scale(states, |fs| {
                fs.set_preferred_scale(1.0);
            });
        });
    }
}
delegate_fractional_scale!(Treewm);

impl DmabufHandler for Treewm {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }
    fn dmabuf_imported(&mut self, _global: &smithay::wayland::dmabuf::DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        // Queue for import on the next render frame where the renderer is available.
        self.pending_dmabufs.push((dmabuf, notifier));
    }
}
delegate_dmabuf!(Treewm);
