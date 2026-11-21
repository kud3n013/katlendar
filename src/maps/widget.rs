//! A map that can be dragged and zoomed, with a pin on a place.

use std::f64::consts::PI;

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense};

use super::tiles::{tiles, TileId, ATTRIBUTION, COPYRIGHT, MAX_ZOOM, TILE};
use crate::icons;
use crate::tr;

/// The furthest north and south Web Mercator draws: the world is square.
const MAX_LAT: f64 = 85.051_128_78;
const MIN_ZOOM: f64 = 1.0;
/// Coarser tiles stand in for one not yet here, up to this many zooms out.
const STAND_IN: u8 = 6;
/// The ground tiles are drawn on, OpenStreetMap's own.
const LAND: Color32 = Color32::from_rgb(0xf2, 0xef, 0xe9);
const PIN: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

/// What part of the world a map shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapView {
    /// latitude and longitude of the middle, in degrees
    pub center: (f64, f64),
    /// 0 shows the whole world in one tile; each step doubles the size
    pub zoom: f64,
}

/// Where a spot is, in pixels from the world's top left corner, at `zoom`.
pub fn world(lat: f64, lon: f64, zoom: f64) -> (f64, f64) {
    let n = TILE * zoom.exp2();
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0 * n;
    (x, y)
}

/// The spot at a pixel of the world, the other way from [`world`].
/// Longitude is brought back between -180 and 180.
pub fn spot(x: f64, y: f64, zoom: f64) -> (f64, f64) {
    let n = TILE * zoom.exp2();
    let lon = x / n * 360.0 - 180.0;
    let lat = (PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    (lat.clamp(-MAX_LAT, MAX_LAT), (lon + 180.0).rem_euclid(360.0) - 180.0)
}

impl MapView {
    /// Moves what is shown by `dx`, `dy` points, as a drag does.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let (x, y) = world(self.center.0, self.center.1, self.zoom);
        self.center = spot(x - dx, y - dy, self.zoom);
    }
    /// Zooms by `dz` steps, keeping the spot `off` points from the middle
    /// where it is: under the pointer, for a scroll.
    pub fn zoom_by(&mut self, dz: f64, off: (f64, f64)) {
        let z = (self.zoom + dz).clamp(MIN_ZOOM, MAX_ZOOM as f64);
        let (x, y) = world(self.center.0, self.center.1, self.zoom);
        let f = (z - self.zoom).exp2();
        self.center = spot((x + off.0) * f - off.0, (y + off.1) * f - off.1, z);
        self.zoom = z;
    }
}

#[allow(dead_code)]
pub struct MapResponse {
    pub response: egui::Response,
    /// the spot clicked, if the map was
    pub clicked: Option<(f64, f64)>,
}

/// Draws the map in `size`, with `pin` on it. A drag moves it; scrolling,
/// a double click and the buttons zoom.
pub fn show(ui: &mut egui::Ui, view: &mut MapView, pin: Option<(f64, f64)>, size: egui::Vec2) -> MapResponse {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let mid = rect.center();
    let off = |p: Pos2| ((p.x - mid.x) as f64, (p.y - mid.y) as f64);
    if resp.dragged() {
        let d = resp.drag_delta();
        view.pan(d.x as f64, d.y as f64);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if let Some(p) = resp.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let dz = scroll as f64 / 120.0 + (pinch as f64).log2();
        if dz != 0.0 {
            view.zoom_by(dz, off(p));
            // the scroll zoomed the map, so the window around it stays put
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
        if resp.double_clicked() {
            view.zoom_by(1.0, off(p));
        }
    }
    let clicked = resp.clicked().then(|| resp.interact_pointer_pos()).flatten().map(|p| {
        let (x, y) = world(view.center.0, view.center.1, view.zoom);
        let (dx, dy) = off(p);
        spot(x + dx, y + dy, view.zoom)
    });

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, LAND);
    draw_tiles(ui.ctx(), &painter, rect, view);
    if let Some((lat, lon)) = pin {
        let (px, py) = world(lat, lon, view.zoom);
        let (cx, cy) = world(view.center.0, view.center.1, view.zoom);
        // the copy of the world nearest the middle, once it has wrapped
        let w = TILE * view.zoom.exp2();
        let dx = px - cx - ((px - cx) / w).round() * w;
        draw_pin(&painter, mid + vec2(dx as f32, (py - cy) as f32));
    }
    painter.rect_stroke(rect, 0.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);

    // the buttons, then the credit, on top of the map
    let side = 24.0;
    let mut at = pos2(rect.right() - 6.0 - side, rect.top() + 6.0);
    let mut button = |ui: &mut egui::Ui, icon: &str, tip: &str| {
        let r = Rect::from_min_size(at, vec2(side, side));
        at.y += side + 4.0;
        ui.put(r, egui::Button::new(icon)).on_hover_text(tip).clicked()
    };
    if button(ui, icons::PLUS, tr("Zoom in")) {
        view.zoom_by(view.zoom.floor() + 1.0 - view.zoom, (0.0, 0.0));
    }
    if button(ui, icons::MINUS, tr("Zoom out")) {
        view.zoom_by(view.zoom.ceil() - 1.0 - view.zoom, (0.0, 0.0));
    }
    if let Some(p) = pin.filter(|p| {
        let (a, b) = (world(p.0, p.1, view.zoom), world(view.center.0, view.center.1, view.zoom));
        (a.0 - b.0).hypot(a.1 - b.1) > 1.0
    }) {
        if button(ui, icons::LOCATE, tr("Back to the place")) {
            view.center = p;
        }
    }
    credit(ui, &painter, rect, resp.id);
    MapResponse { response: resp, clicked }
}

