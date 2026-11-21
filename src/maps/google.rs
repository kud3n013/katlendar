//! Google Maps, for those who give the app an API key of their own:
//! places from Places Autocomplete (New) and Place Details (New), the
//! address at a spot from the Geocoding API, and map tiles from the Map
//! Tiles API.
//!
//! What Google's terms ask, and how it is kept:
//! - a search and the details of the place it ends in share one session
//!   token, which is made anew after each place is picked;
//! - a place's id may be kept for good, so it is;
//! - the latitude and longitude Google gives may be kept 30 days at most,
//!   so a spot from Google is let go after that (see `Place::expire`), and
//!   it is never written to a calendar server;
//! - tiles are kept in memory only, never on disk;
//! - "Google Maps" and the map data's copyright are shown under the map.
//!
//! The key is kept in the keyring and goes into the question only as it
//! is sent, so it is never part of what is kept in memory as a question.

use std::sync::RwLock;

use serde_json::{json, Value};

use super::osm::{Ask, Found, Kind};
use crate::sync::oauth::url_encode;

/// The name the key is kept under in the keyring.
pub const SECRET: &str = "maps:google";
/// Results a search brings back; Autocomplete gives at most five.
const LIMIT: usize = 5;
/// The furthest a search looks around the map, in metres: Google's limit.
const NEAR_RADIUS: f64 = 50_000.0;
/// The details asked for a place: those billed as Essentials.
const DETAILS_FIELDS: &str = "id,formattedAddress,location,viewport";

static KEY: RwLock<Option<String>> = RwLock::new(None);

/// The API key, once it has been read from the keyring or given.
pub fn key() -> Option<String> {
    KEY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Puts the key in use, or none.
pub fn set_key(key: Option<String>) {
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    *KEY.write().unwrap_or_else(|e| e.into_inner()) = key;
}

/// Reads the key from the keyring, off the window's thread.
pub fn load_key(allow_file: bool) {
    crate::worker::spawn("maps-key", move || {
        if let Ok(k) = crate::secrets::get(SECRET, allow_file) {
            set_key(Some(k));
        }
    });
}

/// Keeps `key` in the keyring and puts it in use; an empty one is
/// forgotten. Says how keeping it went, off the window's thread.
pub fn save_key(key: String, allow_file: bool, done: impl FnOnce(Result<(), String>) + Send + 'static) {
    let key = key.trim().to_string();
    set_key(Some(key.clone()));
    crate::worker::spawn("maps-key", move || {
        if key.is_empty() {
            crate::secrets::delete(SECRET);
            done(Ok(()));
        } else {
            done(crate::secrets::set(SECRET, &key, allow_file));
        }
    });
}

/// A new session token: a random UUID, as Google suggests.
pub fn new_session() -> String {
    let b: [u8; 16] = rand::random();
    let h: String = b.iter().enumerate().map(|(i, x)| if i == 6 { format!("{:02x}", (x & 0x0f) | 0x40) } else if i == 8 { format!("{:02x}", (x & 0x3f) | 0x80) } else { format!("{x:02x}") }).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// The person's language and country, as Google takes them: "en-GB" and
/// "GB" from LANG=en_GB.UTF-8. Without a country, the language alone.
pub fn locale() -> (String, Option<String>) {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
    read_locale(&env)
}

fn read_locale(env: &str) -> (String, Option<String>) {
    let base = env.split(['.', '@']).next().unwrap_or("");
    let mut parts = base.split(['_', '-']);
    let lang = parts.next().filter(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic())).map(str::to_ascii_lowercase);
    let region = parts.next().filter(|r| r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic())).map(str::to_ascii_uppercase);
    match (lang, region) {
        (Some(l), Some(r)) => (format!("{l}-{r}"), Some(r)),
        (Some(l), None) if l != "c" => (l, None),
        _ => ("en".into(), None),
    }
}

