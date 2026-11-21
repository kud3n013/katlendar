//! How a task's due date or an event comes again: the patterns, the
//! custom "every N days, weeks, months or years" rule, and how it ends.

use crate::strings::{tr, trf};
use chrono::{Datelike, Duration, Months, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

/// The most times a repeat is laid out or counted.
pub const MAX_TIMES: usize = 400;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug, Default)]
pub enum Repeat {
    #[default]
    None,
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Yearly,
    /// on the picked weekdays: bit 0 Monday to bit 6 Sunday, as a frame's;
    /// kept for what was saved before `Every`
    Custom(u8),
    /// the custom repeat: every so many days, weeks, months or years
    Every(Rule),
}

/// Every `n` units. Weeks may name their weekdays; months may keep the
/// weekday of the month (the third Friday) instead of the date.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct Rule {
    pub n: u16,
    pub unit: Unit,
    /// for weeks: bit 0 Monday to bit 6 Sunday; none means the first day's
    #[serde(default)]
    pub days: u8,
    /// for months: the same weekday of the month, not the same date
    #[serde(default)]
    pub by_weekday: bool,
}

impl Default for Rule {
    fn default() -> Self {
        Rule { n: 1, unit: Unit::Week, days: 0, by_weekday: false }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum Unit {
    Day,
    #[default]
    Week,
    Month,
    Year,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::Day, Unit::Week, Unit::Month, Unit::Year];
    /// Its name after a count of `n`: "day" or "days".
    pub fn name(&self, n: u16) -> &'static str {
        match (self, n == 1) {
            (Unit::Day, true) => tr("day"),
            (Unit::Day, false) => tr("days"),
            (Unit::Week, true) => tr("week"),
            (Unit::Week, false) => tr("weeks"),
            (Unit::Month, true) => tr("month"),
            (Unit::Month, false) => tr("months"),
            (Unit::Year, true) => tr("year"),
            (Unit::Year, false) => tr("years"),
        }
    }
}

/// When a repeat stops.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug, Default)]
pub enum Ends {
    #[default]
    Never,
    /// on this day, the last that may have one
    On(NaiveDate),
    /// after this many times, the first among them
    After(u16),
}

impl Ends {
    pub fn is_never(&self) -> bool {
        *self == Ends::Never
    }
    /// How the end reads after a repeat's name: "until Nov 20, 2026".
    pub fn label(&self) -> String {
        match self {
            Ends::Never => String::new(),
            Ends::On(d) => trf("until {}", &[&d.format("%b %-d, %Y")]),
            Ends::After(1) => tr("once").into(),
            Ends::After(n) => trf("{} times", &[n]),
        }
    }
    /// What is left of it once the first time has gone by and the next is
    /// on `next`: None when there is no next time.
    pub fn after_one(&self, next: NaiveDate) -> Option<Ends> {
        match *self {
            Ends::Never => Some(Ends::Never),
            Ends::On(d) => (next <= d).then_some(*self),
            Ends::After(n) => (n > 1).then(|| Ends::After(n - 1)),
        }
    }
}

const DAY_NAMES: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
const DAY_LONG: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const ORDINALS: [&str; 5] = ["first", "second", "third", "fourth", "last"];

/// Which of its weekday in the month `d` is: 0 the first to 3 the fourth,
/// 4 the last when it is the fifth.
pub fn week_of_month(d: NaiveDate) -> usize {
    ((d.day0() / 7) as usize).min(4)
}

/// "the third Thursday" of the month `d` is in.
pub fn nth_weekday_name(d: NaiveDate) -> String {
    let wd = d.weekday().num_days_from_monday() as usize;
    trf("the {} {}", &[&tr(ORDINALS[week_of_month(d)]), &tr(DAY_LONG[wd])])
}

/// The `nth` (0 first … 4 last) `wd` of the month that starts on `first`.
fn nth_weekday_in(first: NaiveDate, wd: Weekday, nth: usize) -> NaiveDate {
    let to = (wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64).rem_euclid(7);
    let d = first + Duration::days(to + 7 * nth.min(3) as i64);
    if nth < 4 {
        return d;
    }
    // the last: the fifth when the month has one, else the fourth
    let fifth = d + Duration::days(7);
    if fifth.month() == first.month() { fifth } else { d }
}

fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

fn day_list(days: u8) -> String {
    (0..7).filter(|i| days & (1 << i) != 0).map(|i| tr(DAY_NAMES[i])).collect::<Vec<_>>().join(", ")
}

