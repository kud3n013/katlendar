//! The dialog that finds a place: a search that answers as you type, and
//! a map whose pin can be put anywhere, which looks up the address there.
//!
//! Keys: the arrows go through the results and Enter takes one; Enter
//! again uses the place. Esc closes the results, then the dialog.

use eframe::egui::{self, vec2, Align, Key, Layout, Modifiers, RichText};

use super::osm::{self, Answer, Found};
use super::widget::{self, world, MapView};
use crate::model::Place;
use crate::{icons, theme, tr, trf};

/// How the dialog was closed.
pub enum Closed {
    /// with this place, or none when its words were cleared
    Use(Option<Place>),
    Cancel,
}

#[derive(Clone)]
struct Picker {
    /// what is typed in the search box
    query: String,
    /// the line the place is saved as
    text: String,
    pin: Option<(f64, f64)>,
    view: MapView,
    /// the results are shown under the search box
    listing: bool,
    results: Vec<Found>,
    /// the result the arrow keys are on
    sel: usize,
    /// the address at the pin, being looked up
    reverse: Option<String>,
    /// the words were there when the dialog opened, so they are looked up at once
    at_once: bool,
    focus: bool,
    /// the frame the dialog was last drawn in
    seen: u64,
}

/// Opens the dialog `id` on `place`: on its pin when it has one, else
/// looking up its words.
pub fn open(ctx: &egui::Context, id: egui::Id, place: &Option<Place>) {
    let text = place.as_ref().map(|p| p.text.clone()).unwrap_or_default();
    let pin = place.as_ref().and_then(|p| p.geo);
    // with no pin, the map is where the last one was left
    let last = ctx.data(|d| d.get_temp::<MapView>(last_view()));
    let view = match pin {
        Some(p) => MapView { center: p, zoom: 16.0 },
        None => last.unwrap_or(MapView { center: (30.0, 10.0), zoom: 2.0 }),
    };
    let listing = pin.is_none() && !text.trim().is_empty();
    let p = Picker { query: text.clone(), text, pin, view, listing, results: Vec::new(), sel: 0, reverse: None, at_once: true, focus: true, seen: 0 };
    ctx.data_mut(|d| d.insert_temp(id, p));
}

/// Test aid: opens the dialog searching `words` at once.
pub fn open_searching(ctx: &egui::Context, id: egui::Id, place: &Option<Place>, words: &str) {
    open(ctx, id, place);
    let Some(mut p) = ctx.data(|d| d.get_temp::<Picker>(id)) else { return };
    if !words.trim().is_empty() {
        (p.query, p.listing, p.at_once) = (words.to_string(), true, true);
    }
    ctx.data_mut(|d| d.insert_temp(id, p));
}

fn last_view() -> egui::Id {
    crate::ids::place_picker("last view")
}

/// The frame a picker was last drawn in, for each window.
fn shown(ctx: &egui::Context) -> egui::Id {
    crate::ids::place_picker(("shown", ctx.viewport_id()))
}

/// Whether a picker was drawn in this window last frame, so Esc is its own.
pub fn is_open(ctx: &egui::Context) -> bool {
    let mark = shown(ctx);
    ctx.data(|d| d.get_temp::<u64>(mark)).is_some_and(|f| f + 1 >= ctx.cumulative_frame_nr())
}

