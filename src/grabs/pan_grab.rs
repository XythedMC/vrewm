use smithay::{
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

pub struct PanCanvasGrab {
    pub start_data: PointerGrabStartData<Treewm>,
    pub initial_viewport_x: f64,
    pub initial_viewport_y: f64,
}

impl PointerGrab<Treewm> for PanCanvasGrab {
    fn motion(
        &mut self,
        data: &mut Treewm,
        handle: &mut PointerInnerHandle<'_, Treewm>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        data.viewport_x = self.initial_viewport_x - delta.x;
        data.viewport_y = self.initial_viewport_y - delta.y;
        // Keep target/anim_start in sync so no animation fights the pan.
        data.viewport_target_x = data.viewport_x;
        data.viewport_target_y = data.viewport_y;
        data.viewport_anim_start_x = data.viewport_x;
        data.viewport_anim_start_y = data.viewport_y;
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
        const BTN_MIDDLE: u32 = 0x112;
        if !handle.current_pressed().contains(&BTN_MIDDLE) {
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