impl Rule {
    /// The weekdays it comes on: the picked ones, else the first day's.
    fn mask(&self, anchor: NaiveDate) -> u8 {
        match self.days & 0x7f {
            0 => 1 << anchor.weekday().num_days_from_monday(),
            m => m,
        }
    }
    /// The `i`th time of a month or year rule, from `anchor`.
    fn month_time(&self, anchor: NaiveDate, i: u32) -> Option<NaiveDate> {
        let step = if self.unit == Unit::Year { 12 } else { 1 } * self.n.max(1) as u32;
        let months = Months::new(i.checked_mul(step)?);
        if self.by_weekday && self.unit == Unit::Month {
            let first = anchor.with_day(1)?.checked_add_months(months)?;
            Some(nth_weekday_in(first, anchor.weekday(), week_of_month(anchor)))
        } else {
            // the same date, or the month's last day where it has none
            anchor.checked_add_months(months)
        }
    }
    /// The first time after `prev` of the rule started on `anchor`.
    fn after(&self, anchor: NaiveDate, prev: NaiveDate) -> Option<NaiveDate> {
        let n = self.n.max(1) as i64;
        match self.unit {
            Unit::Day => Some(prev + Duration::days(n)),
            Unit::Week => {
                let mask = self.mask(anchor);
                let week0 = monday_of(anchor);
                let mut d = prev + Duration::days(1);
                for _ in 0..(7 * n + 7) {
                    let on = mask & (1 << d.weekday().num_days_from_monday()) != 0;
                    if on && ((monday_of(d) - week0).num_weeks()).rem_euclid(n) == 0 {
                        return Some(d);
                    }
                    d += Duration::days(1);
                }
                None
            }
            Unit::Month | Unit::Year => {
                let step = if self.unit == Unit::Year { 12 } else { 1 } * n;
                let gone = (prev.year() as i64 * 12 + prev.month0() as i64) - (anchor.year() as i64 * 12 + anchor.month0() as i64);
                let from = (gone / step).max(1) as u32;
                (from..from + 3).filter_map(|i| self.month_time(anchor, i)).find(|d| *d > prev)
            }
        }
    }
    /// How it reads; `day` is the first day, for the date of a month or year.
    pub fn label(&self, day: Option<NaiveDate>) -> String {
        let n = self.n.max(1);
        let every = if n == 1 { trf("Every {}", &[&self.unit.name(1)]) } else { trf("Every {} {}", &[&n, &self.unit.name(n)]) };
        match self.unit {
            Unit::Day => every,
            Unit::Week => match (self.days & 0x7f, n) {
                (0, _) => every,
                (0x1f, 1) => tr("Every weekday").into(),
                (m, 1) => trf("Every {}", &[&day_list(m)]),
                (m, _) => trf("{} on {}", &[&every, &day_list(m)]),
            },
            Unit::Month => match day {
                Some(d) if self.by_weekday => trf("{} on {}", &[&every, &nth_weekday_name(d)]),
                Some(d) => trf("{} on day {}", &[&every, &d.day()]),
                None => every,
            },
            Unit::Year => match day {
                Some(d) => trf("{} on {}", &[&every, &d.format("%b %-d")]),
                None => every,
            },
        }
    }
}

