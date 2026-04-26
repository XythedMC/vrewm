use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

use crate::Treewm;

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<Treewm>,
    /// The window being dragged.
    pub window: Window,
    /// Cached surface so we can find the window in the canvas Vec efficiently.
    pub window_surface: WlSurface,
    /// Canvas position of the window when the drag started.
    pub initial_canvas_x: f64,
    pub initial_canvas_y: f64,
}

impl PointerGrab<Treewm> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // No surface has pointer focus while dragging.
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_canvas_x = self.initial_canvas_x + delta.x;
        let new_canvas_y = self.initial_canvas_y + delta.y;

        // Keep canvas coordinates in sync so panning still works correctly.
        for cw in data.windows.iter_mut() {
            if cw.window
                .toplevel()
                .map_or(false, |t| t.wl_surface() == &self.window_surface)
            {
                cw.canvas_x = new_canvas_x;
                cw.canvas_y = new_canvas_y;
                cw.target_x = new_canvas_x;
                cw.target_y = new_canvas_y;
                cw.anim_start_x = new_canvas_x;
                cw.anim_start_y = new_canvas_y;
                if data.view_mode == crate::state::ViewMode::TreeView {
                    cw.tree_x = Some(new_canvas_x);
                    cw.tree_y = Some(new_canvas_y);
                }
                break;
            }
        }

        let screen_x = (new_canvas_x - data.viewport_x) as i32;
        let screen_y = (new_canvas_y - data.viewport_y) as i32;
        data.space.map_element(self.window.clone(), (screen_x, screen_y), true);
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

    fn start_data(&self) -> &PointerGrabStartData<Treewm> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Treewm) {}
}
