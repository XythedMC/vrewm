use smithay::{
    delegate_cursor_shape,
    wayland::cursor_shape::CursorShapeManagerState,
};
use crate::Treewm;

delegate_cursor_shape!(Treewm);