/// The search for `q` in session `session`, with places near `near` (a
/// spot and a map zoom) first.
pub fn suggest(q: &str, session: &str, near: Option<((f64, f64), f64)>) -> Ask {
    let (lang, region) = locale();
    let mut body = json!({ "input": q.trim(), "sessionToken": session, "languageCode": lang });
    if let Some(r) = region {
        body["regionCode"] = json!(r.to_ascii_lowercase());
    }
    // the spot rounded, as for Photon: a small move of the map asks nothing new
    if let Some(((lat, lon), zoom)) = near.filter(|n| n.1 >= 4.0) {
        let round = |x: f64| (x * 100.0).round() / 100.0;
        // about what the map shows, within Google's limit
        let radius = (40_075_000.0 / zoom.exp2()).clamp(1_000.0, NEAR_RADIUS).round();
        body["locationBias"] = json!({ "circle": { "center": { "latitude": round(lat), "longitude": round(lon) }, "radius": radius } });
    }
    Ask { url: "https://places.googleapis.com/v1/places:autocomplete".into(), body: Some(body.to_string()), kind: Kind::GoogleSuggest }
}

/// The details of a place picked from a search, which ends its session.
pub fn details(place_id: &str, session: &str) -> Ask {
    let (lang, _) = locale();
    let url = format!("https://places.googleapis.com/v1/places/{}?sessionToken={}&languageCode={}", url_encode(place_id), url_encode(session), url_encode(&lang));
    Ask { url, body: None, kind: Kind::GoogleDetails }
}

/// The address at a spot.
pub fn reverse((lat, lon): (f64, f64)) -> Ask {
    let (lang, _) = locale();
    let url = format!("https://maps.googleapis.com/maps/api/geocode/json?latlng={lat:.6},{lon:.6}&language={}", url_encode(&lang));
    Ask { url, body: None, kind: Kind::GoogleReverse }
}

/// Asks Google; the key goes in now.
pub fn fetch(agent: &ureq::Agent, ask: &Ask) -> Result<String, String> {
    let key = key().ok_or("no Google Maps API key is set")?;
    let resp = match ask.kind {
        Kind::GoogleSuggest => agent
            .post(&ask.url)
            .header("X-Goog-Api-Key", &key)
            .header("Content-Type", "application/json")
            .send(ask.body.as_deref().unwrap_or("{}")),
        Kind::GoogleDetails => agent.get(&ask.url).header("X-Goog-Api-Key", &key).header("X-Goog-FieldMask", DETAILS_FIELDS).call(),
        Kind::GoogleReverse => agent.get(&format!("{}&key={}", ask.url, url_encode(&key))).call(),
        Kind::Photon => return Err("not a question for Google".into()),
    };
    let mut resp = resp.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = resp.body_mut().with_config().limit(2 << 20).read_to_string().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(error_of(&body).unwrap_or_else(|| format!("Google answered {status}")));
    }
    Ok(body)
}

/// What a failed answer says went wrong: `error.message` from Places and
/// the tile server, `error_message` from Geocoding.
pub fn error_of(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v["error"]["message"].as_str().or(v["error_message"].as_str()).map(str::to_string)
}

/// The places in an Autocomplete answer. They have no spot yet: that
/// comes with their details.
pub fn parse_suggestions(body: &str) -> Result<Vec<Found>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    // no suggestions comes as an empty answer
    for s in v["suggestions"].as_array().into_iter().flatten() {
        let p = &s["placePrediction"];
        let Some(id) = p["placeId"].as_str().filter(|i| !i.is_empty()) else { continue };
        let main = p["structuredFormat"]["mainText"]["text"].as_str().map(str::trim).unwrap_or("");
        let rest = p["structuredFormat"]["secondaryText"]["text"].as_str().map(str::trim).unwrap_or("");
        let whole = p["text"]["text"].as_str().map(str::trim).unwrap_or("");
        let (name, detail) = if main.is_empty() { (whole.to_string(), String::new()) } else { (main.to_string(), rest.to_string()) };
        let text = if !main.is_empty() && !rest.is_empty() { format!("{main}, {rest}") } else if whole.is_empty() { name.clone() } else { whole.to_string() };
        if text.is_empty() {
            continue;
        }
        out.push(Found { name, detail, text, geo: None, extent: None, place_id: Some(id.to_string()) });
        if out.len() == LIMIT {
            break;
        }
    }
    Ok(out)
}

