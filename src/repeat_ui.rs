//! The repeat rows of the block window and the editors: a dropdown of the
//! usual patterns and, once Custom is picked, rows under it for every so
//! many days, weeks, months or years, which days, and how it ends.

use crate::dialogs::{date_popup, PickLook};
use crate::model::*;
use crate::strings::{tr, trf};
use chrono::{Datelike, NaiveDate};
use eframe::egui;

/// How a repeat ends, with the values of the choices not picked kept, so
/// going back to one finds it as it was.
#[derive(Clone, Copy, PartialEq)]
struct EndPick {
    mode: u8,
    until: NaiveDate,
    count: u16,
}

const NEVER: u8 = 0;
const ON: u8 = 1;
const AFTER: u8 = 2;

impl EndPick {
    fn new(ends: Ends, r: Repeat, day: NaiveDate) -> Self {
        let mut p = EndPick { mode: NEVER, until: r.first_until(day), count: 10 };
        match ends {
            Ends::Never => {}
            Ends::On(d) => (p.mode, p.until) = (ON, d),
            Ends::After(n) => (p.mode, p.count) = (AFTER, n),
        }
        p
    }
    fn ends(&self) -> Ends {
        match self.mode {
            ON => Ends::On(self.until),
            AFTER => Ends::After(self.count.max(1)),
            _ => Ends::Never,
        }
    }
}

/// The repeat rows of an editor's two-column grid, each name drawn by
/// `label`: Repeat, then Every and On while Custom is picked, then Ends
/// while it repeats. `forever` lets the repeat go on with no end (a
/// task's); else a repeat always has one. `tip` is told on Repeat's name
/// and `ends_tip` on Ends'. Gives back whether anything changed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn repeat_rows(
    ui: &mut egui::Ui,
    salt: &str,
    r: &mut Repeat,
    ends: &mut Ends,
    day: NaiveDate,
    look: PickLook,
    forever: bool,
    label: &dyn Fn(&mut egui::Ui, &str) -> egui::Response,
    tip: Option<&str>,
    ends_tip: Option<&str>,
) -> bool {
    let was = (*r, *ends);
    let monthly_nth = Repeat::Every(Rule { n: 1, unit: Unit::Month, days: 0, by_weekday: true });
    let presets = [Repeat::None, Repeat::Daily, Repeat::Weekdays, Repeat::Weekly, Repeat::Monthly, monthly_nth, Repeat::Yearly];
    // Custom stays picked while its rows are in use, even when what they
    // make is one of the usual patterns. It is kept with the repeat it was
    // picked for, so a repeat changed some other way lets it go.
    let custom_id = ui.scope_id().with((salt, "custom"));
    // test aid: a custom repeat set, once
    if crate::testaid::get().repeat.as_deref() == Some(salt) {
        let once = custom_id.with("aid");
        if !ui.data(|d| d.get_temp::<bool>(once).unwrap_or(false)) {
            ui.data_mut(|d| d.insert_temp(once, true));
            *r = Repeat::Every(Rule { n: 2, unit: Unit::Week, days: 0b101, by_weekday: false });
            *ends = Ends::After(10);
        }
    }
    let picked_for = ui.data(|d| d.get_temp::<Repeat>(custom_id));
    let mut custom = *r != Repeat::None && (picked_for == Some(*r) || !presets.contains(r));

    let l = label(ui, tr("Repeat"));
    if let Some(t) = tip {
        l.on_hover_text(t);
    }
    let width = ui.available_width().clamp(120.0, 250.0);
    let shown = if custom { tr("Custom").to_string() } else { r.label_on(Some(day)) };
    // tall enough for every choice, so none is hidden below a scroll
    egui::ComboBox::from_id_salt((salt, "repeat")).width(width).height(f32::INFINITY).selected_text(shown).show_ui(ui, |ui| {
        ui.set_min_width(220.0);
        for v in presets {
            if ui.selectable_label(!custom && *r == v, v.label_on(Some(day))).clicked() {
                *r = v;
                custom = false;
            }
        }
        ui.separator();
        if ui.selectable_label(custom, tr("Custom")).clicked() && !custom {
            let mut rule = r.rule().unwrap_or_default();
            if rule.unit == Unit::Week && rule.days == 0 {
                rule.days = start_bit(day);
            }
            *r = Repeat::from_rule(rule);
            custom = true;
        }
    });
    ui.end_row();

    if custom {
        let mut rule = r.rule().unwrap_or_default();
        if rule.unit == Unit::Week && rule.days == 0 {
            rule.days = start_bit(day);
        }
        let before = rule;
        label(ui, tr("Every"));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut rule.n).range(1..=99).speed(0.05));
            let n = rule.n;
            egui::ComboBox::from_id_salt((salt, "unit")).selected_text(rule.unit.name(n)).show_ui(ui, |ui| {
                for u in Unit::ALL {
                    ui.selectable_value(&mut rule.unit, u, u.name(n));
                }
            });
        });
        ui.end_row();
        match rule.unit {
            Unit::Week => {
                label(ui, tr("On"));
                if rule.days == 0 {
                    rule.days = start_bit(day);
                }
                day_chips(ui, &mut rule.days, look.first_day, true);
                ui.end_row();
            }
            Unit::Month => {
                label(ui, tr("On"));
                let on_day = trf("Day {}", &[&day.day()]);
                let on_nth = trf("The {}", &[&nth_weekday_name(day)]);
                let shown = if rule.by_weekday { on_nth.clone() } else { on_day.clone() };
                egui::ComboBox::from_id_salt((salt, "monthby")).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut rule.by_weekday, false, on_day);
                    ui.selectable_value(&mut rule.by_weekday, true, on_nth);
                });
                ui.end_row();
            }
            _ => {}
        }
        // a weekly repeat shown with its weekday stays as it was until
        // something here is changed
        if rule != before {
            *r = Repeat::from_rule(rule);
        }
        ui.data_mut(|d| d.insert_temp(custom_id, *r));
    } else {
        ui.data_mut(|d| d.remove::<Repeat>(custom_id));
    }

    // a repeat just picked starts with an end where it must have one
    if *r == Repeat::None {
        *ends = Ends::Never;
    } else {
        if !forever && *ends == Ends::Never {
            *ends = Ends::On(r.first_until(day));
        }
        let l = label(ui, tr("Ends"));
        if let Some(t) = ends_tip {
            l.on_hover_text(t);
        }
        ends_field(ui, salt, *r, ends, day, look, forever);
        ui.end_row();
    }
    (*r, *ends) != was
}