/// Draws the dialog `id` when it is open; says so when it closes.
pub fn show(ctx: &egui::Context, id: egui::Id) -> Option<Closed> {
    let mut p = ctx.data(|d| d.get_temp::<Picker>(id))?;
    let frame = ctx.cumulative_frame_nr();
    // the window it was opened from went away while it was open
    if p.seen != 0 && p.seen + 2 < frame {
        ctx.data_mut(|d| d.remove::<Picker>(id));
        return None;
    }
    p.seen = frame;
    let mark = shown(ctx);
    ctx.data_mut(|d| d.insert_temp(mark, frame));
    let lookups = osm::lookups(ctx);
    let (muted, warn) = (ctx.global_style().visuals.weak_text_color(), theme::DANGER);

    // the keys, before any field or window behind takes them
    let key = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    let (esc, enter, down, up) = (key(Key::Escape), key(Key::Enter), key(Key::ArrowDown), key(Key::ArrowUp));

    // the words typed, looked up once the typing pauses
    let mut searching = false;
    let mut note: Option<(String, bool)> = None;
    let q = p.query.trim().to_string();
    if p.listing && q.chars().count() >= 2 {
        let url = osm::search_url(&q, Some((p.view.center, p.view.zoom)));
        let wait = if p.at_once { std::time::Duration::ZERO } else { osm::PAUSE };
        match lookups.get(&url, wait) {
            Answer::Found(r) => {
                if r != p.results {
                    p.results = r;
                    p.sel = 0;
                }
                if p.results.is_empty() {
                    note = Some((trf("Nothing found for \"{}\"", &[&q]), false));
                }
            }
            Answer::Waiting => searching = true,
            Answer::Failed(e) => note = Some((trf("The search can't be reached: {}", &[&e]), true)),
        }
    }
    // the address where the pin was put
    if let Some(url) = p.reverse.clone() {
        match lookups.get(&url, std::time::Duration::ZERO) {
            Answer::Found(r) => {
                p.reverse = None;
                match r.first() {
                    Some(f) => p.text = f.text.clone(),
                    None => p.text.clear(),
                }
            }
            Answer::Waiting => {}
            Answer::Failed(e) => {
                p.reverse = None;
                note = Some((trf("The address can't be looked up: {}", &[&e]), true));
            }
        }
    }
    if p.pin.is_some() && p.reverse.is_none() && p.text.trim().is_empty() && note.is_none() {
        note = Some((tr("No address is known here; name the place yourself.").to_string(), false));
    }

    let screen = ctx.content_rect();
    let w = (screen.width() - 56.0).clamp(280.0, 620.0);
    let map_size = vec2(w, (screen.height() - 290.0).clamp(150.0, 340.0));
    let can_use = p.pin.is_some() && !p.text.trim().is_empty() && p.reverse.is_none();
    let words = if p.text.trim().is_empty() { p.query.trim().to_string() } else { p.text.trim().to_string() };
    let mut closed = None;

    // the keys, now that it is known what they act on
    let listed = p.listing && !p.results.is_empty();
    if down || up {
        if listed {
            let n = p.results.len();
            p.sel = if down { (p.sel + 1) % n } else { (p.sel + n - 1) % n };
        } else if !p.results.is_empty() {
            p.listing = true;
        }
    }
    let mut take: Option<Found> = None;
    if enter {
        if listed {
            take = p.results.get(p.sel).cloned();
        } else if can_use {
            closed = Some(Closed::Use(Place::of(&p.text, p.pin)));
        }
    }
    if esc {
        if p.listing {
            p.listing = false;
        } else {
            closed = Some(Closed::Cancel);
        }
    }

    let mut map_rect = egui::Rect::NOTHING;
    egui::Modal::new(id.with("modal")).show(ctx, |ui| {
        ui.set_width(w);
        ui.label(RichText::new(tr("Find a place")).size(theme::size::LARGE).strong());
        ui.add_space(4.0);
        let r = ui
            .horizontal(|ui| {
                ui.label(RichText::new(icons::SEARCH).color(muted));
                ui.add(egui::TextEdit::singleline(&mut p.query).id(id.with("search")).hint_text(tr("Search for a place or an address")).desired_width(f32::INFINITY))
            })
            .inner;
        if std::mem::take(&mut p.focus) {
            r.request_focus();
        }
        if r.changed() {
            p.listing = true;
            p.at_once = false;
        }
        ui.add_space(4.0);
        map_rect = egui::Rect::from_min_size(ui.cursor().min, map_size);
        let m = widget::pick(ui, &mut p.view, p.pin, map_size);
        if let Some(spot) = m.clicked.or(m.pin_moved) {
            p.pin = Some(spot);
            p.reverse = Some(osm::reverse_url(spot));
            p.listing = false;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Saved as")).color(muted));
            ui.add(egui::TextEdit::singleline(&mut p.text).hint_text(tr("The name and address everyone sees")).desired_width(f32::INFINITY));
        });
        let line = if p.reverse.is_some() {
            Some((tr("Looking up the address…").to_string(), false))
        } else if searching {
            Some((tr("Searching…").to_string(), false))
        } else if note.is_some() {
            note.clone()
        } else if p.pin.is_none() {
            Some((tr("Search, or click the map to put a pin down.").to_string(), false))
        } else {
            Some((tr("Drag the pin or click the map to move it.").to_string(), false))
        };
        if let Some((t, bad)) = line {
            ui.label(RichText::new(t).size(theme::size::SMALL).color(if bad { warn } else { muted }));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let use_it = ui.add_enabled(can_use, egui::Button::new(RichText::new(tr("Use this place")).strong())).on_hover_text(tr("Enter"));
            if use_it.clicked() {
                closed = Some(Closed::Use(Place::of(&p.text, p.pin)));
            }
            let words_only = ui.add_enabled(!words.is_empty(), egui::Button::new(tr("Text only"))).on_hover_text(tr("Keep the words, without a spot on the map"));
            if words_only.clicked() {
                closed = Some(Closed::Use(Place::of(&words, None)));
            }
            if ui.button(tr("Cancel")).on_hover_text(tr("Esc")).clicked() {
                closed = Some(Closed::Cancel);
            }
        });
    });

    // the results, over the top of the map
    if p.listing && !p.results.is_empty() && closed.is_none() {
        egui::Area::new(id.with("results")).order(egui::Order::Tooltip).fixed_pos(map_rect.min + vec2(6.0, 6.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(map_rect.width() - 12.0 - 14.0);
                ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                    for (i, f) in p.results.iter().enumerate() {
                        let mut job = egui::text::LayoutJob::default();
                        let body = egui::FontId::proportional(theme::size::BODY);
                        job.append(&f.name, 0.0, egui::TextFormat::simple(body, ui.visuals().strong_text_color()));
                        if !f.detail.is_empty() {
                            job.append(&format!("\n{}", f.detail), 0.0, egui::TextFormat::simple(egui::FontId::proportional(theme::size::SMALL), muted));
                        }
                        job.wrap.max_rows = 2;
                        job.wrap.break_anywhere = true;
                        let r = ui.add(egui::Button::selectable(i == p.sel, job));
                        if r.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            p.sel = i;
                        }
                        if r.clicked() {
                            take = Some(f.clone());
                        }
                    }
                });
            });
        });
    }
    if let Some(f) = take {
        p.view = fit(&f, map_size);
        p.pin = Some(f.geo);
        p.text = f.text.clone();
        p.query = f.name.clone();
        p.listing = false;
        p.reverse = None;
        p.focus = true;
    }

    if closed.is_some() {
        ctx.data_mut(|d| {
            d.remove::<Picker>(id);
            d.insert_temp(last_view(), p.view);
        });
    } else {
        ctx.data_mut(|d| d.insert_temp(id, p));
    }
    closed
}