fn lat_lng(v: &Value) -> Option<(f64, f64)> {
    let lat = v["latitude"].as_f64().or(v["lat"].as_f64())?;
    let lon = v["longitude"].as_f64().or(v["lng"].as_f64())?;
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

/// A place's details: its spot, and the area it covers.
pub fn parse_details(body: &str) -> Result<Vec<Found>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let geo = lat_lng(&v["location"]).ok_or("the place has no location")?;
    let text = v["formattedAddress"].as_str().unwrap_or("").trim().to_string();
    let extent = match (lat_lng(&v["viewport"]["low"]), lat_lng(&v["viewport"]["high"])) {
        (Some((s, w)), Some((n, e))) => Some([s, w, n, e]),
        _ => None,
    };
    let place_id = v["id"].as_str().map(str::to_string);
    Ok(vec![Found { name: text.clone(), detail: String::new(), text, geo: Some(geo), extent, place_id }])
}

/// The address at a spot, from Geocoding. Nothing known there is an
/// empty list, not a failure.
pub fn parse_reverse(body: &str) -> Result<Vec<Found>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    match v["status"].as_str().unwrap_or("") {
        "OK" => {}
        "ZERO_RESULTS" => return Ok(vec![]),
        s => return Err(v["error_message"].as_str().map(str::to_string).unwrap_or_else(|| format!("Google answered {s}"))),
    }
    let Some(r) = v["results"].as_array().and_then(|r| r.first()) else { return Ok(vec![]) };
    let text = r["formatted_address"].as_str().unwrap_or("").trim().to_string();
    let geo = lat_lng(&r["geometry"]["location"]);
    Ok(vec![Found { name: text.clone(), detail: String::new(), text, geo, extent: None, place_id: r["place_id"].as_str().map(str::to_string) }])
}

/// The answer to a question for Google, read.
pub fn parse(kind: Kind, body: &str) -> Result<Vec<Found>, String> {
    match kind {
        Kind::GoogleSuggest => parse_suggestions(body),
        Kind::GoogleDetails => parse_details(body),
        Kind::GoogleReverse => parse_reverse(body),
        Kind::Photon => Err("not an answer from Google".into()),
    }
}

/// The map tile session's settings.
pub fn session_request() -> Value {
    let (lang, region) = locale();
    let lang = if lang.contains('-') { lang } else { format!("{lang}-US") };
    json!({ "mapType": "roadmap", "language": lang, "region": region.unwrap_or_else(|| "US".into()), "imageFormat": "png" })
}

/// The session token and when it runs out, in seconds since 1970, from
/// the answer to `createSession`.
pub fn parse_session(body: &str) -> Result<(String, i64), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let session = v["session"].as_str().filter(|s| !s.is_empty()).ok_or("no session in the answer")?.to_string();
    let expiry = v["expiry"].as_str().and_then(|e| e.parse().ok()).or(v["expiry"].as_i64()).unwrap_or(0);
    Ok((session, expiry))
}

