use smithay::{
    backend::{
        renderer::{
            ImportDma, ImportEgl, damage::OutputDamageTracker, element::{AsRenderElements, Kind, surface::WaylandSurfaceRenderElement}, gles::{
                GlesPixelProgram, GlesRenderer, Uniform, UniformName, UniformType, element::PixelShaderElement
            }
        }, winit::{self, WinitEvent}
    }, desktop::{Space, Window, layer_map_for_output}, input::pointer::{CursorIcon, CursorImageStatus}, output::{Mode, Output, PhysicalProperties, Subpixel}, reexports::calloop::EventLoop, utils::{Logical, Rectangle, Scale, Transform}
};

use crate::{Treewm, handlers::config::TreeWMConfig, state::{BackgroundType, CanvasWindow, TreewmElement, ViewMode}};

pub const LINE_FRAG: &str = r#"
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
pub const SOLID_FRAG: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec4 u_color;
void main() {
    gl_FragColor = vec4(u_color.rgb * u_color.a, u_color.a);
}
"#;

pub const BORDER_FRAG: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform vec2 elem_size;
uniform float radius;
uniform vec4 u_color;
uniform float thickness;

void main() {
    vec2 px = v_coords * elem_size;
    vec2 p = px - elem_size / 2.0;

    vec2 d = abs(p) - elem_size / 2.0 + vec2(radius);
    float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
    if (dist > 0.0) { discard; }
    if (dist < -thickness) { discard; }
    gl_FragColor = vec4(u_color.rgb * u_color.a, u_color.a);
}   
"#;
// ── Shader compilation ─────────────────────────────────────────────────────────

