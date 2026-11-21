//! Maps: tiles from OpenStreetMap or Google, a map to drag and zoom,
//! places looked up on Photon or Google, and the dialog that picks a
//! place with them.

pub mod google;
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

/// Where the map and the places come from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Source {
    #[default]
    Osm,
    Google,
}

impl Source {
    /// The source for a choice of map: Google only while there is a key.
    pub fn of(provider: crate::model::MapProvider) -> Source {
        match provider {
            crate::model::MapProvider::Google if google::key().is_some() => Source::Google,
            _ => Source::Osm,
        }
    }
}
