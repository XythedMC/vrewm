use smithay::{
    delegate_layer_shell, desktop::{WindowSurfaceType, layer_map_for_output}, 
    reexports::wayland_server::protocol::wl_output::WlOutput, 
    wayland::shell::wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler},
};
use crate::Treewm;

impl WlrLayerShellHandler for Treewm {
    fn shell_state(&mut self) -> &mut smithay::wayland::shell::wlr_layer::WlrLayerShellState {
        &mut self.wlr_layer_shell_state
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let output = self.space.outputs().next().unwrap();                                                                                  
        let mut map = layer_map_for_output(&output);
        if let Some(_layer) = map.layer_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL) {                                     
            let layer = map.layer_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL).cloned();                                      
            if let Some(layer) = layer {                                                                                                        
                map.unmap_layer(&layer);                                                                                                        
            }                                                                                        
        }
        self.layer_surfaces.retain(|s| s.wl_surface() != surface.wl_surface());
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = self.space.outputs().next().unwrap();
        let layer_surface = smithay::desktop::LayerSurface::new(surface.clone(), namespace);
        layer_map_for_output(&output).map_layer(&layer_surface).unwrap();
        surface.send_configure();
        self.layer_surfaces.push(layer_surface);
    }
}

delegate_layer_shell!(Treewm);