/// The bit of `day`'s weekday, Monday bit 0.
fn start_bit(day: NaiveDate) -> u8 {
    1 << day.weekday().num_days_from_monday()
}

/// How a repeat ends: never (where `forever` allows it), on a day, or after
/// so many times.
fn ends_field(ui: &mut egui::Ui, salt: &str, r: Repeat, ends: &mut Ends, day: NaiveDate, look: PickLook, forever: bool) {
    let salt_id = ui.scope_id().with((salt, "ends_pick"));
    // the values of the choices not picked are kept while the window is open
    let mut pick = ui.data(|d| d.get_temp::<EndPick>(salt_id)).filter(|p| p.ends() == *ends).unwrap_or_else(|| EndPick::new(*ends, r, day));
    ui.horizontal(|ui| {
        let name = |m: u8| match m {
            NEVER => tr("Never"),
            ON => tr("On"),
            _ => tr("After"),
        };
        let modes: &[u8] = if forever { &[NEVER, ON, AFTER] } else { &[ON, AFTER] };
        egui::ComboBox::from_id_salt((salt, "ends")).width(64.0).selected_text(name(pick.mode)).show_ui(ui, |ui| {
            for m in modes {
                ui.selectable_value(&mut pick.mode, *m, name(*m));
            }
        });
        match pick.mode {
            ON => {
                let b = ui.button(pick.until.format("%b %-d, %Y").to_string()).on_hover_text(tr("Pick the last day"));
                if let Some(d) = date_popup(ui, &b, &format!("{salt}_until"), Some(pick.until), look) {
                    pick.until = d.max(day);
                }
            }
            AFTER => {
                ui.add(egui::DragValue::new(&mut pick.count).range(1..=MAX_TIMES as u16).speed(0.1));
                ui.label(if pick.count == 1 { tr("time") } else { tr("times") });
            }
            _ => {}
        }
    });
    *ends = pick.ends();
    ui.data_mut(|d| d.insert_temp(salt_id, pick));
}

/// Mo to Su as toggles, starting on `first_day` (0 Sunday), over a mask
/// with bit 0 Monday to bit 6 Sunday. `keep_one` keeps the last one picked
/// from being taken off.
pub(crate) fn day_chips(ui: &mut egui::Ui, days: &mut u8, first_day: u8, keep_one: bool) {
    let names = [tr("Mo"), tr("Tu"), tr("We"), tr("Th"), tr("Fr"), tr("Sa"), tr("Su")];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for i in 0..7u8 {
            // the place in the week, as Monday-first bits
            let at = (first_day + i + 6) % 7;
            let bit = 1u8 << at;
            let on = *days & bit != 0;
            if ui.selectable_label(on, names[at as usize]).clicked() && !(keep_one && on && *days & 0x7f == bit) {
                *days ^= bit;
            }
        }
    });
}
