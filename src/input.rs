use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData as PointerGrabStartData, MotionEvent, CursorIcon, CursorImageStatus},
    },
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge, wayland_server::protocol::wl_surface::WlSurface},
    utils::SERIAL_COUNTER,
};

use crate::{Treewm, grabs::{PanCanvasGrab, ResizeSurfaceGrab}, state::{ModifierKey, ViewMode}};

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

                let keyboard = self.seat.get_keyboard().expect("Keyboard not found while trying to add it");
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
                        
                        let main_modifier = data.main_modifier;
                        let main_mod = match main_modifier {
                            ModifierKey::Ctrl => modifiers.ctrl,
                            ModifierKey::Alt => modifiers.alt,
                            ModifierKey::Shift => modifiers.shift,
                            ModifierKey::Super => modifiers.logo,
                        };
                        // ── Window resizing (Ctrl + Shift + Arrow) ─────────────
                        // Must be checked before the plain Ctrl+Arrow pan block.
                        if main_mod && modifiers.shift {
                            if let Some(fid) = data.focused_window_id {
                                if let Some(cw) = data.windows.iter_mut().find(|cw| cw.id == fid) {
                                    match sym {
                                        Keysym::Left  => cw.base_width  = (cw.base_width  - 32).max(128),
                                        Keysym::Right => cw.base_width  =  cw.base_width  + 32,
                                        Keysym::Up    => cw.base_height = (cw.base_height - 32).max(128),
                                        Keysym::Down  => cw.base_height =  cw.base_height + 32,
                                        _ => {}
                                    }
                                    data.apply_layout();
                                    return FilterResult::Intercept(());
                                }
                            }
                        }

                        // ── Viewport panning (Ctrl + Arrow / Home) ──────────────
                        if main_mod && !modifiers.shift {
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

                        if main_mod && modifiers.alt && sym == Keysym::BackSpace {
                            data.loop_signal.stop();
                            return FilterResult::Intercept(());
                        }

                        // ── View mode toggle (Ctrl + Space) ─────────────────────
                        if main_mod && sym == Keysym::space {
                            toggle_view_mode = true;
                            return FilterResult::Intercept(());
                        }

                        // ── Tree navigation (Ctrl + P / N / C) ──────────────────
                        if main_mod {
                            if sym == Keysym::q {
                                data.windows
                                    .iter()
                                    .find(|cw| cw.id == data.focused_window_id.expect("No focused window to close"))
                                    .and_then(|cw| cw.window.toplevel()
                                    .map(|t| t.send_close()));
                                return FilterResult::Intercept(());
                            }
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
                        if main_mod && sym == Keysym::z {
                            focus_zoom_requested = true;
                            return FilterResult::Intercept(());
                        }

                        if main_mod && sym == Keysym::f {
                            data.toggle_fullscreen();
                            return FilterResult::Intercept(())
                        }

                        FilterResult::Forward
                    },
                );

                // Apply view mode toggle (keyboard mutex now released).
                if toggle_view_mode {
                    self.view_mode = match self.view_mode {
                        ViewMode::Tiling => {
                            self.zoom_anim_start = self.zoom;
                            self.zoom_target = 0.7;
                            self.zoom_animating = true;
                            ViewMode::TreeView
                        },
                        ViewMode::TreeView => {
                            self.tiling_root_id = self.focused_window_id;
                            self.zoom = 1.0;
                            self.zoom_target = 1.0;
                            self.zoom_anim_start = 1.0;
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
                let output = self.space.outputs().next().expect("No other monitors connected. Either went through all, or none are connected");
                let output_geo = self.space.output_geometry(output).expect("Monitor connected but not fully configured, so geometry couldnt be drawn");
                let pos =
                    event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().expect("No pointer/mouse connected or found");
                let under = self.surface_under(pos);
                let keyboard = self.seat.get_keyboard().expect("Keyboard not found - this is a bug");
                pointer.motion(
                    self,
                    under.clone(),
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);

                if let Some((wl_surf, _)) = under {           
                    if let Some(window) = self.windows.iter().find(|cw| {
                            cw.window
                                .toplevel()
                                .map_or(false, |t| t.wl_surface() == &wl_surf)
                        }) {
                        let window_id = window.id;

                        let wx = (window.canvas_x - self.viewport_x) as i32;
                        let wy = (window.canvas_y - self.viewport_y) as i32;
                        let ww = window.base_width as i32;
                        let wh = window.base_height as i32;
                        let px = pointer.current_location().x as i32;
                        let py = pointer.current_location().y as i32;

                        if px > wx + ww - 8 && py > wy + wh - 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::SeResize); }
                        else if px > wx + ww - 8 && py < wy + 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::NeResize); }
                        else if px < wx + 8 && py < wy + 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::NwResize); }
                        else if px < wx + 8 && py > wy + wh - 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::SwResize); }
                        else if px > wx + ww - 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::EResize); }
                        else if px < wx + 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::WResize); }
                        else if py > wy + wh - 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::SResize); }
                        else if py < wy + 8 { self.cursor_icon = CursorImageStatus::Named(CursorIcon::NResize); }
                        else { self.cursor_icon = CursorImageStatus::default_named(); }

                        if self.config.hover_to_focus {
                            keyboard.set_focus(self, Some(wl_surf.clone()), serial);
                            self.focused_window_id = Some(window_id);
                        }
                    }
                    
                }
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().expect("No pointer/mouse connected or found");
                let keyboard = self.seat.get_keyboard().expect("Keyboard not found - this is a bug");
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                const BTN_MIDDLE: u32 = 0x112;
                const BTN_LEFT: u32 = 0x110;
                
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
                } else if ButtonState::Pressed == button_state && !pointer.is_grabbed()
                    && button == BTN_LEFT
                {
                    let win_positions = self
                        .windows
                        .iter()
                        .find(|cw|{
                            let wx = (cw.canvas_x - self.viewport_x) as i32;
                            let wy = (cw.canvas_y - self.viewport_y) as i32;
                            let ww = cw.base_width as i32;
                            let wh = cw.base_height as i32;
                            let px = pointer.current_location().x as i32;
                            let py = pointer.current_location().y as i32;

                            !((wx..(wx+ww)).contains(&px) && (wy..(wy+wh)).contains(&py)) && ((wx-8..(wx+ww+8)).contains(&px) && (wy-8..wy+wh+8).contains(&py))
                    });
                    let mut edge: ResizeEdge = ResizeEdge::None;
                    if let Some(cw) = win_positions {
                        let wx = (cw.canvas_x - self.viewport_x) as i32;
                        let wy = (cw.canvas_y - self.viewport_y) as i32;
                        let ww = cw.base_width as i32;
                        let wh = cw.base_height as i32;
                        let px = pointer.current_location().x as i32;
                        let py = pointer.current_location().y as i32;

                        if px > wx + ww - 8 && py > wy + wh - 8 { edge = ResizeEdge::BottomRight; }
                        else if px > wx + ww - 8 && py < wy + 8 { edge = ResizeEdge::TopRight; }
                        else if px < wx + 8 && py < wy + 8 { edge = ResizeEdge::TopLeft; }
                        else if px < wx + 8 && py > wy + wh - 8 { edge = ResizeEdge::BottomLeft; }
                        else if px > wx + ww - 8 { edge = ResizeEdge::Right; }
                        else if px < wx + 8 { edge = ResizeEdge::Left; }
                        else if py > wy + wh - 8 { edge = ResizeEdge::Bottom; }
                        else if py < wy + 8 { edge = ResizeEdge::Top; }

                        let surface = cw.window.toplevel().expect("Window doesnt have a top level").wl_surface().clone();

                        let grab = ResizeSurfaceGrab {
                            start_data: PointerGrabStartData {
                                focus: None,
                                button: BTN_LEFT,
                                location: pointer.current_location(),
                            },
                            window_surface: surface,
                            initial_width: ww,
                            initial_height: wh,
                            initial_canvas_x: cw.canvas_x,
                            initial_canvas_y: cw.canvas_y,
                            grabbed_edge: edge, 
                        };
                        pointer.set_grab(self, grab, serial, Focus::Clear);
                    }
                } else if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        let wl_surf = window.toplevel().expect("Couldn't get ToplevelSurface as window is a popup").wl_surface().clone();
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
                                    window.toplevel().expect("Couldn't get ToplevelSurface as window is a popup").send_pending_configure();
                                });
                            }
                            ViewMode::TreeView => {}
                        }
                    } else {
                        self.space.elements().for_each(|window| {
                            window.set_activated(false);
                            window.toplevel().expect("Couldn't get ToplevelSurface as window is a popup").send_pending_configure();
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
                    let pointer = self.seat.get_pointer().expect("No pointer/mouse connected or found");
                    let pointer_loc = pointer.current_location();

                    let old_zoom = self.zoom;
                    let zoom_factor = 1.1_f64.powf(-vertical_amount / 15.0);
                    self.zoom = (self.zoom * zoom_factor).clamp(0.2, 5.0);
                    self.zoom_target = self.zoom;

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

                let pointer = self.seat.get_pointer().expect("No pointer/mouse connected or found");
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}
