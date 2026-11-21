//! Looking places up on Photon, the OpenStreetMap search that allows
//! searching as you type: by name or address, and the address at a spot.
//! Questions for Google, when it is the map in use, go the same way.
//!
//! One question is asked at a time, on a worker thread. A search waits
//! until the typing has paused, and every answer is kept, so going back
//! to an earlier word asks nothing.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::Value;

use super::USER_AGENT;
use crate::sync::oauth::url_encode;

/// The public Photon server, free to use fairly.
const PHOTON: &str = "https://photon.komoot.io";
/// How long typing has to pause before a search goes out.
pub const PAUSE: Duration = Duration::from_millis(300);
/// The least time between two questions to the server.
const GAP: Duration = Duration::from_millis(250);
/// A question that failed is asked again after this long.
const RETRY: Duration = Duration::from_secs(20);
/// Results a search brings back.
const LIMIT: usize = 6;
/// Answers kept; past this they are all let go.
const KEEP: usize = 300;

/// A place the server knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// the first line: its name, or its street address when it has none
    pub name: String,
    /// the rest of the address
    pub detail: String,
    /// the one line the place is saved as: "Name, street, postcode city, country"
    pub text: String,
    /// latitude and longitude; a place from a Google search has none
    /// until its details are looked up
    pub geo: Option<(f64, f64)>,
    /// the area a town or a country covers: south, west, north, east
    pub extent: Option<[f64; 4]>,
    /// Google's id for the place
    pub place_id: Option<String>,
}

/// Who a question is for, and how its answer reads.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Photon,
    GoogleSuggest,
    GoogleDetails,
    GoogleReverse,
}

/// A question to a server: what is asked, never the key it is asked with.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Ask {
    pub url: String,
    /// what is sent, for a question that is posted
    pub body: Option<String>,
    pub kind: Kind,
}

impl Ask {
    pub fn photon(url: String) -> Ask {
        Ask { url, body: None, kind: Kind::Photon }
    }
}

/// What the server said to a question.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Waiting,
    Found(Vec<Found>),
    Failed(String),
}

/// The search for `q`, with places near `near` (a spot and a map zoom)
/// first. The spot is rounded, so a small move of the map asks nothing new.
pub fn search_url(q: &str, near: Option<((f64, f64), f64)>) -> String {
    let mut url = format!("{PHOTON}/api/?q={}&limit={LIMIT}", url_encode(q.trim()));
    if let Some(l) = lang() {
        url += &format!("&lang={l}");
    }
    // a map of the whole world says nothing about where to look
    if let Some(((lat, lon), zoom)) = near.filter(|n| n.1 >= 4.0) {
        url += &format!("&lat={lat:.2}&lon={lon:.2}&zoom={}", zoom.round() as u8);
    }
    url
}

/// The address at a spot.
pub fn reverse_url((lat, lon): (f64, f64)) -> String {
    let mut url = format!("{PHOTON}/reverse?lat={lat:.6}&lon={lon:.6}&limit=1");
    if let Some(l) = lang() {
        url += &format!("&lang={l}");
    }
    url
}

/// The language of the person's system, when Photon has names in it;
/// otherwise names come in the place's own language.
fn lang() -> Option<&'static str> {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))?;
    ["en", "de", "fr"].into_iter().find(|l| env.starts_with(l))
}

/// Countries whose addresses put the house number before the street.
const NUMBER_FIRST: &[&str] = &["AU", "CA", "FR", "GB", "IE", "IN", "LU", "MY", "NZ", "PH", "SG", "US", "ZA"];
/// Countries whose addresses put the postcode after the town.
const POSTCODE_AFTER: &[&str] = &["AU", "CA", "GB", "US"];

/// The places in a Photon answer, a GeoJSON feature list. The same
/// place twice (a building and its entrance, say) is shown once.
pub fn parse(body: &str) -> Result<Vec<Found>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let features = v["features"].as_array().ok_or("no features in the answer")?;
    let mut out: Vec<Found> = Vec::new();
    for f in features.iter().filter_map(found) {
        if !out.iter().any(|o| o.text == f.text) {
            out.push(f);
        }
    }
    Ok(out)
}

