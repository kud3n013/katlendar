//! Map tiles from OpenStreetMap or Google. A tile is fetched on a worker
//! thread the first time it is shown, and kept in memory as a texture
//! while it is being looked at. One from OpenStreetMap is also kept on
//! disk for as long as the server says (a week when it says nothing, as
//! its tile policy asks); one from Google never is, as Google's terms
//! ask. Nothing is fetched before it is needed on screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;

use super::{google, Source, USER_AGENT};

/// The side of a tile, in pixels.
pub const TILE: f64 = 256.0;
/// Who to thank, under every map from OpenStreetMap, and where that is explained.
pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";
pub const COPYRIGHT: &str = "https://www.openstreetmap.org/copyright";
/// What Google asks to be shown under its maps, before the data's copyright.
pub const GOOGLE_ATTRIBUTION: &str = "Google Maps";
pub const GOOGLE_TERMS: &str = "https://www.google.com/intl/en/help/terms_maps/";
const GOOGLE_TILES: &str = "https://tile.googleapis.com";

impl Source {
    /// The closest zoom the tile server draws.
    pub fn max_zoom(self) -> u8 {
        match self {
            Source::Osm => 19,
            Source::Google => 21,
        }
    }
}

/// Textures kept in memory; the ones looked at longest ago go first.
const IN_MEMORY: usize = 256;
/// Tiles waiting to be fetched; past this the oldest wish is dropped,
/// as it is likely scrolled out of sight.
const QUEUE: usize = 64;
const WORKERS: usize = 2;
/// How long a tile is kept when the server says nothing: a week.
const KEEP_SECS: i64 = 7 * 24 * 3600;
/// How long a tile that could not be had is left before it is tried again.
const RETRY: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TileId {
    pub src: Source,
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// The tile one zoom out that covers this one.
    pub fn parent(self) -> Option<TileId> {
        (self.z > 0).then(|| TileId { src: self.src, z: self.z - 1, x: self.x / 2, y: self.y / 2 })
    }
    fn valid(self) -> bool {
        self.z <= self.src.max_zoom() && self.x < 1 << self.z && self.y < 1 << self.z
    }
    fn url(self) -> String {
        format!("https://tile.openstreetmap.org/{}/{}/{}.png", self.z, self.x, self.y)
    }
    /// Where the tile is kept under the cache folder.
    fn path(self, dir: &Path) -> PathBuf {
        dir.join("osm").join(self.z.to_string()).join(self.x.to_string()).join(format!("{}.png", self.y))
    }
}

enum Slot {
    /// asked for, not here yet
    Waiting,
    /// here, and the frame it was last drawn in
    Ready(egui::TextureHandle, u64),
    /// could not be had, at that time
    Failed(Instant),
}

/// The part of the world a map shows, for Google's copyright: a zoom
/// and the tiles at its corners, west, north, east, south.
pub type Area = (u8, [u32; 4]);

#[derive(Default)]
struct State {
    slots: HashMap<TileId, Slot>,
    /// the last one in is fetched first: it is the one just asked for
    queue: Vec<TileId>,
    started: bool,
    /// the copyright of Google's map data in the area last shown, and
    /// the area it was asked for
    credit: Option<(Area, Option<String>)>,
    /// the area whose copyright is to be asked next
    credit_wanted: Option<Area>,
}

/// Google's tile session: the key it was made with, its token, and when
/// it runs out (seconds since 1970).
struct Session {
    key: String,
    token: String,
    expiry: i64,
}

pub struct Tiles {
    state: Mutex<State>,
    wake: Condvar,
    ctx: egui::Context,
    dir: PathBuf,
    session: Mutex<Option<Session>>,
}

static TILES: OnceLock<Arc<Tiles>> = OnceLock::new();

/// The app's one set of tiles.
pub fn tiles(ctx: &egui::Context) -> Arc<Tiles> {
    TILES.get_or_init(|| Arc::new(Tiles { state: Mutex::default(), wake: Condvar::new(), ctx: ctx.clone(), dir: cache_dir(), session: Mutex::new(None) })).clone()
}

/// Whether a tile is on its way; a picture of the app waits for it.
pub fn busy() -> bool {
    TILES.get().is_some_and(|t| {
        let s = t.lock();
        s.credit_wanted.is_some() || s.slots.values().any(|s| matches!(s, Slot::Waiting))
    })
}

