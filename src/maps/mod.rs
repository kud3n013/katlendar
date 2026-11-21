//! Maps: tiles from OpenStreetMap, and a map to drag and zoom.

mod tiles;
mod widget;

pub use tiles::busy;
pub use widget::{show, MapView};