/// The view that shows a result whole: a town or country fills the map,
/// a building is seen close up.
fn fit(f: &Found, size: egui::Vec2) -> MapView {
    let Some([s, w, n, e]) = f.extent else { return MapView { center: f.geo, zoom: 17.0 } };
    let (x0, y0) = world(n, w, 0.0);
    let (x1, y1) = world(s, e, 0.0);
    let (dx, dy) = ((x1 - x0).abs().max(1e-9), (y1 - y0).abs().max(1e-9));
    let zoom = (size.x as f64 / dx).min(size.y as f64 / dy).log2().clamp(3.0, 17.0);
    MapView { center: super::widget::spot((x0 + x1) / 2.0, (y0 + y1) / 2.0, 0.0), zoom }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    /// Runs a frame of the picker `id` with these events; what it closed with.
    fn frame(ctx: &egui::Context, id: egui::Id, events: Vec<egui::Event>) -> Option<Closed> {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 700.0))), events, ..Default::default() };
        let mut closed = None;
        let mut out = ctx.run_ui(input, |ui| closed = show(ui.ctx(), id));
        out.textures_delta.clear();
        closed
    }

    fn state(ctx: &egui::Context, id: egui::Id) -> Picker {
        ctx.data(|d| d.get_temp::<Picker>(id)).expect("the picker is open")
    }

    #[test]
    fn the_keys_go_through_the_results_and_use_one() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("test picker");
        open_searching(&ctx, id, &None, "corner cafe");
        assert!(frame(&ctx, id, vec![]).is_none());
        let place = |name: &str, geo| Found { name: name.into(), detail: "London".into(), text: format!("{name}, London"), geo, extent: None };
        let url = osm::search_url("corner cafe", Some(((30.0, 10.0), 2.0)));
        osm::lookups(&ctx).answer(&url, Answer::Found(vec![place("Corner Cafe", (51.5, -0.1)), place("Corner Cafe Two", (51.6, -0.2))]));
        frame(&ctx, id, vec![]);
        assert_eq!(state(&ctx, id).results.len(), 2);
        // down to the second, and Enter takes it
        assert!(frame(&ctx, id, vec![key(Key::ArrowDown)]).is_none());
        assert_eq!(state(&ctx, id).sel, 1);
        assert!(frame(&ctx, id, vec![key(Key::Enter)]).is_none());
        let p = state(&ctx, id);
        assert_eq!((p.pin, p.text.as_str(), p.listing), (Some((51.6, -0.2)), "Corner Cafe Two, London", false));
        assert_eq!(p.view, MapView { center: (51.6, -0.2), zoom: 17.0 });
        // Enter again uses it
        match frame(&ctx, id, vec![key(Key::Enter)]) {
            Some(Closed::Use(p)) => assert_eq!(p, Place::of("Corner Cafe Two, London", Some((51.6, -0.2)))),
            _ => panic!("Enter uses the place"),
        }
        assert!(ctx.data(|d| d.get_temp::<Picker>(id)).is_none());
    }

    #[test]
    fn esc_closes_the_results_then_the_dialog() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("esc picker");
        let place = Place::of("Home", None);
        open(&ctx, id, &place);
        frame(&ctx, id, vec![]);
        assert!(state(&ctx, id).listing, "words with no spot are looked up at once");
        assert!(frame(&ctx, id, vec![key(Key::Escape)]).is_none());
        assert!(!state(&ctx, id).listing);
        assert!(matches!(frame(&ctx, id, vec![key(Key::Escape)]), Some(Closed::Cancel)));
    }

    #[test]
    fn a_town_fills_the_map_and_a_house_is_close_up() {
        let house = Found { name: "1".into(), detail: String::new(), text: "1".into(), geo: (51.5, -0.12), extent: None };
        assert_eq!(fit(&house, vec2(400.0, 300.0)), MapView { center: (51.5, -0.12), zoom: 17.0 });
        // Paris, about 0.25 degrees wide
        let town = Found { extent: Some([48.81, 2.22, 48.90, 2.47]), ..house };
        let v = fit(&town, vec2(400.0, 300.0));
        assert!((10.0..12.0).contains(&v.zoom), "zoom {}", v.zoom);
        assert!((v.center.0 - 48.855).abs() < 0.01 && (v.center.1 - 2.345).abs() < 0.01, "{:?}", v.center);
    }
}