/// Beside the app's data: `…/katlendar/tiles`.
fn cache_dir() -> PathBuf {
    crate::model::data_path().with_file_name("tiles")
}

impl Tiles {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The tile's texture when it is here; else it is asked for, once.
    pub fn get(self: &Arc<Self>, id: TileId) -> Option<egui::TextureId> {
        if let Some(t) = self.peek(id) {
            return Some(t);
        }
        let mut s = self.lock();
        match s.slots.get(&id) {
            Some(Slot::Waiting | Slot::Ready(..)) => return None,
            Some(Slot::Failed(at)) if at.elapsed() < RETRY => return None,
            _ if !id.valid() => return None,
            _ => {}
        }
        s.slots.insert(id, Slot::Waiting);
        s.queue.push(id);
        if s.queue.len() > QUEUE {
            let old = s.queue.remove(0);
            s.slots.remove(&old);
        }
        self.start(s);
        None
    }

    /// Starts the workers the first time, and wakes one.
    fn start(self: &Arc<Self>, mut s: MutexGuard<'_, State>) {
        // tests ask for tiles without fetching them
        let start = !std::mem::replace(&mut s.started, true) && !cfg!(test);
        drop(s);
        if start {
            for i in 0..WORKERS {
                let me = self.clone();
                crate::worker::spawn(&format!("tiles-{i}"), move || me.work());
            }
        }
        self.wake.notify_one();
    }

    /// The copyright of Google's map data in `area`: the one last known
    /// while the new one is asked for.
    pub fn credit(self: &Arc<Self>, area: Area) -> Option<String> {
        let mut s = self.lock();
        let known = s.credit.as_ref().and_then(|c| c.1.clone());
        if s.credit.as_ref().is_some_and(|c| c.0 == area) || s.credit_wanted == Some(area) {
            return known;
        }
        s.credit_wanted = Some(area);
        self.start(s);
        known
    }

    /// The tile's texture when it is here, without asking for it.
    pub fn peek(&self, id: TileId) -> Option<egui::TextureId> {
        let frame = self.ctx.cumulative_frame_nr();
        match self.lock().slots.get_mut(&id) {
            Some(Slot::Ready(t, used)) => {
                *used = frame;
                Some(t.id())
            }
            _ => None,
        }
    }

    fn work(&self) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .into();
        loop {
            let job = {
                let mut s = self.lock();
                loop {
                    if let Some(id) = s.queue.pop() {
                        break Ok(id);
                    }
                    // the copyright once the tiles are here
                    if let Some(area) = s.credit_wanted.take() {
                        break Err(area);
                    }
                    s = self.wake.wait(s).unwrap_or_else(|e| e.into_inner());
                }
            };
            let id = match job {
                Ok(id) => id,
                Err(area) => {
                    let c = self.copyright(&agent, area);
                    let mut s = self.lock();
                    // a failure keeps what was known, and is asked again for another area
                    let known = s.credit.as_ref().and_then(|c| c.1.clone());
                    s.credit = Some((area, c.or(known)));
                    drop(s);
                    self.ctx.request_repaint();
                    continue;
                }
            };
            let got = match id.src {
                Source::Osm => fetch(&agent, id, &self.dir),
                Source::Google => self.fetch_google(&agent, id),
            };
            let slot = match got {
                Some(img) => {
                    let name = format!("tile {}/{}/{}", id.z, id.x, id.y);
                    Slot::Ready(self.ctx.load_texture(name, img, egui::TextureOptions::LINEAR), self.ctx.cumulative_frame_nr())
                }
                None => Slot::Failed(Instant::now()),
            };
            let mut s = self.lock();
            s.slots.insert(id, slot);
            forget_old(&mut s.slots);
            drop(s);
            self.ctx.request_repaint();
        }
    }
}