/// The tiles under the view. Each is asked for nearest the middle last,
/// so it comes first; while one is on its way a coarser one already here
/// is drawn enlarged in its place.
fn draw_tiles(ctx: &egui::Context, painter: &egui::Painter, rect: Rect, view: &MapView) {
    let tiles = tiles(ctx);
    let z = view.zoom.round().clamp(0.0, MAX_ZOOM as f64) as u8;
    let scale = (view.zoom - z as f64).exp2();
    let (cx, cy) = world(view.center.0, view.center.1, z as f64);
    let n = 1i64 << z;
    let half = (rect.width() as f64 / 2.0 / scale, rect.height() as f64 / 2.0 / scale);
    let range = |c: f64, h: f64| ((c - h) / TILE).floor() as i64..=((c + h) / TILE).floor() as i64;
    let mid = rect.center();
    let mut wanted = Vec::new();
    for ty in range(cy, half.1) {
        if !(0..n).contains(&ty) {
            continue;
        }
        for tx in range(cx, half.0) {
            let min = pos2(mid.x + ((tx as f64 * TILE - cx) * scale) as f32, mid.y + ((ty as f64 * TILE - cy) * scale) as f32);
            let r = Rect::from_min_size(min, egui::Vec2::splat((TILE * scale) as f32));
            let id = TileId { z, x: tx.rem_euclid(n) as u32, y: ty as u32 };
            wanted.push((r.center().distance_sq(mid), id, r));
        }
    }
    wanted.sort_by(|a, b| b.0.total_cmp(&a.0));
    let full = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    for (_, id, r) in wanted {
        if let Some(tex) = tiles.get(id) {
            painter.image(tex, r, full, Color32::WHITE);
            continue;
        }
        let mut up = id;
        for k in 1..=STAND_IN.min(z) {
            up = up.parent().unwrap_or(up);
            if let Some(tex) = tiles.peek(up) {
                let parts = (1u32 << k) as f32;
                let at = pos2((id.x % (1 << k)) as f32 / parts, (id.y % (1 << k)) as f32 / parts);
                painter.image(tex, r, Rect::from_min_size(at, egui::Vec2::splat(1.0 / parts)), Color32::WHITE);
                break;
            }
        }
    }
}

/// A pin with its point on `tip`.
fn draw_pin(painter: &egui::Painter, tip: Pos2) {
    let head = tip - vec2(0.0, 17.0);
    let r = 8.0;
    painter.circle_filled(tip, 2.5, Color32::from_black_alpha(70));
    let side = vec2(r * 0.86, r * 0.5);
    let outline = egui::Stroke::new(1.5, Color32::WHITE);
    painter.add(egui::Shape::convex_polygon(vec![tip, head + vec2(side.x, side.y), head - vec2(side.x, -side.y)], PIN, outline));
    painter.circle(head, r, PIN, outline);
    // the outline where the point meets the head is painted over
    painter.add(egui::Shape::convex_polygon(vec![tip - vec2(0.0, 2.5), head + vec2(side.x - 1.3, side.y), head - vec2(side.x - 1.3, -side.y)], PIN, egui::Stroke::NONE));
    painter.circle_filled(head, 3.0, Color32::WHITE);
}

