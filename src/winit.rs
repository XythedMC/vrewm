use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::Kind,
            gles::{
                element::PixelShaderElement,
                GlesPixelProgram, GlesRenderer, Uniform, UniformName, UniformType,
            },
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{state::ViewMode, Treewm};

// ── Shader sources ─────────────────────────────────────────────────────────────
// compile_custom_pixel_shader prepends "#version 100\n" — do NOT include it here.

/// Straight-line connector with anti-aliased edges and endpoint dots.
/// Uses highp + premultiplied alpha output (Smithay blends with GL_ONE, GL_ONE_MINUS_SRC_ALPHA).
const LINE_FRAG: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform vec2  p_start;
uniform vec2  p_end;
uniform float thickness;
uniform vec4  u_color;
uniform vec2  elem_size;

float dist_segment(vec2 p, vec2 a, vec2 b) {
    vec2  ab = b - a;
    float t  = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return length(p - a - t * ab);
}

void main() {
    vec2  px = v_coords * elem_size;
    float d  = dist_segment(px, p_start, p_end);

    // Filled circles at endpoints (radius 4 px).
    float dot_r = 4.0;
    d = min(d, max(length(px - p_start) - dot_r, 0.0));
    d = min(d, max(length(px - p_end)   - dot_r, 0.0));

    float ht = thickness * 0.5;
    float a  = 1.0 - smoothstep(ht - 1.0, ht + 1.5, d);
    float fa = u_color.a * a;
    // Premultiplied alpha: transparent pixels must output (0,0,0,0), not (r,g,b,0).
    gl_FragColor = vec4(u_color.rgb * fa, fa);
}
"#;

/// Solid-color rectangle — used for the mode indicator square (premultiplied).
const SOLID_FRAG: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec4 u_color;
void main() {
    gl_FragColor = vec4(u_color.rgb * u_color.a, u_color.a);
}
"#;

// ── Shader compilation ─────────────────────────────────────────────────────────

fn compile_line(r: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    r.compile_custom_pixel_shader(
        LINE_FRAG,
        &[
            UniformName::new("p_start",   UniformType::_2f),
            UniformName::new("p_end",     UniformType::_2f),
            UniformName::new("thickness", UniformType::_1f),
            UniformName::new("u_color",   UniformType::_4f),
            UniformName::new("elem_size", UniformType::_2f),
        ],
    )
    .map_err(|e| eprintln!("treewm: line shader compile failed: {e}"))
    .ok()
}

fn compile_solid(r: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    r.compile_custom_pixel_shader(
        SOLID_FRAG,
        &[UniformName::new("u_color", UniformType::_4f)],
    )
    .map_err(|e| eprintln!("treewm: solid shader compile failed: {e}"))
    .ok()
}

// ── Per-frame element builders ─────────────────────────────────────────────────

fn line_element(prog: &GlesPixelProgram, start: (f32, f32), end: (f32, f32)) -> PixelShaderElement {
    let pad   = 8.0_f32;
    let min_x = start.0.min(end.0) - pad;
    let min_y = start.1.min(end.1) - pad;
    let max_x = start.0.max(end.0) + pad;
    let max_y = start.1.max(end.1) + pad;
    let ew    = (max_x - min_x).max(1.0);
    let eh    = (max_y - min_y).max(1.0);

    // Convert to element-local pixel coordinates.
    let ls = (start.0 - min_x, start.1 - min_y);
    let le = (end.0   - min_x, end.1   - min_y);

    let area = Rectangle {
        loc: (min_x as i32, min_y as i32).into(),
        size: (ew as i32, eh as i32).into(),
    };

    PixelShaderElement::new(
        prog.clone(),
        area,
        None,
        1.0,
        vec![
            Uniform::new("p_start",   (ls.0, ls.1)),
            Uniform::new("p_end",     (le.0, le.1)),
            Uniform::new("thickness", 2.0_f32),
            Uniform::new("u_color",   (0.55_f32, 0.78_f32, 1.0_f32, 0.45_f32)),
            Uniform::new("elem_size", (ew, eh)),
        ],
        Kind::Unspecified,
    )
}

fn connector_elements(state: &Treewm, prog: &GlesPixelProgram) -> Vec<PixelShaderElement> {
    state
        .windows
        .iter()
        .filter_map(|cw| {
            let pid    = cw.parent_id?;
            let parent = state.windows.iter().find(|p| p.id == pid)?;

            let px = (parent.canvas_x - state.viewport_x) as f32;
            let py = (parent.canvas_y - state.viewport_y) as f32;
            let cx = (cw.canvas_x    - state.viewport_x) as f32;
            let cy = (cw.canvas_y    - state.viewport_y) as f32;

            // Parent bottom-center → child top-center.
            Some(line_element(prog, (px + 400.0, py + 600.0), (cx + 400.0, cy)))
        })
        .collect()
}