impl Tiles {
    /// Google's tile session, made when there is none, it has run out,
    /// or the key has changed.
    fn google_session(&self, agent: &ureq::Agent) -> Option<(String, String)> {
        let key = google::key()?;
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp();
        if let Some(x) = s.as_ref().filter(|x| x.key == key && x.expiry > now + 60) {
            return Some((key, x.token.clone()));
        }
        let mut resp = agent
            .post(&format!("{GOOGLE_TILES}/v1/createSession?key={}", crate::sync::oauth::url_encode(&key)))
            .header("Content-Type", "application/json")
            .send(google::session_request().to_string())
            .ok()?;
        let ok = resp.status().is_success();
        let body = resp.body_mut().read_to_string().ok()?;
        if !ok {
            eprintln!("katlendar: no Google map session: {}", google::error_of(&body).unwrap_or(body));
            return None;
        }
        let (token, expiry) = google::parse_session(&body).ok()?;
        // with no expiry given, a day is assumed
        let expiry = if expiry > 0 { expiry } else { now + 24 * 3600 };
        *s = Some(Session { key: key.clone(), token: token.clone(), expiry });
        Some((key, token))
    }

    /// A tile from Google, kept in memory only.
    fn fetch_google(&self, agent: &ureq::Agent, id: TileId) -> Option<egui::ColorImage> {
        let (key, token) = self.google_session(agent)?;
        let url = format!("{GOOGLE_TILES}/v1/2dtiles/{}/{}/{}?session={}&key={}", id.z, id.x, id.y, token, crate::sync::oauth::url_encode(&key));
        let mut resp = agent.get(&url).call().ok()?;
        match resp.status().as_u16() {
            200 => decode(&resp.body_mut().with_config().limit(4 << 20).read_to_vec().ok()?),
            // the session is no longer good: the next tile makes a new one
            401 | 403 => {
                *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
                None
            }
            _ => None,
        }
    }

    /// The copyright of Google's map data in `area`.
    fn copyright(&self, agent: &ureq::Agent, (z, [w, n, e, s]): Area) -> Option<String> {
        let (key, token) = self.google_session(agent)?;
        let edge = |x: u32, y: u32| super::widget::spot(x as f64 * TILE, y as f64 * TILE, z as f64);
        let (north, west) = edge(w, n);
        let (south, east) = edge(e + 1, s + 1);
        let url = format!(
            "{GOOGLE_TILES}/tile/v1/viewport?session={token}&key={}&zoom={z}&north={north:.6}&south={south:.6}&east={east:.6}&west={west:.6}",
            crate::sync::oauth::url_encode(&key)
        );
        let mut resp = agent.get(&url).call().ok()?;
        let ok = resp.status().is_success();
        let body = resp.body_mut().read_to_string().ok()?;
        ok.then(|| google::parse_copyright(&body)).flatten()
    }
}

/// Lets go of the textures looked at longest ago, past [`IN_MEMORY`].
fn forget_old(slots: &mut HashMap<TileId, Slot>) {
    let mut ready: Vec<(u64, TileId)> = slots
        .iter()
        .filter_map(|(id, s)| match s {
            Slot::Ready(_, used) => Some((*used, *id)),
            _ => None,
        })
        .collect();
    if ready.len() <= IN_MEMORY {
        return;
    }
    ready.sort_unstable_by_key(|r| r.0);
    for (_, id) in &ready[..ready.len() - IN_MEMORY] {
        slots.remove(id);
    }
}

/// A tile from the disk while it is fresh, else from the server: asked
/// for only if changed when there is a copy, which is kept and used when
/// the server can't be reached.
fn fetch(agent: &ureq::Agent, id: TileId, dir: &Path) -> Option<egui::ColorImage> {
    let path = id.path(dir);
    let meta_path = path.with_extension("meta");
    let now = chrono::Utc::now().timestamp();
    let (expires, etag) = std::fs::read_to_string(&meta_path).map(|m| read_meta(&m)).unwrap_or((0, None));
    let from_disk = || std::fs::read(&path).ok().and_then(|b| decode(&b));
    if expires > now {
        if let Some(img) = from_disk() {
            return Some(img);
        }
    }
    let have = path.is_file();
    let mut req = agent.get(id.url()).header("User-Agent", USER_AGENT);
    if let (true, Some(tag)) = (have, etag.as_deref()) {
        req = req.header("If-None-Match", tag);
    }
    let Ok(mut resp) = req.call() else { return from_disk() };
    let header = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let keep = expiry(header("cache-control").as_deref(), header("expires").as_deref(), now);
    let new_tag = header("etag");
    match resp.status().as_u16() {
        200 => {
            let bytes = resp.body_mut().with_config().limit(4 << 20).read_to_vec().ok()?;
            let img = decode(&bytes)?;
            save(&path, &bytes, &write_meta(keep, new_tag.as_deref()));
            Some(img)
        }
        304 if have => {
            let _ = std::fs::write(&meta_path, write_meta(keep, new_tag.as_deref().or(etag.as_deref())));
            from_disk()
        }
        _ => from_disk(),
    }
}

