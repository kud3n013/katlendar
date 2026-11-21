//! Maps: tiles from OpenStreetMap, a map to drag and zoom, places looked
//! up on Photon, and the dialog that picks a place with them.

mod osm;
pub mod picker;
mod tiles;
mod widget;

pub use widget::{show, MapView};

/// The tile and search policies ask for a name that tells this app apart.
const USER_AGENT: &str = concat!("katlendar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/kud3n013/katlendar)");

/// Whether a tile or a search is on its way; a picture of the app waits for it.
pub fn busy() -> bool {
    tiles::busy() || osm::busy()
}