/// "© OpenStreetMap contributors" in the bottom right corner, which
/// opens what that means.
fn credit(ui: &mut egui::Ui, painter: &egui::Painter, rect: Rect, id: egui::Id) {
    let ink = Color32::from_gray(50);
    let galley = painter.layout_no_wrap(ATTRIBUTION.into(), egui::FontId::proportional(10.0), ink);
    let pad = vec2(4.0, 1.0);
    let r = Rect::from_min_max(rect.right_bottom() - galley.size() - pad * 2.0, rect.right_bottom());
    painter.rect_filled(r, 0.0, Color32::from_white_alpha(200));
    painter.galley(r.min + pad, galley, ink);
    let link = ui.interact(r, id.with("credit"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(COPYRIGHT);
    if link.clicked() {
        crate::platform::open_url(COPYRIGHT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn spots_and_pixels_turn_into_each_other() {
        // the middle of the world, and its corners
        assert!(near(world(0.0, 0.0, 0.0), (128.0, 128.0)));
        assert!(near(world(MAX_LAT, -180.0, 1.0), (0.0, 0.0)));
        assert!(near(world(-MAX_LAT, 180.0, 1.0), (512.0, 512.0)));
        for (lat, lon) in [(52.52, 13.405), (-33.87, 151.21), (64.13, -21.9), (0.0, -179.99)] {
            for zoom in [0.0, 3.5, 16.0, 19.0] {
                let (x, y) = world(lat, lon, zoom);
                assert!(near(spot(x, y, zoom), (lat, lon)), "{lat},{lon} at {zoom}");
            }
        }
        // Berlin's tile at zoom 10, as the tile server numbers it
        let (x, y) = world(52.52, 13.405, 10.0);
        assert_eq!(((x / TILE) as u32, (y / TILE) as u32), (550, 335));
        // past the date line comes back round
        assert!(near(spot(world(0.0, 179.0, 2.0).0 + TILE * 4.0 * 2.0 / 360.0, 512.0, 2.0), (0.0, -179.0)));
    }

    /// Runs frames of a 300 by 160 map at the top left of a window, one
    /// per list of events, and gives back where it was left.
    fn drive(frames: Vec<Vec<egui::Event>>) -> MapView {
        let ctx = egui::Context::default();
        let mut view = MapView { center: (51.5, -0.12), zoom: 16.0 };
        for (i, events) in frames.into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                time: Some(i as f64 / 60.0),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                show(ui, &mut view, Some((51.5, -0.12)), vec2(300.0, 160.0));
            });
            // there is no painter to take the font textures
            out.textures_delta.clear();
        }
        view
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    #[test]
    fn a_drag_moves_the_map_with_the_pointer() {
        let p = pos2(100.0, 80.0);
        let mv = |x: f32, y: f32| egui::Event::PointerMoved(p + vec2(x, y));
        let v = drive(vec![vec![mv(0.0, 0.0)], vec![button(p, true)], vec![mv(40.0, 0.0)], vec![mv(80.0, 30.0)], vec![button(p + vec2(80.0, 30.0), false)], vec![]]);
        let (x0, y0) = world(51.5, -0.12, 16.0);
        let (x1, y1) = world(v.center.0, v.center.1, 16.0);
        assert_eq!(v.zoom, 16.0);
        assert!(((x0 - x1) - 80.0).abs() < 0.01 && ((y0 - y1) - 30.0).abs() < 0.01, "moved {} {}", x0 - x1, y0 - y1);
    }

    #[test]
    fn the_wheel_and_buttons_zoom() {
        let p = pos2(60.0, 50.0);
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 2.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() };
        let mut frames = vec![vec![egui::Event::PointerMoved(p)], vec![wheel]];
        frames.extend((0..60).map(|_| vec![]));
        let v = drive(frames);
        assert!(v.zoom > 16.5, "the wheel up zooms in, to {}", v.zoom);
        // the + button is in the top right corner of the map
        let plus = pos2(300.0 - 18.0, 18.0);
        let v = drive(vec![vec![egui::Event::PointerMoved(plus)], vec![button(plus, true)], vec![button(plus, false)], vec![]]);
        assert_eq!(v.zoom, 17.0);
        assert!(near(v.center, (51.5, -0.12)), "the + keeps the middle");
    }

    #[test]
    fn zooming_keeps_the_spot_under_the_pointer() {
        let mut v = MapView { center: (48.2, 16.37), zoom: 12.0 };
        let off = (100.0, -40.0);
        let under = |v: &MapView| {
            let (x, y) = world(v.center.0, v.center.1, v.zoom);
            spot(x + off.0, y + off.1, v.zoom)
        };
        let before = under(&v);
        v.zoom_by(1.5, off);
        assert_eq!(v.zoom, 13.5);
        let after = under(&v);
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        v.zoom_by(40.0, (0.0, 0.0));
        assert_eq!(v.zoom, MAX_ZOOM as f64, "no closer than the server draws");
        let mut w = MapView { center: (0.0, 0.0), zoom: 2.0 };
        w.pan(-TILE, 0.0);
        assert!(near(w.center, (0.0, 90.0)));
    }
}
