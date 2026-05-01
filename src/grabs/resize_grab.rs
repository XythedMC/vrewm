use smithay::{
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    reexports::{wayland_server::protocol::wl_surface::WlSurface,
                wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge},
    utils::{Logical, Point},
};
use crate::Treewm;

pub struct ResizeSurfaceGrab {
    pub start_data: PointerGrabStartData<Treewm>,
    /// Cached surface so we can find the window in the canvas Vec efficiently.
    pub window_surface: WlSurface,
    /// The size of the window when the drag started.
    pub initial_width: i32,
    pub initial_height: i32,

    pub initial_canvas_x: f64,
    pub initial_canvas_y: f64,

    pub grabbed_edge: ResizeEdge,
}

impl PointerGrab<Treewm> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        _focus: Option<(<Treewm as smithay::input::SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        
        let delta = event.location - self.start_data.location;
        let mut new_width = self.initial_width;
        let mut new_height = self.initial_height;
        let mut canvas_x = self.initial_canvas_x;
        let mut canvas_y = self.initial_canvas_y;

        match self.grabbed_edge {
            ResizeEdge::Bottom => new_height = (new_height + delta.y as i32).max(128),
            ResizeEdge::Left => (new_width, canvas_x) = ((new_width - delta.x as i32).max(128), self.initial_canvas_x + delta.x),
            ResizeEdge::Top => (new_height, canvas_y) = ((new_height - delta.y as i32).max(128), self.initial_canvas_y + delta.y),
            ResizeEdge::Right => new_width = (new_width + delta.x as i32).max(128),
            ResizeEdge::TopLeft => (new_width, new_height, canvas_x, canvas_y) = ((new_width - delta.x as i32).max(128), (new_height - delta.y as i32).max(128), self.initial_canvas_x + delta.x, self.initial_canvas_y + delta.y),
            ResizeEdge::BottomLeft => (new_width, new_height, canvas_x) = ((new_width - delta.x as i32).max(128), (new_height + delta.y as i32).max(128), self.initial_canvas_x + delta.x),
            ResizeEdge::TopRight => (new_width, new_height, canvas_y) = ((new_width + delta.x as i32).max(128), (new_height - delta.y as i32).max(128), self.initial_canvas_y + delta.y),
            ResizeEdge::BottomRight => (new_width, new_height) = ((new_width + delta.x as i32).max(128), (new_height + delta.y as i32).max(128)),
            ResizeEdge::None => {},
            _ => {},
        };

        for cw in data.windows.iter_mut() {
            if cw.window
                .toplevel()
                .map_or(false, |t| t.wl_surface() == &self.window_surface)
            {
                cw.base_height = new_height;
                cw.base_width = new_width;
                cw.canvas_x = canvas_x;
                cw.target_x = canvas_x;
                cw.canvas_y = canvas_y;
                cw.target_y = canvas_y;
                if let Some(tl) = cw.window.toplevel() {
                    tl.with_pending_state(|s| { s.size = Some((new_width, new_height).into()); });
                    tl.send_pending_configure();
                }
            }
        }
        data.sync_window_positions();

    }
    fn relative_motion(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        const BTN_LEFT: u32 = 0x110;
        if !handle.current_pressed().contains(&BTN_LEFT) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Treewm, handle: &mut PointerInnerHandle<'_, Treewm>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn unset(&mut self, _data: &mut Treewm) { }
    
    fn start_data(&self) -> &PointerGrabStartData<Treewm> {
        &self.start_data
    }
}