impl Repeat {
    pub const ALL: [Repeat; 6] = [Repeat::None, Repeat::Daily, Repeat::Weekdays, Repeat::Weekly, Repeat::Monthly, Repeat::Yearly];
    pub fn label(&self) -> String {
        self.label_on(None)
    }
    /// How it reads for something on `day`: "Every month on day 20".
    pub fn label_on(&self, day: Option<NaiveDate>) -> String {
        match self {
            Repeat::None => tr("No repeat").into(),
            Repeat::Daily => tr("Every day").into(),
            Repeat::Weekdays => tr("Every weekday").into(),
            Repeat::Weekly => match day {
                Some(d) => trf("Every week on {}", &[&tr(DAY_LONG[d.weekday().num_days_from_monday() as usize])]),
                None => tr("Every week").into(),
            },
            Repeat::Monthly => match day {
                Some(d) => trf("Every month on day {}", &[&d.day()]),
                None => tr("Every month").into(),
            },
            Repeat::Yearly => match day {
                Some(d) => trf("Every year on {}", &[&d.format("%b %-d")]),
                None => tr("Every year").into(),
            },
            Repeat::Custom(days) if days & 0x7f == 0 => tr("Custom").into(),
            Repeat::Custom(_) | Repeat::Every(_) => self.rule().map(|r| r.label(day)).unwrap_or_default(),
        }
    }
    /// The same repeat as a custom rule, to edit; None for no repeat.
    pub fn rule(&self) -> Option<Rule> {
        let r = |n, unit, days| Some(Rule { n, unit, days, by_weekday: false });
        match *self {
            Repeat::None => None,
            Repeat::Daily => r(1, Unit::Day, 0),
            Repeat::Weekdays => r(1, Unit::Week, 0x1f),
            Repeat::Weekly => r(1, Unit::Week, 0),
            Repeat::Monthly => r(1, Unit::Month, 0),
            Repeat::Yearly => r(1, Unit::Year, 0),
            Repeat::Custom(days) => r(1, Unit::Week, days & 0x7f),
            Repeat::Every(rule) => Some(rule),
        }
    }
    /// A custom rule as the plain pattern it matches, if there is one.
    pub fn from_rule(r: Rule) -> Repeat {
        let r = Rule { n: r.n.max(1), days: if r.unit == Unit::Week { r.days & 0x7f } else { 0 }, by_weekday: r.by_weekday && r.unit == Unit::Month, ..r };
        match (r.n, r.unit, r.days, r.by_weekday) {
            (1, Unit::Day, _, _) => Repeat::Daily,
            (1, Unit::Week, 0, _) => Repeat::Weekly,
            (1, Unit::Week, 0x1f, _) => Repeat::Weekdays,
            (1, Unit::Month, _, false) => Repeat::Monthly,
            (1, Unit::Year, _, _) => Repeat::Yearly,
            _ => Repeat::Every(r),
        }
    }
    /// A custom repeat on the weekday of `day` alone, to start picking from.
    pub fn custom_from(day: NaiveDate) -> Repeat {
        Repeat::Custom(1 << day.weekday().num_days_from_monday())
    }
    /// The first time after `prev` of a repeat that started on `anchor`.
    pub fn after(&self, anchor: NaiveDate, prev: NaiveDate) -> Option<NaiveDate> {
        match self {
            Repeat::None => None,
            Repeat::Weekdays => {
                let mut n = prev + Duration::days(1);
                while matches!(n.weekday(), Weekday::Sat | Weekday::Sun) {
                    n += Duration::days(1);
                }
                Some(n)
            }
            r => r.rule()?.after(anchor, prev),
        }
    }
    /// The next date after `d` in this pattern.
    pub fn next(&self, d: NaiveDate) -> NaiveDate {
        self.after(d, d).unwrap_or(d)
    }
    /// A first last day for a repeating event that starts on `day`: four
    /// weeks on, twelve weeks for a weekly one, a year for a monthly one,
    /// five for a yearly one; longer as the rule skips more.
    pub fn first_until(&self, day: NaiveDate) -> NaiveDate {
        let Some(r) = self.rule() else { return day };
        let n = r.n.max(1) as u32;
        let later = match (self, r.unit) {
            (Repeat::Daily | Repeat::Weekdays, _) | (_, Unit::Day) => day.checked_add_signed(Duration::weeks(4 * n as i64)),
            (_, Unit::Week) => day.checked_add_signed(Duration::weeks(12 * n as i64)),
            (_, Unit::Month) => day.checked_add_months(Months::new(12 * n)),
            (_, Unit::Year) => day.checked_add_months(Months::new(60 * n)),
        };
        later.unwrap_or(day)
    }
    /// The days after `day`, up to and with `until`, on which an event
    /// that starts on `day` comes again. A monthly one keeps its day of the
    /// month where the month has it.
    pub fn days_after(&self, day: NaiveDate, until: NaiveDate) -> Vec<NaiveDate> {
        self.times_after(day, Ends::On(until))
    }
    /// The days after `day` on which something that starts on `day` comes
    /// again, as far as `ends` lets it; at most [`MAX_TIMES`].
    pub fn times_after(&self, day: NaiveDate, ends: Ends) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let most = match ends {
            Ends::After(n) => (n as usize).saturating_sub(1).min(MAX_TIMES),
            _ => MAX_TIMES,
        };
        let mut prev = day;
        while out.len() < most {
            let Some(d) = self.after(day, prev) else { break };
            if matches!(ends, Ends::On(u) if d > u) {
                break;
            }
            out.push(d);
            prev = d;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn every(n: u16, unit: Unit, days: u8, by_weekday: bool) -> Repeat {
        Repeat::Every(Rule { n, unit, days, by_weekday })
    }

    #[test]
    fn every_other_week_comes_on_the_picked_days_of_every_second_week() {
        // Monday Nov 2, 2026; Monday and Wednesday every 2 weeks
        let r = every(2, Unit::Week, 0b101, false);
        assert_eq!(r.times_after(d(2026, 11, 2), Ends::After(5)), vec![d(2026, 11, 4), d(2026, 11, 16), d(2026, 11, 18), d(2026, 11, 30)]);
        assert_eq!(r.label(), "Every 2 weeks on Mo, We");
        // a task done on the Wednesday is due next on the Monday after the skipped week
        assert_eq!(r.next(d(2026, 11, 4)), d(2026, 11, 16));
    }

    #[test]
    fn a_monthly_repeat_by_weekday_keeps_the_nth_weekday() {
        let third_thu = every(1, Unit::Month, 0, true);
        assert_eq!(third_thu.times_after(d(2026, 11, 19), Ends::After(4)), vec![d(2026, 12, 17), d(2027, 1, 21), d(2027, 2, 18)]);
        assert_eq!(third_thu.label_on(Some(d(2026, 11, 19))), "Every month on the third Thursday");
        // Nov 30 is the fifth Monday: it goes on as the last Monday
        let last_mon = third_thu.times_after(d(2026, 11, 30), Ends::On(d(2027, 2, 28)));
        assert_eq!(last_mon, vec![d(2026, 12, 28), d(2027, 1, 25), d(2027, 2, 22)]);
    }

    #[test]
    fn a_yearly_repeat_on_leap_day_falls_back_and_returns() {
        let got = Repeat::Yearly.times_after(d(2028, 2, 29), Ends::After(5));
        assert_eq!(got, vec![d(2029, 2, 28), d(2030, 2, 28), d(2031, 2, 28), d(2032, 2, 29)]);
        assert_eq!(Repeat::Yearly.label_on(Some(d(2026, 11, 20))), "Every year on Nov 20");
    }

    #[test]
    fn every_few_days_stops_on_the_last_day() {
        let r = every(3, Unit::Day, 0, false);
        assert_eq!(r.times_after(d(2026, 11, 20), Ends::On(d(2026, 11, 29))), vec![d(2026, 11, 23), d(2026, 11, 26), d(2026, 11, 29)]);
        assert_eq!(r.label(), "Every 3 days");
    }

    #[test]
    fn a_never_ending_repeat_is_capped() {
        assert_eq!(Repeat::Daily.times_after(d(2026, 11, 20), Ends::Never).len(), MAX_TIMES);
        assert!(Repeat::Daily.times_after(d(2026, 11, 20), Ends::After(1)).is_empty());
    }

    #[test]
    fn the_end_counts_down_as_times_go_by() {
        assert_eq!(Ends::After(3).after_one(d(2026, 11, 21)), Some(Ends::After(2)));
        assert_eq!(Ends::After(1).after_one(d(2026, 11, 21)), None);
        assert_eq!(Ends::On(d(2026, 11, 30)).after_one(d(2026, 12, 1)), None);
        assert_eq!(Ends::Never.after_one(d(2026, 12, 1)), Some(Ends::Never));
        assert_eq!((Ends::After(1).label(), Ends::After(4).label()), ("once".to_string(), "4 times".to_string()));
        assert_eq!(Ends::On(d(2026, 11, 20)).label(), "until Nov 20, 2026");
    }

    #[test]
    fn a_rule_that_matches_a_plain_pattern_becomes_it() {
        let rule = |n, unit, days, by_weekday| Rule { n, unit, days, by_weekday };
        assert_eq!(Repeat::from_rule(rule(1, Unit::Week, 0, false)), Repeat::Weekly);
        assert_eq!(Repeat::from_rule(rule(1, Unit::Week, 0x1f, false)), Repeat::Weekdays);
        assert_eq!(Repeat::from_rule(rule(1, Unit::Month, 0, false)), Repeat::Monthly);
        // days picked for a unit that has none are dropped
        assert_eq!(Repeat::from_rule(rule(1, Unit::Day, 0b11, true)), Repeat::Daily);
        assert_eq!(Repeat::from_rule(rule(2, Unit::Day, 0, false)), every(2, Unit::Day, 0, false));
        for r in Repeat::ALL.into_iter().skip(1) {
            assert_eq!(Repeat::from_rule(r.rule().unwrap()), r);
        }
    }

    #[test]
    fn old_and_new_repeats_read_back_from_saved_data() {
        for r in [Repeat::Weekly, Repeat::Custom(0b101), every(2, Unit::Month, 0, true)] {
            let text = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Repeat>(&text).unwrap(), r);
        }
        // a rule saved without the newer fields
        let r: Repeat = serde_json::from_str(r#"{"Every":{"n":3,"unit":"Day"}}"#).unwrap();
        assert_eq!(r, every(3, Unit::Day, 0, false));
    }
}