/// The copyright of the map data in an area, from the viewport answer.
pub fn parse_copyright(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v["copyright"].as_str().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_locale_is_written_for_google() {
        assert_eq!(read_locale("en_GB.UTF-8"), ("en-GB".into(), Some("GB".into())));
        assert_eq!(read_locale("de_DE@euro"), ("de-DE".into(), Some("DE".into())));
        assert_eq!(read_locale("fr"), ("fr".into(), None));
        assert_eq!(read_locale("C.UTF-8"), ("en".into(), None));
        assert_eq!(read_locale(""), ("en".into(), None));
    }

    #[test]
    fn a_session_token_is_a_uuid() {
        let t = new_session();
        assert_eq!(t.len(), 36);
        assert_eq!(&t[14..15], "4");
        assert!(t.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert_ne!(t, new_session());
    }

    #[test]
    fn a_search_keeps_its_session_and_looks_near_the_map() {
        let a = suggest(" café ", "tok", Some(((51.51361, -0.13652), 15.0)));
        assert_eq!(a.kind, Kind::GoogleSuggest);
        assert_eq!(a.url, "https://places.googleapis.com/v1/places:autocomplete");
        let b: Value = serde_json::from_str(a.body.as_deref().unwrap()).unwrap();
        assert_eq!(b["input"], "café");
        assert_eq!(b["sessionToken"], "tok");
        assert_eq!(b["locationBias"]["circle"]["center"]["latitude"], 51.51);
        assert_eq!(b["locationBias"]["circle"]["radius"], 1223.0);
        // the whole world is no hint
        let a = suggest("x", "tok", Some(((0.0, 0.0), 2.0)));
        assert!(!a.body.unwrap().contains("locationBias"));
        // the key is never part of the question
        set_key(None);
        assert!(!details("ChIJ+x", "tok").url.contains("key="));
        assert!(details("ChIJ+x", "tok").url.starts_with("https://places.googleapis.com/v1/places/ChIJ%2Bx?sessionToken=tok&languageCode="));
        assert!(reverse((51.5, -0.1)).url.starts_with("https://maps.googleapis.com/maps/api/geocode/json?latlng=51.500000,-0.100000&language="));
    }

    #[test]
    fn suggestions_read_as_one_line() {
        let body = r#"{"suggestions":[
            {"placePrediction":{"place":"places/A","placeId":"A","text":{"text":"Café Lisboa, Rua Augusta, Lisbon, Portugal"},
             "structuredFormat":{"mainText":{"text":"Café Lisboa"},"secondaryText":{"text":"Rua Augusta, Lisbon, Portugal"}}}},
            {"placePrediction":{"placeId":"B","text":{"text":"Lisbon, Portugal"}}},
            {"queryPrediction":{"text":{"text":"cafés in Lisbon"}}}
        ]}"#;
        let got = parse_suggestions(body).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].name.as_str(), got[0].detail.as_str()), ("Café Lisboa", "Rua Augusta, Lisbon, Portugal"));
        assert_eq!(got[0].text, "Café Lisboa, Rua Augusta, Lisbon, Portugal");
        assert_eq!((got[0].geo, got[0].place_id.as_deref()), (None, Some("A")));
        assert_eq!(got[1].text, "Lisbon, Portugal");
        assert_eq!(parse_suggestions("{}").unwrap(), vec![]);
        assert!(parse_suggestions("nope").is_err());
    }

    #[test]
    fn details_give_the_spot_and_the_area() {
        let body = r#"{"id":"A","formattedAddress":"R. Augusta 1, Lisboa, Portugal","location":{"latitude":38.71,"longitude":-9.137},
            "viewport":{"low":{"latitude":38.70,"longitude":-9.14},"high":{"latitude":38.72,"longitude":-9.13}}}"#;
        let f = &parse_details(body).unwrap()[0];
        assert_eq!(f.geo, Some((38.71, -9.137)));
        assert_eq!(f.extent, Some([38.70, -9.14, 38.72, -9.13]));
        assert_eq!(f.place_id.as_deref(), Some("A"));
        assert!(parse_details(r#"{"id":"A"}"#).is_err());
    }

    #[test]
    fn the_address_at_a_spot() {
        let body = r#"{"status":"OK","results":[{"formatted_address":"12 High St, London W1D 3QF, UK","place_id":"P","geometry":{"location":{"lat":51.5136,"lng":-0.1365}}}]}"#;
        let f = &parse_reverse(body).unwrap()[0];
        assert_eq!((f.text.as_str(), f.place_id.as_deref(), f.geo), ("12 High St, London W1D 3QF, UK", Some("P"), Some((51.5136, -0.1365))));
        assert_eq!(parse_reverse(r#"{"status":"ZERO_RESULTS","results":[]}"#).unwrap(), vec![]);
        assert_eq!(parse_reverse(r#"{"status":"REQUEST_DENIED","error_message":"The provided API key is invalid."}"#), Err("The provided API key is invalid.".into()));
    }

    #[test]
    fn failures_say_what_went_wrong() {
        assert_eq!(error_of(r#"{"error":{"code":403,"message":"API key not valid.","status":"PERMISSION_DENIED"}}"#).as_deref(), Some("API key not valid."));
        assert_eq!(error_of(r#"{"error_message":"over quota","status":"OVER_QUERY_LIMIT"}"#).as_deref(), Some("over quota"));
        assert_eq!(error_of("<html>"), None);
    }

    #[test]
    fn tile_sessions_and_copyright() {
        assert_eq!(parse_session(r#"{"session":"S1","expiry":"1361828036","tileWidth":256,"imageFormat":"png"}"#), Ok(("S1".into(), 1361828036)));
        assert!(parse_session("{}").is_err());
        let r = session_request();
        assert_eq!((r["mapType"].as_str(), r["imageFormat"].as_str()), (Some("roadmap"), Some("png")));
        assert!(r["language"].as_str().unwrap().contains('-'), "a language and a country");
        assert_eq!(parse_copyright(r#"{"copyright":"Map data ©2026 Google","maxZoomRects":[]}"#).as_deref(), Some("Map data ©2026 Google"));
        assert_eq!(parse_copyright(r#"{"copyright":""}"#), None);
    }
}
