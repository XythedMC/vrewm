use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData as PointerGrabStartData, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::SERIAL_COUNTER,
};

use crate::{grabs::PanCanvasGrab, state::ViewMode, Treewm};

impl Treewm {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();

                let mut pending_tree_focus: Option<u32> = None;
                let mut toggle_view_mode = false;
                let mut focus_zoom_requested = false;
                let mut snap_to_roots_requested = false;
                let mut reset_viewport_requested = false;

                let keyboard = self.seat.get_keyboard().unwrap();
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    key_state,
                    serial,
                    time,
                    |data, modifiers, handle| {
                        if key_state != KeyState::Pressed {
                            return FilterResult::Forward;
                        }

                        let sym = handle.modified_sym();

                        // ── Viewport panning (Ctrl + Arrow / Home) ──────────────
                        if modifiers.ctrl {
                            if sym == Keysym::Left {
                                data.pan(-100.0, 0.0);
                                return FilterResult::Intercept(());
                            } else if sym == Keysym::Right {
                                data.pan(100.0, 0.0);
                                return FilterResult::Intercept(());
                            } else if sym == Keysym::Up {
                                data.pan(0.0, -100.0);
                                return FilterResult::Intercept(());
                            } else if sym == Keysym::Down {
                                data.pan(0.0, 100.0);
                                return FilterResult::Intercept(());
                            } else if sym == Keysym::Home {
                                match data.view_mode {
                                    ViewMode::Tiling => reset_viewport_requested = true,
                                    ViewMode::TreeView => snap_to_roots_requested = true,
                                }
                                return FilterResult::Intercept(());
                            }
                        }

                        // ── View mode toggle (Ctrl + Space) ─────────────────────
                        if modifiers.ctrl && sym == Keysym::space {
                            toggle_view_mode = true;
                            return FilterResult::Intercept(());
                        }

                        // ── Tree navigation (Ctrl + P / N / C) ──────────────────
                        if modifiers.ctrl {
                            if sym == Keysym::p {
                                pending_tree_focus = data
                                    .focused_window_id
                                    .and_then(|fid| {
                                        data.windows.iter().find(|cw| cw.id == fid)
                                    })
                                    .and_then(|cw| cw.parent_id);
                                return FilterResult::Intercept(());
                            } else if sym == Keysym::n {
                                if let Some(fid) = data.focused_window_id {
                                    let siblings = data.siblings_of(fid);
                                    if let Some(pos) =
                                        siblings.iter().position(|&id| id == fid)
                                    {
                                        let next = siblings[(pos + 1) % siblings.len()];
                                        if next != fid {
                                            pending_tree_focus = Some(next);
                                        }
                                    }
                                }
                                return FilterResult::Intercept(());
                            } else if sym == Keysym::c {
                                pending_tree_focus = data
                                    .focused_window_id
                                    .and_then(|fid| {
                                        data.windows.iter().find(|cw| cw.id == fid)
                                    })
                                    .and_then(|cw| cw.children.first().copied());
                                return FilterResult::Intercept(());
                            }
                        }

                        // ── Focus zoom (Ctrl + F, tree view) ────────────────────
                        if modifiers.ctrl && sym == Keysym::f {
                            focus_zoom_requested = true;
                            return FilterResult::Intercept(());
                        }

                        FilterResult::Forward
                    },
                );

                // Apply view mode toggle (keyboard mutex now released).
                if toggle_view_mode {
                    self.view_mode = match self.view_mode {
                        ViewMode::Tiling => ViewMode::TreeView,
                        ViewMode::TreeView => {
                            self.tiling_root_id = self.focused_window_id;
                            self.zoom = 1.0;
                            if let Some(output) = self.space.outputs().next() {
                                output.change_current_state(None, None, Some(smithay::output::Scale::Fractional(self.zoom)), None);
                            }
                            ViewMode::Tiling
                        }
                    };
                    self.apply_layout();
                    let mode_str = match self.view_mode {
                        ViewMode::Tiling => "tiling".to_string(),
                        ViewMode::TreeView => "tree".to_string(),
                    };
                    self.emit_event(crate::ipc::IpcEvent::ModeChanged { mode: mode_str });
                }

                // Apply tree focus change (keyboard mutex now released).
                if let Some(target_id) = pending_tree_focus {
                    self.focus_by_id(target_id);
                    self.tiling_root_id = Some(target_id);
                    match self.view_mode {
                        ViewMode::Tiling => self.apply_layout(),
                        ViewMode::TreeView => self.center_viewport_on_focused(),
                    }
                }

                if focus_zoom_requested && self.view_mode == ViewMode::TreeView {
                    self.focus_zoom();
                }

                if snap_to_roots_requested {
                    self.snap_to_roots();
                }

                if reset_viewport_requested {
                    self.viewport_x = 0.0;
                    self.viewport_y = 0.0;
                    self.viewport_target_x = 0.0;
                    self.viewport_target_y = 0.0;
                    self.viewport_anim_start_x = 0.0;
                    self.viewport_anim_start_y = 0.0;
                    self.sync_window_positions();
                }
            }
            InputEvent::PointerMotion { .. } => {}
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();
                let output_geo = self.space.output_geometry(output).unwrap();
                let pos =
                    event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().unwrap();
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                const BTN_MIDDLE: u32 = 0x112;
                if ButtonState::Pressed == button_state && !pointer.is_grabbed()
                    && button == BTN_MIDDLE
                {
                    let grab = PanCanvasGrab {
                        start_data: PointerGrabStartData {
                            focus: None,
                            button: BTN_MIDDLE,
                            location: pointer.current_location(),
                        },
                        initial_viewport_x: self.viewport_x,
                        initial_viewport_y: self.viewport_y,
                    };
                    pointer.set_grab(self, grab, serial, Focus::Clear);
                } else if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        let wl_surf = window.toplevel().unwrap().wl_surface().clone();
                        keyboard.set_focus(self, Some(wl_surf.clone()), serial);

                        self.focused_window_id = self
                            .windows
                            .iter()
                            .find(|cw| {
                                cw.window
                                    .toplevel()
                                    .map_or(false, |t| t.wl_surface() == &wl_surf)
                            })
                            .map(|cw| cw.id);

                        // In tree view, windows are free-form: don't reposition them on focus.
                        // In tiling, recalculate layout around the newly focused window.
                        match self.view_mode {
                            ViewMode::Tiling => {
                                self.apply_layout();
                                self.space.elements().for_each(|window| {
                                    window.toplevel().unwrap().send_pending_configure();
                                });
                            }
                            ViewMode::TreeView => {}
                        }
                    } else {
                        self.space.elements().for_each(|window| {
                            window.set_activated(false);
                            window.toplevel().unwrap().send_pending_configure();
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                        self.focused_window_id = None;
                        if self.view_mode == ViewMode::Tiling {
                            self.apply_layout();
                        }
                    }
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                if self.view_mode == ViewMode::TreeView && vertical_amount != 0.0 {
                    let pointer = self.seat.get_pointer().unwrap();
                    let pointer_loc = pointer.current_location();

                    let old_zoom = self.zoom;
                    let zoom_factor = 1.1_f64.powf(-vertical_amount / 15.0);
                    self.zoom = (self.zoom * zoom_factor).clamp(0.2, 5.0);

                    self.viewport_x += pointer_loc.x - pointer_loc.x * (old_zoom / self.zoom);
                    self.viewport_y += pointer_loc.y - pointer_loc.y * (old_zoom / self.zoom);
                    self.viewport_target_x = self.viewport_x;
                    self.viewport_target_y = self.viewport_y;
                    self.viewport_anim_start_x = self.viewport_x;
                    self.viewport_anim_start_y = self.viewport_y;

                    if let Some(output) = self.space.outputs().next() {
                        output.change_current_state(None, None, Some(smithay::output::Scale::Fractional(self.zoom)), None);
                    }
                    self.sync_window_positions();
                    return;
                }

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }
                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}
