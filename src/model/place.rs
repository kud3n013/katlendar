//! Where an event or task happens: the text people read, and the spot
//! on a map when one is known.

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default, Debug, PartialEq)]
pub struct Place {
    /// what is shown, and sent as the event's location
    pub text: String,
    /// latitude and longitude in degrees, when known
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<(f64, f64)>,
}

impl Place {
    /// A place with only a text; None when the text is empty.
    pub fn named(text: &str) -> Option<Place> {
        let text = text.trim();
        (!text.is_empty()).then(|| Place { text: text.to_string(), geo: None })
    }
    /// A place from what a server sent; None when it sent nothing.
    pub fn of(text: &str, geo: Option<(f64, f64)>) -> Option<Place> {
        let text = text.trim();
        (!text.is_empty() || geo.is_some()).then(|| Place { text: text.to_string(), geo })
    }
}

/// The text of a place, empty for none.
pub fn place_text(p: &Option<Place>) -> &str {
    p.as_ref().map(|p| p.text.as_str()).unwrap_or("")
}

/// Sets the text of a place as typed; it is kept as typed, so a space
/// being typed stays. A new text drops the spot on the map, which
/// belonged to the old one; an empty text drops the place.
pub fn set_place_text(p: &mut Option<Place>, text: &str) {
    if text.trim().is_empty() {
        *p = None;
    } else if let Some(old) = p.as_mut().filter(|o| o.text.trim() == text.trim()) {
        old.text = text.to_string();
    } else {
        *p = Some(Place { text: text.to_string(), geo: None });
    }
}

/// Where "Open in" takes a place.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum OpenIn {
    OpenStreetMap,
    Google,
    Apple,
    /// a `geo:` link, for the map app this computer has
    MapApp,
}

impl Default for OpenIn {
    /// Apple Maps on a Mac, which has it; OpenStreetMap elsewhere, which
    /// asks for no account.
    fn default() -> Self {
        if cfg!(target_os = "macos") { OpenIn::Apple } else { OpenIn::OpenStreetMap }
    }
}

impl OpenIn {
    /// The targets this system can open: a map app by `geo:` link only
    /// where the desktop hands such links to one.
    pub fn all() -> Vec<OpenIn> {
        let mut v = vec![OpenIn::OpenStreetMap, OpenIn::Google, OpenIn::Apple];
        if cfg!(not(any(windows, target_os = "macos"))) {
            v.push(OpenIn::MapApp);
        }
        v
    }
    pub fn name(self) -> &'static str {
        match self {
            OpenIn::OpenStreetMap => "OpenStreetMap",
            OpenIn::Google => "Google Maps",
            OpenIn::Apple => "Apple Maps",
            OpenIn::MapApp => crate::strings::tr("Map app"),
        }
    }
    /// The link that shows `p` there: its spot when known, else a search
    /// for its text. None for a place with neither.
    pub fn link(self, p: &Place) -> Option<String> {
        let enc = crate::sync::oauth::url_encode;
        let text = p.text.trim();
        let geo = p.geo.map(|(lat, lon)| (geo_number(lat), geo_number(lon)));
        if text.is_empty() && geo.is_none() {
            return None;
        }
        Some(match (self, geo) {
            (OpenIn::OpenStreetMap, Some((lat, lon))) => format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=17/{lat}/{lon}"),
            (OpenIn::OpenStreetMap, None) => format!("https://www.openstreetmap.org/search?query={}", enc(text)),
            (OpenIn::Google, Some((lat, lon))) => format!("https://www.google.com/maps/search/?api=1&query={lat}%2C{lon}"),
            (OpenIn::Google, None) => format!("https://www.google.com/maps/search/?api=1&query={}", enc(text)),
            // the text names the pin
            (OpenIn::Apple, Some((lat, lon))) => {
                let name = if text.is_empty() { format!("{lat},{lon}") } else { text.to_string() };
                format!("https://maps.apple.com/?ll={lat},{lon}&q={}", enc(&name))
            }
            (OpenIn::Apple, None) => format!("https://maps.apple.com/?q={}", enc(text)),
            (OpenIn::MapApp, Some((lat, lon))) => format!("geo:{lat},{lon}"),
            // the search form map apps take, from Android's
            (OpenIn::MapApp, None) => format!("geo:0,0?q={}", enc(text)),
        })
    }
}

