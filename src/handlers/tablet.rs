use smithay::{delegate_tablet_manager, wayland::tablet_manager::TabletSeatHandler};
use crate::Treewm;

impl TabletSeatHandler for Treewm {
    
}
delegate_tablet_manager!(Treewm);