/// Writes the tile beside a note of when it goes stale. The picture goes
/// through a temporary file, so a half-written one is never read.
fn save(path: &Path, png: &[u8], meta: &str) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, png).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        let _ = std::fs::write(path.with_extension("meta"), meta);
    }
}

fn decode(png: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// When a reply goes stale, in seconds since 1970: as Cache-Control's
/// max-age says, else as Expires says, else a week from `now`.
/// no-cache and no-store make it stale at once.
pub fn expiry(cache_control: Option<&str>, expires: Option<&str>, now: i64) -> i64 {
    if let Some(cc) = cache_control {
        for part in cc.split(',').map(|p| p.trim().to_ascii_lowercase()) {
            if part == "no-cache" || part == "no-store" {
                return now;
            }
            if let Some(secs) = part.strip_prefix("max-age=").and_then(|v| v.trim_matches('"').parse::<i64>().ok()) {
                return now + secs.max(0);
            }
        }
    }
    match expires {
        // a date that can't be read, such as "0", means stale already
        Some(e) => chrono::DateTime::parse_from_rfc2822(e.trim()).map(|d| d.timestamp()).unwrap_or(now),
        None => now + KEEP_SECS,
    }
}

/// The note kept beside a tile: when it goes stale, and its ETag.
fn write_meta(expires: i64, etag: Option<&str>) -> String {
    format!("{expires}\n{}\n", etag.unwrap_or(""))
}

fn read_meta(text: &str) -> (i64, Option<String>) {
    let mut lines = text.lines();
    let expires = lines.next().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    let etag = lines.next().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    (expires, etag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_follows_the_server() {
        let now = 1_000_000;
        assert_eq!(expiry(Some("public, max-age=3600"), None, now), now + 3600);
        assert_eq!(expiry(Some("max-age=60"), Some("Thu, 01 Jan 1970 00:00:00 GMT"), now), now + 60, "max-age wins over Expires");
        assert_eq!(expiry(Some("no-cache"), None, now), now);
        assert_eq!(expiry(None, Some("Sun, 06 Nov 1994 08:49:37 GMT"), now), 784111777);
        assert_eq!(expiry(None, Some("0"), now), now);
        assert_eq!(expiry(None, None, now), now + 7 * 24 * 3600, "a week when nothing is said");
        assert_eq!(expiry(Some("public"), None, now), now + 7 * 24 * 3600);
    }

    #[test]
    fn notes_read_back() {
        assert_eq!(read_meta(&write_meta(42, Some("\"abc\""))), (42, Some("\"abc\"".into())));
        assert_eq!(read_meta(&write_meta(42, None)), (42, None));
        assert_eq!(read_meta(""), (0, None));
    }

    #[test]
    fn tiles_have_a_place() {
        let t = TileId { src: Source::Osm, z: 3, x: 5, y: 2 };
        assert_eq!(t.url(), "https://tile.openstreetmap.org/3/5/2.png");
        assert_eq!(t.path(Path::new("/c")), Path::new("/c/osm/3/5/2.png"));
        assert_eq!(t.parent(), Some(TileId { src: Source::Osm, z: 2, x: 2, y: 1 }));
        assert_eq!(TileId { src: Source::Osm, z: 0, x: 0, y: 0 }.parent(), None);
        assert!(!TileId { src: Source::Osm, z: 3, x: 8, y: 0 }.valid());
        assert!(!TileId { src: Source::Osm, z: 20, x: 0, y: 0 }.valid());
        // Google draws closer
        assert!(TileId { src: Source::Google, z: 20, x: 0, y: 0 }.valid());
        assert!(!TileId { src: Source::Google, z: 22, x: 0, y: 0 }.valid());
        assert!(USER_AGENT.starts_with("katlendar/") && USER_AGENT.contains("github.com"));
    }
}