pub fn compile_line(r: &mut GlesRenderer) -> Option<GlesPixelProgram> {
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

pub fn compile_solid(r: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    r.compile_custom_pixel_shader(
        SOLID_FRAG,
        &[UniformName::new("u_color", UniformType::_4f)],
    )
    .map_err(|e| eprintln!("treewm: solid shader compile failed: {e}"))
    .ok()
}

pub fn compile_border(r: &mut GlesRenderer) -> Option<GlesPixelProgram> {
        r.compile_custom_pixel_shader(
        BORDER_FRAG,
        &[
            UniformName::new("elem_size", UniformType::_2f),
            UniformName::new("radius", UniformType::_1f),
            UniformName::new("thickness", UniformType::_1f),
            UniformName::new("u_color", UniformType::_4f),
        ],
    )
    .map_err(|e| eprintln!("treewm: border shader compile failed: {e}"))
    .ok()
}

// ── Per-frame element builders ─────────────────────────────────────────────────

pub fn line_element(prog: &GlesPixelProgram, start: (f32, f32), end: (f32, f32)) -> PixelShaderElement {
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

pub fn connector_elements(windows: &[CanvasWindow], viewport_x: f64, viewport_y: f64, prog: &GlesPixelProgram) -> Vec<PixelShaderElement> {
    windows
        .iter()
        .filter_map(|cw| {
            if cw.is_fullscreen { return None; }
            let pid    = cw.parent_id?;
            let parent = windows.iter().find(|p| p.id == pid)?;

            let px = (parent.canvas_x - viewport_x) as f32;
            let py = (parent.canvas_y - viewport_y) as f32;
            let cx = (cw.canvas_x    - viewport_x) as f32;
            let cy = (cw.canvas_y    - viewport_y) as f32;

            let phw = parent.base_width  as f32 / 2.0;
            let ph  = parent.base_height as f32;
            let chw = cw.base_width      as f32 / 2.0;

            // Parent bottom-center → child top-center.
            Some(line_element(prog, (px + phw, py + ph), (cx + chw, cy)))
        })
        .collect()
}

pub fn focus_border_elements(
    focused_window_id: Option<u32>,
    config: TreeWMConfig,
    zoom: f64,
    prog: &GlesPixelProgram, 
    cw: &CanvasWindow, 
    geo: Rectangle<i32, Logical>
) -> PixelShaderElement {
    let fid = focused_window_id;
    let sx = ((geo.loc.x as f64 - config.border_width as f64) / zoom) as i32;
    let sy = ((geo.loc.y as f64 - config.border_width as f64) / zoom) as i32;
    let wh = ((geo.size.h + (config.border_width as i32 * 2)) as f64) as i32;
    let ww = ((geo.size.w + (config.border_width as i32 * 2)) as f64) as i32;
    let t = config.border_width;
    let mut color = [0.0, 0.0, 0.0, 1.0];

    if Some(cw.id) == fid {
        let [r, g, b] = config.focused_border_color;
        color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    }
    else {
        let [r, g, b] = config.unfocused_border_color;
        color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    }
    
    let area = Rectangle { loc: (sx, sy).into(), size: (ww, wh).into() };

    PixelShaderElement::new(
        prog.clone(),
        area,
        None, // Damage = None means Smithay handles it or we force full redraw
        1.0,
        vec![
            Uniform::new("u_color", color),
            Uniform::new("elem_size", (ww as f32 * zoom as f32, wh as f32 * zoom as f32)),
            Uniform::new("radius", config.corner_rounding),
            Uniform::new("thickness", t as f32)
        ],
        Kind::Unspecified,
    )
}

pub fn indicator_element(view_mode: ViewMode, prog: &GlesPixelProgram) -> PixelShaderElement {
    let color: (f32, f32, f32, f32) = match view_mode {
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
        None,
        1.0,
        vec![Uniform::new("u_color", color)],
        Kind::Unspecified,
    )
}

pub fn build_render_elements(
    windows: &[CanvasWindow],
    space: &Space<Window>,
    focused_window_id: Option<u32>,
    view_mode: ViewMode,
    tiling_visible_ids: &[u32],
    scale: f64,
    zoom: f64,
    viewport_x: f64,
    viewport_y: f64,
    config: &TreeWMConfig,
    renderer: &mut GlesRenderer,
    line_prog: &Option<GlesPixelProgram>, 
    solid_prog: &Option<GlesPixelProgram>,
    border_prog: &Option<GlesPixelProgram>
) ->Vec<TreewmElement> {
    // Assemble overlay elements for this frame.
    let mut overlays: Vec<TreewmElement> = Vec::new();
    let (focused, unfocused): (Vec<&CanvasWindow>, Vec<&CanvasWindow>) = windows.iter().partition(|cw| Some(cw.id) == focused_window_id );
    let output = space.outputs().next().unwrap().clone();
    for focused_window in focused {
        if view_mode == ViewMode::Tiling && !tiling_visible_ids.contains(&focused_window.id) {continue;}
    
        if let Some(geo) = space.element_geometry(&focused_window.window) {
            if let Some(prog) = &border_prog {
                overlays.push(TreewmElement::Shader(focus_border_elements(Some(focused_window.id), config.clone(), zoom, prog, focused_window, geo)));
            }
            overlays.extend(
                focused_window.window.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    renderer,
                    geo.loc.to_physical_precise_round(scale),
                    Scale::from(scale),
                    1.0,
                ).into_iter().map(TreewmElement::Surface)
            );
        }
    }
    for unfocused_window in unfocused {
        if view_mode == ViewMode::Tiling && !tiling_visible_ids.contains(&unfocused_window.id) {continue;}
        if let Some(geo) = space.element_geometry(&unfocused_window.window) {
            if let Some(prog) = &border_prog {
                overlays.push(TreewmElement::Shader(focus_border_elements(Some(unfocused_window.id), config.clone(), zoom, prog, unfocused_window, geo)));
            }
            overlays.extend(
                unfocused_window.window.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    renderer,
                    geo.loc.to_physical_precise_round(scale),
                    Scale::from(scale),
                    1.0,
                ).into_iter().map(TreewmElement::Surface)
            );
        }
    }

    // Render layer surfaces (wlr-layer-shell: background/bottom/top/overlay).
    {
        let layer_map = smithay::desktop::layer_map_for_output(&output);
        for layer in layer_map.layers() {
            let loc = layer_map.layer_geometry(layer).unwrap_or_default().loc;
            overlays.extend(
                layer.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    renderer,
                    loc.to_physical_precise_round(scale),
                    Scale::from(scale),
                    1.0,
                ).into_iter().map(TreewmElement::Surface)
            );
        }
    }

    if let Some(prog) = &solid_prog {
        overlays.push(TreewmElement::Shader(indicator_element(view_mode, prog)));
    }
    if view_mode == ViewMode::TreeView {
        if let Some(prog) = &line_prog {
            overlays.extend(connector_elements(windows, viewport_x, viewport_y, prog).into_iter().map(TreewmElement::Shader));
        }
    }
    overlays
}