fn focus_border_elements(state: &Treewm, prog: &GlesPixelProgram) -> Vec<PixelShaderElement> {
    let fid = match state.focused_window_id {
        Some(id) => id,
        None => return Vec::new(),
    };
    let cw = match state.windows.iter().find(|cw| cw.id == fid) {
        Some(cw) => cw,
        None => return Vec::new(),
    };

    let sx = (cw.canvas_x - state.viewport_x) as i32;
    let sy = (cw.canvas_y - state.viewport_y) as i32;
    let ww = 800_i32;
    let wh = 600_i32;
    let t = 2_i32;
    let color = (1.0_f32, 0.90_f32, 0.3_f32, 1.0_f32); // bright amber, premultiplied handled by shader

    let rects = [
        Rectangle { loc: (sx,       sy - t     ).into(), size: (ww,         t          ).into() },
        Rectangle { loc: (sx,       sy + wh    ).into(), size: (ww,         t          ).into() },
        Rectangle { loc: (sx - t,   sy - t     ).into(), size: (t,      wh + 2 * t     ).into() },
        Rectangle { loc: (sx + ww,  sy - t     ).into(), size: (t,      wh + 2 * t     ).into() },
    ];

    rects.iter().map(|&area| {
        PixelShaderElement::new(
            prog.clone(),
            area,
            Some(vec![area]),
            1.0,
            vec![Uniform::new("u_color", color)],
            Kind::Unspecified,
        )
    }).collect()
}

fn indicator_element(state: &Treewm, prog: &GlesPixelProgram) -> PixelShaderElement {
    let color: (f32, f32, f32, f32) = match state.view_mode {
        ViewMode::Tiling   => (0.25, 0.85, 0.45, 0.85), // green
        ViewMode::TreeView => (0.35, 0.60, 1.00, 0.85), // blue
    };
    let area = Rectangle {
        loc: (12, 12).into(),
        size: (18, 18).into(),
    };
    PixelShaderElement::new(
        prog.clone(),
        area,
        Some(vec![area]),
        1.0,
        vec![Uniform::new("u_color", color)],
        Kind::Unspecified,
    )
}

// ── Winit backend init ─────────────────────────────────────────────────────────

pub fn init_winit(
    event_loop: &mut EventLoop<Treewm>,
    state: &mut Treewm,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut backend, winit) = winit::init()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<Treewm>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    // Compile overlay shaders once, before the event loop starts.
    let line_prog  = compile_line(backend.renderer());
    let solid_prog = compile_solid(backend.renderer());

    backend.window().request_redraw();

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            match event {
                WinitEvent::Resized { size, .. } => {
                    output.change_current_state(
                        Some(Mode { size, refresh: 60_000 }),
                        None,
                        None,
                        None,
                    );
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => {
                    state.tick_animation();

                    let size   = backend.window_size();
                    let damage = Rectangle::from_size(size);

                    {
                        let (renderer, mut framebuffer) = match backend.bind() {
                            Ok(v)  => v,
                            Err(e) => { eprintln!("treewm: bind error: {e}"); return; }
                        };

                        // Assemble overlay elements for this frame.
                        let mut overlays: Vec<PixelShaderElement> = Vec::new();

                        if state.view_mode == ViewMode::TreeView {
                            if let Some(prog) = &line_prog {
                                overlays.extend(connector_elements(state, prog));
                            }
                            if let Some(prog) = &solid_prog {
                                overlays.extend(focus_border_elements(state, prog));
                            }
                        }
                        if let Some(prog) = &solid_prog {
                            overlays.push(indicator_element(state, prog));
                        }

                        if let Err(e) = smithay::desktop::space::render_output::<
                            _,
                            PixelShaderElement,
                            _,
                            _,
                        >(
                            &output,
                            renderer,
                            &mut framebuffer,
                            1.0,
                            0,
                            [&state.space],
                            &overlays,
                            &mut damage_tracker,
                            [0.1, 0.1, 0.1, 1.0],
                        ) {
                            eprintln!("treewm: render error: {e}");
                            return;
                        }
                    }

                    if let Err(e) = backend.submit(Some(&[damage])) {
                        eprintln!("treewm: submit error: {e}");
                        return;
                    }

                    state.space.elements().for_each(|window| {
                        window.send_frame(
                            &output,
                            state.start_time.elapsed(),
                            Some(Duration::ZERO),
                            |_, _| Some(output.clone()),
                        )
                    });

                    state.space.refresh();
                    state.popups.cleanup();
                    let _ = state.display_handle.flush_clients();

                    backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => {
                    state.loop_signal.stop();
                }
                _ => (),
            };
        })?;

    Ok(())
}