/// A latitude and longitude in range.
fn in_range(lat: f64, lon: f64) -> Option<(f64, f64)> {
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

/// The value of an iCalendar GEO line, "lat;lon".
pub fn parse_geo(value: &str) -> Option<(f64, f64)> {
    let (a, b) = value.trim().split_once(';')?;
    in_range(a.trim().parse().ok()?, b.trim().parse().ok()?)
}

/// A geo URI (RFC 5870), "geo:lat,lon[,alt][;u=…]".
pub fn parse_geo_uri(value: &str) -> Option<(f64, f64)> {
    let v = value.trim();
    let rest = v.get(..4).filter(|p| p.eq_ignore_ascii_case("geo:")).map(|_| &v[4..])?;
    let coords = rest.split(';').next()?;
    let mut parts = coords.split(',');
    let lat = parts.next()?.trim().parse().ok()?;
    let lon = parts.next()?.trim().parse().ok()?;
    in_range(lat, lon)
}

/// Writes coordinates the way GEO and geo URIs take them: six decimals,
/// about ten centimetres, with no trailing zeros.
pub fn geo_number(x: f64) -> String {
    let s = format!("{x:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// Older versions kept the place as a first line "Where: …" in the
/// notes. Takes it out: the place, and the notes without it.
pub fn split_where(notes: &str) -> (Option<Place>, String) {
    let mut lines = notes.splitn(2, '\n');
    let first = lines.next().unwrap_or("");
    match first.trim_end_matches('\r').strip_prefix("Where: ") {
        Some(p) => (Place::named(p), lines.next().unwrap_or("").to_string()),
        None => (None, notes.to_string()),
    }
}

/// Two spots that write the same at six decimals.
pub fn same_geo(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> bool {
    match (a, b) {
        (Some((a1, a2)), Some((b1, b2))) => geo_number(a1) == geo_number(b1) && geo_number(a2) == geo_number(b2),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_coordinates() {
        assert_eq!(parse_geo("37.386013;-122.082932"), Some((37.386013, -122.082932)));
        assert_eq!(parse_geo(" 1 ; 2 "), Some((1.0, 2.0)));
        assert_eq!(parse_geo("91;0"), None, "out of range");
        assert_eq!(parse_geo("1,2"), None);
        assert_eq!(parse_geo_uri("geo:48.2010,16.3695,183;u=40"), Some((48.201, 16.3695)));
        assert_eq!(parse_geo_uri("GEO:-33.9,151.2"), Some((-33.9, 151.2)));
        assert_eq!(parse_geo_uri("geo:1;2"), None);
        assert_eq!(parse_geo_uri("http://x"), None);
        assert_eq!(parse_geo_uri("geo:1,181"), None);
    }

    #[test]
    fn writes_coordinates() {
        assert_eq!(geo_number(52.52), "52.52");
        assert_eq!(geo_number(13.0), "13");
        assert_eq!(geo_number(-0.0000001), "0");
        assert_eq!(geo_number(1.23456789), "1.234568");
        assert!(same_geo(Some((1.0000001, 2.0)), Some((1.0, 2.0))));
        assert!(!same_geo(Some((1.00001, 2.0)), Some((1.0, 2.0))));
        assert!(!same_geo(None, Some((1.0, 2.0))));
    }

    #[test]
    fn where_lines() {
        assert_eq!(split_where("Where: Room 4\nBring notes"), (Place::named("Room 4"), "Bring notes".to_string()));
        assert_eq!(split_where("Where: Room 4"), (Place::named("Room 4"), String::new()));
        assert_eq!(split_where("Notes\nWhere: later"), (None, "Notes\nWhere: later".to_string()));
        assert_eq!(split_where(""), (None, String::new()));
    }

    #[test]
    fn open_in_links() {
        let spot = Place { text: "Café Lisboa, Rua 1".into(), geo: Some((38.7, -9.140000)) };
        let words = Place::named("Café Lisboa, Rua 1").unwrap();
        let links = |p: &Place| [OpenIn::OpenStreetMap, OpenIn::Google, OpenIn::Apple, OpenIn::MapApp].map(|o| o.link(p).unwrap());
        assert_eq!(
            links(&spot),
            [
                "https://www.openstreetmap.org/?mlat=38.7&mlon=-9.14#map=17/38.7/-9.14",
                "https://www.google.com/maps/search/?api=1&query=38.7%2C-9.14",
                "https://maps.apple.com/?ll=38.7,-9.14&q=Caf%C3%A9%20Lisboa%2C%20Rua%201",
                "geo:38.7,-9.14",
            ]
        );
        assert_eq!(
            links(&words),
            [
                "https://www.openstreetmap.org/search?query=Caf%C3%A9%20Lisboa%2C%20Rua%201",
                "https://www.google.com/maps/search/?api=1&query=Caf%C3%A9%20Lisboa%2C%20Rua%201",
                "https://maps.apple.com/?q=Caf%C3%A9%20Lisboa%2C%20Rua%201",
                "geo:0,0?q=Caf%C3%A9%20Lisboa%2C%20Rua%201",
            ]
        );
        // a spot with no name still has a pin; nothing at all has no link
        assert_eq!(OpenIn::Apple.link(&Place { text: " ".into(), geo: Some((1.0, 2.0)) }).as_deref(), Some("https://maps.apple.com/?ll=1,2&q=1%2C2"));
        assert_eq!(OpenIn::Google.link(&Place::default()), None);
        // what is typed is sent whole: signs can't end the link early
        assert!(OpenIn::Google.link(&Place::named("A&B #2?x=1").unwrap()).unwrap().ends_with("query=A%26B%20%232%3Fx%3D1"));
    }

    #[test]
    fn typing_keeps_the_spot_only_for_the_same_place() {
        let mut p = Place::of("Cafe", Some((1.0, 2.0)));
        set_place_text(&mut p, "Cafe ");
        assert_eq!(p, Some(Place { text: "Cafe ".into(), geo: Some((1.0, 2.0)) }));
        set_place_text(&mut p, "Cafe B");
        assert_eq!(p, Some(Place { text: "Cafe B".into(), geo: None }));
        set_place_text(&mut p, "  ");
        assert_eq!(p, None);
        assert_eq!(Place::of("", Some((1.0, 2.0))), Some(Place { text: String::new(), geo: Some((1.0, 2.0)) }));
        assert_eq!(Place::of(" ", None), None);
    }
}