fn found(f: &Value) -> Option<Found> {
    let c = f["geometry"]["coordinates"].as_array()?;
    let geo = (c.get(1)?.as_f64()?, c.first()?.as_f64()?);
    let p = &f["properties"];
    let s = |k: &str| p[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let cc = s("countrycode").unwrap_or("").to_uppercase();
    let numbered = |st: &str, n: &str| if NUMBER_FIRST.contains(&cc.as_str()) { format!("{n} {st}") } else { format!("{st} {n}") };
    let street = match (s("street"), s("housenumber")) {
        (Some(st), Some(n)) => Some(numbered(st, n)),
        (Some(st), None) => Some(st.to_string()),
        // a numbered house whose street is its name
        (None, Some(n)) => s("name").map(|name| numbered(name, n)),
        (None, None) => None,
    };
    // "High Street", on High Street, is said once, with its number if it has one
    let name = s("name").filter(|n| {
        let (n, st) = (n.to_lowercase(), street.as_deref().unwrap_or("").to_lowercase());
        !(st == n || st.starts_with(&format!("{n} ")) || st.ends_with(&format!(" {n}")))
    });
    let town = s("city").or(s("town")).or(s("village")).or(s("locality"));
    let town = match (s("postcode"), town) {
        (Some(pc), Some(t)) if POSTCODE_AFTER.contains(&cc.as_str()) => Some(format!("{t} {pc}")),
        (Some(pc), Some(t)) => Some(format!("{pc} {t}")),
        (pc, t) => t.or(pc).map(str::to_string),
    };
    // a town's own result has no town in it: its region says which one it is
    let region = if town.is_none() { s("state").map(str::to_string) } else { None };
    let mut parts: Vec<String> = Vec::new();
    for part in [name.map(str::to_string), street, town, region, s("country").map(str::to_string)].into_iter().flatten() {
        if !parts.iter().any(|p| p.eq_ignore_ascii_case(&part)) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return None;
    }
    let extent = p["extent"].as_array().and_then(|e| {
        let n: Vec<f64> = e.iter().filter_map(Value::as_f64).collect();
        // Photon writes west, north, east, south
        (n.len() == 4).then(|| [n[1].min(n[3]), n[0].min(n[2]), n[1].max(n[3]), n[0].max(n[2])])
    });
    Some(Found { name: parts[0].clone(), detail: parts[1..].join(", "), text: parts.join(", "), geo: Some(geo), extent, place_id: None })
}

/// The questions asked and their answers, without the clock or the network.
#[derive(Default)]
struct Book {
    answers: HashMap<Ask, (Answer, Instant)>,
    /// the question to ask next, and when it may go out
    wanted: Option<(Ask, Instant)>,
    /// the question out now
    asking: Option<Ask>,
}

impl Book {
    /// The answer to `url`. When there is none yet, it is the question to
    /// ask next, `wait` from now; a question asked again keeps its place.
    fn want(&mut self, url: &Ask, now: Instant, wait: Duration) -> Answer {
        match self.answers.get(url) {
            Some((Answer::Failed(_), at)) if now.duration_since(*at) >= RETRY => {}
            Some((a, _)) => return a.clone(),
            None => {}
        }
        if self.asking.as_ref() == Some(url) {
            return Answer::Waiting;
        }
        match &self.wanted {
            Some((u, _)) if u == url => {}
            _ => self.wanted = Some((url.clone(), now + wait)),
        }
        Answer::Waiting
    }

    /// The question to ask now; else when the next one may go (or none is wanted).
    fn next(&mut self, now: Instant) -> Result<Ask, Option<Instant>> {
        match self.wanted.take() {
            Some((url, due)) if due <= now => {
                self.asking = Some(url.clone());
                Ok(url)
            }
            other => {
                let due = other.as_ref().map(|w| w.1);
                self.wanted = other;
                Err(due)
            }
        }
    }

    fn done(&mut self, url: Ask, answer: Answer, now: Instant) {
        if self.answers.len() >= KEEP {
            self.answers.clear();
        }
        self.answers.insert(url, (answer, now));
        self.asking = None;
    }

    fn busy(&self) -> bool {
        self.wanted.is_some() || self.asking.is_some()
    }
}

pub struct Lookups {
    book: Mutex<Book>,
    wake: Condvar,
    ctx: egui::Context,
    started: Mutex<bool>,
}

static LOOKUPS: OnceLock<Arc<Lookups>> = OnceLock::new();

/// The app's one place for questions to Photon and Google.
pub fn lookups(ctx: &egui::Context) -> Arc<Lookups> {
    LOOKUPS.get_or_init(|| Arc::new(Lookups { book: Mutex::default(), wake: Condvar::new(), ctx: ctx.clone(), started: Mutex::new(false) })).clone()
}

/// Whether a question is waiting to go or out now.
pub fn busy() -> bool {
    LOOKUPS.get().is_some_and(|l| l.lock().busy())
}

impl Lookups {
    fn lock(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The answer to `url`, asked for `wait` from now when there is none
    /// yet. Asked again each frame, it goes out once.
    pub fn get(self: &Arc<Self>, url: &Ask, wait: Duration) -> Answer {
        let answer = self.lock().want(url, Instant::now(), wait);
        if answer == Answer::Waiting {
            let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
            // tests never reach the network
            if !*started && !cfg!(test) {
                *started = true;
                let me = self.clone();
                crate::worker::spawn("places", move || me.work());
            }
            self.wake.notify_one();
        }
        answer
    }

    /// Test aid: the server's answer to `url`, as if it had come.
    #[cfg(test)]
    pub fn answer(&self, url: &Ask, answer: Answer) {
        let mut b = self.lock();
        b.want(url, Instant::now(), Duration::ZERO);
        b.done(url.clone(), answer, Instant::now());
    }

    fn work(&self) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut last: Option<Instant> = None;
        loop {
            let url = {
                let mut b = self.lock();
                loop {
                    let now = Instant::now();
                    // the server is asked at most so often
                    let free = last.map_or(now, |l| l + GAP);
                    if now < free && b.wanted.is_some() {
                        b = self.wake.wait_timeout(b, free - now).unwrap_or_else(|e| e.into_inner()).0;
                        continue;
                    }
                    match b.next(now) {
                        Ok(url) => break url,
                        Err(Some(due)) => b = self.wake.wait_timeout(b, due.saturating_duration_since(now)).unwrap_or_else(|e| e.into_inner()).0,
                        Err(None) => b = self.wake.wait(b).unwrap_or_else(|e| e.into_inner()),
                    }
                }
            };
            last = Some(Instant::now());
            let answer = fetch(&agent, &url);
            self.lock().done(url, answer, Instant::now());
            self.ctx.request_repaint();
        }
    }
}

fn fetch(agent: &ureq::Agent, ask: &Ask) -> Answer {
    if ask.kind != Kind::Photon {
        return match super::google::fetch(agent, ask).and_then(|body| super::google::parse(ask.kind, &body)) {
            Ok(found) => Answer::Found(found),
            Err(e) => Answer::Failed(e),
        };
    }
    let mut resp = match agent.get(&ask.url).header("User-Agent", USER_AGENT).call() {
        Ok(r) => r,
        Err(e) => return Answer::Failed(e.to_string()),
    };
    if !resp.status().is_success() {
        return Answer::Failed(format!("the server answered {}", resp.status()));
    }
    match resp.body_mut().with_config().limit(2 << 20).read_to_string() {
        Ok(body) => parse(&body).map_or_else(Answer::Failed, Answer::Found),
        Err(e) => Answer::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(lon: f64, lat: f64, props: &str) -> String {
        format!(r#"{{"type":"Feature","geometry":{{"type":"Point","coordinates":[{lon},{lat}]}},"properties":{props}}}"#)
    }

    fn answer(features: &[String]) -> String {
        format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, features.join(","))
    }

    #[test]
    fn places_read_as_one_line_that_stands_alone() {
        let body = answer(&[
            // a named place in Germany: street then number, postcode then town
            feature(13.3777, 52.5163, r#"{"name":"Brandenburger Tor","street":"Pariser Platz","housenumber":"1","postcode":"10117","city":"Berlin","country":"Germany","countrycode":"DE"}"#),
            // a British house with no name: the number comes first, the postcode last
            feature(-0.1365, 51.5136, r#"{"street":"High Street","housenumber":"12","postcode":"W1D 3QF","city":"London","country":"United Kingdom","countrycode":"GB","type":"house"}"#),
            // a town, with the area it covers
            feature(2.35, 48.85, r#"{"name":"Paris","state":"Île-de-France","country":"France","countrycode":"FR","type":"city","extent":[2.22,48.90,2.47,48.81]}"#),
            // the same building again
            feature(13.3778, 52.5164, r#"{"name":"Brandenburger Tor","street":"Pariser Platz","housenumber":"1","postcode":"10117","city":"Berlin","country":"Germany","countrycode":"DE"}"#),
            // nothing to call it by
            feature(0.0, 0.0, r#"{}"#),
        ]);
        let got = parse(&body).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].text, "Brandenburger Tor, Pariser Platz 1, 10117 Berlin, Germany");
        assert_eq!((got[0].name.as_str(), got[0].detail.as_str()), ("Brandenburger Tor", "Pariser Platz 1, 10117 Berlin, Germany"));
        assert_eq!(got[0].geo, Some((52.5163, 13.3777)));
        assert_eq!(got[1].text, "12 High Street, London W1D 3QF, United Kingdom");
        assert_eq!(got[1].name, "12 High Street");
        assert_eq!(got[2].text, "Paris, Île-de-France, France");
        assert_eq!(got[2].extent, Some([48.81, 2.22, 48.90, 2.47]));
        assert!(parse("not json").is_err());
        assert_eq!(parse(r#"{"features":[]}"#).unwrap(), vec![]);
    }

    #[test]
    fn a_street_is_named_once() {
        // a street's own result is named after the street it is on
        let body = answer(&[feature(-0.13, 51.51, r#"{"name":"High Street","street":"High Street","postcode":"W1","city":"London","country":"United Kingdom","countrycode":"GB"}"#)]);
        assert_eq!(parse(&body).unwrap()[0].text, "High Street, London W1, United Kingdom");
        // a shop named after its street keeps its number
        let body = answer(&[feature(-0.13, 51.51, r#"{"name":"High Street","street":"High Street","housenumber":"4","city":"London","countrycode":"GB"}"#)]);
        assert_eq!(parse(&body).unwrap()[0].text, "4 High Street, London");
    }

    #[test]
    fn questions_are_written_for_photon() {
        let url = search_url(" café & co ", Some(((51.51361, -0.13652), 15.6)));
        assert!(url.starts_with("https://photon.komoot.io/api/?q=caf%C3%A9%20%26%20co&limit=6"), "{url}");
        assert!(url.ends_with("&lat=51.51&lon=-0.14&zoom=16"), "{url}");
        // the whole world is no hint
        assert!(!search_url("x", Some(((0.0, 0.0), 2.0))).contains("&lat="));
        let url = reverse_url((51.5136, -0.1365));
        assert!(url.starts_with("https://photon.komoot.io/reverse?lat=51.513600&lon=-0.136500&limit=1"), "{url}");
    }

    fn a(url: &str) -> Ask {
        Ask::photon(url.to_string())
    }

    #[test]
    fn a_search_waits_for_the_typing_to_pause() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut b = Book::default();
        assert_eq!(b.want(&a("ca"), t0, PAUSE), Answer::Waiting);
        assert_eq!(b.next(ms(100)), Err(Some(ms(300))));
        // another letter starts the wait again
        assert_eq!(b.want(&a("caf"), ms(200), PAUSE), Answer::Waiting);
        assert_eq!(b.next(ms(400)), Err(Some(ms(500))));
        // the same question each frame keeps its place in line
        b.want(&a("caf"), ms(450), PAUSE);
        assert_eq!(b.next(ms(500)), Ok(a("caf")));
        assert!(b.busy());
        // out now: asking again sends nothing more
        assert_eq!(b.want(&a("caf"), ms(520), PAUSE), Answer::Waiting);
        assert_eq!(b.next(ms(900)), Err(None));
        b.done(a("caf"), Answer::Found(vec![]), ms(700));
        assert!(!b.busy());
        // kept: asked again, it is there at once
        assert_eq!(b.want(&a("caf"), ms(800), PAUSE), Answer::Found(vec![]));
        assert_eq!(b.next(ms(2000)), Err(None));
        // a failure is kept a while, then asked again
        b.done(a("x"), Answer::Failed("down".into()), ms(1000));
        assert_eq!(b.want(&a("x"), ms(2000), Duration::ZERO), Answer::Failed("down".into()));
        assert_eq!(b.want(&a("x"), ms(1000) + RETRY, Duration::ZERO), Answer::Waiting);
        assert_eq!(b.next(ms(1000) + RETRY), Ok(a("x")));
    }
}
