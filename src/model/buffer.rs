//! Buffer time: minutes kept free before and after the events of chosen
//! calendars, for travel, traffic or a breather. Buffers are worked out from
//! the events whenever they are needed and never saved or synced.

use super::*;
use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// Buffer time for the events of one calendar.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug, Default)]
#[serde(default)]
pub struct CalBuffer {
    pub calendar: u64,
    pub enabled: bool,
    /// minutes kept free before each event, and after it
    pub before_min: i64,
    pub after_min: i64,
}

/// An event's own buffer time, in place of its calendar's. Both at
/// zero switches its buffers off.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
#[serde(default)]
pub struct EventBuffer {
    pub before_min: i64,
    pub after_min: i64,
}

/// The lengths offered for a buffer, in minutes.
pub const BUFFER_CHOICES: [i64; 11] = [0, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180];

/// Longest buffer on either side, in minutes.
pub const BUFFER_MAX_MIN: i64 = 240;

/// One buffer on the calendar: the stretch before or after an event.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BufferOcc {
    pub event: u64,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    /// before the event; else after it
    pub before: bool,
}

impl Prefs {
    /// The buffer set for a calendar, switched on and not empty.
    pub fn buffer_for(&self, cal: u64) -> Option<&CalBuffer> {
        self.buffers.iter().find(|b| b.calendar == cal && b.enabled && (b.before_min > 0 || b.after_min > 0))
    }
    /// The buffer setting of a calendar, made when first asked for.
    pub fn buffer_mut(&mut self, cal: u64) -> &mut CalBuffer {
        if let Some(i) = self.buffers.iter().position(|b| b.calendar == cal) {
            return &mut self.buffers[i];
        }
        self.buffers.push(CalBuffer { calendar: cal, enabled: false, before_min: 15, after_min: 15 });
        self.buffers.last_mut().unwrap()
    }
}

impl Data {
    /// Minutes before and after an event: its own, else its calendar's.
    pub fn buffer_of(&self, e: &Event) -> (i64, i64) {
        let (b, a) = match e.buffer {
            Some(o) => (o.before_min, o.after_min),
            None => self.prefs.buffer_for(e.calendar).map_or((0, 0), |c| (c.before_min, c.after_min)),
        };
        (b.clamp(0, BUFFER_MAX_MIN), a.clamp(0, BUFFER_MAX_MIN))
    }

    /// Buffers touching [a, b), by start: of timed, kept events on visible
    /// calendars, with their own buffers or their calendar's. Task sessions
    /// get none.
    pub fn buffers_in(&self, a: NaiveDateTime, b: NaiveDateTime) -> Vec<BufferOcc> {
        let mut v = vec![];
        for e in &self.events {
            if e.all_day || e.skipped || e.task_id.is_some() || !self.calendar(e.calendar).is_some_and(|c| c.visible) {
                continue;
            }
            let (before, after) = self.buffer_of(e);
            if before > 0 {
                v.push(BufferOcc { event: e.id, start: e.start - Duration::minutes(before), end: e.start, before: true });
            }
            if after > 0 {
                v.push(BufferOcc { event: e.id, start: e.end, end: e.end + Duration::minutes(after), before: false });
            }
        }
        v.retain(|o| o.start < b && o.end > a);
        v.sort_by_key(|o| (o.start, o.end));
        v
    }

    /// An event with its buffers around it, while it has any and they show.
    fn padded(&self, e: &Event) -> Option<(NaiveDateTime, NaiveDateTime)> {
        if e.all_day || e.skipped || e.task_id.is_some() || !self.calendar(e.calendar).is_some_and(|c| c.visible) {
            return None;
        }
        let (b, a) = self.buffer_of(e);
        (b > 0 || a > 0).then(|| (e.start - Duration::minutes(b), e.end + Duration::minutes(a)))
    }

    /// The spans, buffers and all, of the buffered events that event `id`
    /// may not run into: every one but those it already overlaps where it
    /// is now.
    fn buffer_walls(&self, id: u64) -> Option<(&Event, Vec<(NaiveDateTime, NaiveDateTime)>)> {
        let e = self.event(id)?;
        let here = self.padded(e)?;
        let walls = self.events.iter().filter(|o| o.id != id).filter_map(|o| self.padded(o)).filter(|w| !(w.0 < here.1 && w.1 > here.0)).collect();
        Some((e, walls))
    }

    /// Where event `id` stops when dragged to start at `start`: there while
    /// its buffers stay clear of other events' buffers, else pushed back out
    /// to the side it came from, its buffer just touching the other's.
    pub fn buffer_clear_start(&self, id: u64, start: NaiveDateTime) -> NaiveDateTime {
        let Some((e, walls)) = self.buffer_walls(id) else { return start };
        let (b, a) = self.buffer_of(e);
        let (len, down) = (e.end - e.start, start >= e.start);
        let (b, a) = (Duration::minutes(b), Duration::minutes(a));
        let mut s = start;
        // each push may run it into the next one along, so go until clear
        for _ in 0..=walls.len() {
            let Some(w) = walls.iter().find(|w| w.0 < s + len + a && w.1 > s - b) else { break };
            s = if down { w.0 - len - a } else { w.1 + b };
        }
        s
    }

    /// Where event `id`'s end stops when dragged to `end`: short of the
    /// next buffered event, so their buffers only touch.
    pub fn buffer_clear_end(&self, id: u64, end: NaiveDateTime) -> NaiveDateTime {
        let Some((e, walls)) = self.buffer_walls(id) else { return end };
        let (b, a) = self.buffer_of(e);
        let from = e.start - Duration::minutes(b);
        let a = Duration::minutes(a);
        walls.iter().filter(|w| w.0 >= from && w.0 < end + a).map(|w| w.0 - a).fold(end, |m, t| m.min(t))
    }

    /// Buffers touching one day.
    pub fn buffers_on(&self, day: NaiveDate) -> Vec<BufferOcc> {
        let a = day.and_hms_opt(0, 0, 0).unwrap();
        self.buffers_in(a, a + Duration::days(1))
    }

    /// Buffer time the planner may not use on a day: all of it, unless
    /// tasks are let into buffers.
    pub(super) fn buffer_cuts(&self, day: NaiveDate) -> Vec<(NaiveDateTime, NaiveDateTime)> {
        if !self.prefs.buffers_keep_out {
            return vec![];
        }
        self.buffers_on(day).into_iter().map(|o| (o.start, o.end)).collect()
    }

    /// The buffer that keeps a task out of a stretch of time, if one does.
    pub fn buffer_keeping_out(&self, start: NaiveDateTime, end: NaiveDateTime) -> Option<BufferOcc> {
        if !self.prefs.buffers_keep_out {
            return None;
        }
        self.buffers_in(start, end).into_iter().next()
    }
}
