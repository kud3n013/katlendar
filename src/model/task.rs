//! Tasks and the lists they are kept in.

use super::*;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub notes: String,
    /// where it is done; its blocks show it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<Place>,
    pub due: Option<NaiveDate>,
    /// 0 none, 1-3 high, 4-5 medium, 6-9 low
    pub priority: u8,
    pub duration_min: Option<i64>,
    pub done: bool,
    /// the planner may move its sessions
    #[serde(default)]
    pub flexible: bool,
    /// task list it belongs to; 0 is the Inbox
    #[serde(default)]
    pub list: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    /// not to be planned before this day
    #[serde(default)]
    pub earliest: Option<NaiveDate>,
    /// how its due date repeats once it is completed
    #[serde(default)]
    pub repeat: Repeat,
    /// when the repeat stops; each completion counts one time
    #[serde(default, skip_serializing_if = "Ends::is_never")]
    pub repeat_end: Ends,
    /// the same server fields as on events, for synced task lists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
    /// its blocks' colour here, over its list's; never sent to a server
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// Put away from the Completed list; still kept and synced.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// Its place among the deadlines of a day, once they were put in order
    /// by hand; it counts only while that day is still its due date. Never
    /// sent to a server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_order: Option<(NaiveDate, u32)>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct TaskList {
    pub id: u64,
    pub name: String,
    pub color: [u8; 3],
    /// a Lucide glyph shown in place of the ring; empty for the ring, or
    /// for a synced list its service's logo
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// the synced account it belongs to; None for one's own lists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<u64>,
    /// its address on the server
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctag: Option<String>,
    /// the server does not take changes to it
    #[serde(default)]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub const INBOX_COLOR: [u8; 3] = [0x35, 0x85, 0xe1];

impl Data {
    /// A list's name, colour and icon as kept; list 0 is the Inbox, whose
    /// name is empty until it is renamed. A list with no icon picked shows
    /// its own default.
    pub fn list_look(&self, id: u64) -> (String, [u8; 3], String) {
        match self.lists.iter().find(|l| l.id == id) {
            Some(l) if l.icon.is_empty() => (l.name.clone(), l.color, self.list_icon(id).to_string()),
            Some(l) => (l.name.clone(), l.color, l.icon.clone()),
            None => (self.prefs.inbox_name.clone(), self.prefs.inbox_color.unwrap_or(INBOX_COLOR), self.prefs.inbox_icon.clone()),
        }
    }
    /// A list's name as it is shown: a synced list's starts with its
    /// service, as in "Todoist - Inbox".
    pub fn list_title(&self, id: u64) -> String {
        let Some(l) = self.lists.iter().find(|l| l.id == id) else { return self.prefs.inbox_name() };
        let name = if l.name.trim().is_empty() { crate::strings::tr("Untitled list").to_string() } else { l.name.clone() };
        match l.account.and_then(|a| self.account(a)) {
            Some(a) => format!("{} - {name}", a.service()),
            None => name,
        }
    }
    /// The icon a list shows when none was picked: its service's logo for
    /// a synced list, else none, which is the ring.
    pub fn list_icon(&self, id: u64) -> &'static str {
        let account = self.lists.iter().find(|l| l.id == id).and_then(|l| l.account);
        account.and_then(|a| self.account(a)).map(|a| a.icon()).unwrap_or("")
    }
    pub fn set_list_look(&mut self, id: u64, look: (String, [u8; 3], String)) {
        let default = self.list_icon(id);
        if let Some(l) = self.lists.iter_mut().find(|l| l.id == id) {
            (l.name, l.color, l.icon) = look;
            // the default is kept as none, so it follows the account
            if l.icon == default {
                l.icon.clear();
            }
        } else if id == 0 {
            self.prefs.inbox_name = look.0;
            self.prefs.inbox_color = Some(look.1).filter(|c| *c != INBOX_COLOR);
            self.prefs.inbox_icon = look.2;
        }
    }
    /// The open tasks due on `day`, as the calendar lists them: those put in
    /// order by hand keep that order, and the rest go by priority, each
    /// before the first one of a lower priority.
    pub fn deadlines_on(&self, day: NaiveDate) -> Vec<u64> {
        let mut open: Vec<&Task> = self.tasks.iter().filter(|t| !t.done && t.due == Some(day)).collect();
        open.sort_by_key(|t| (t.rank(), t.title.to_lowercase(), t.id));
        let (mut placed, rest): (Vec<&Task>, Vec<&Task>) = open.into_iter().partition(|t| t.hand_place(day).is_some());
        placed.sort_by_key(|t| t.hand_place(day));
        for t in rest {
            let at = placed.iter().position(|o| o.rank() > t.rank()).unwrap_or(placed.len());
            placed.insert(at, t);
        }
        placed.into_iter().map(|t| t.id).collect()
    }
    /// Puts the deadlines of `day` in this order by hand.
    pub fn order_deadlines(&mut self, day: NaiveDate, ids: &[u64]) {
        for (i, id) in ids.iter().enumerate() {
            if let Some(t) = self.task_mut(*id) {
                t.due_order = Some((day, i as u32));
            }
        }
    }
    /// Removes one of one's own lists; its tasks go to the Inbox.
    pub fn remove_own_list(&mut self, id: u64) {
        self.lists.retain(|l| l.id != id);
        for t in self.tasks.iter_mut().filter(|t| t.list == id) {
            t.list = 0;
        }
        for f in self.frames.iter_mut() {
            f.filter.lists.retain(|l| *l != id);
        }
    }
}

impl TaskList {
    /// A synced list whose server takes changes.
    pub fn is_synced_writable(&self) -> bool {
        self.account.is_some() && !self.read_only
    }
}

impl Task {
    /// Its priority as a sort key: highest first, none last.
    pub fn rank(&self) -> u8 {
        if self.priority == 0 { 10 } else { self.priority }
    }
    /// Where it was put by hand among the deadlines of `day`.
    fn hand_place(&self, day: NaiveDate) -> Option<u32> {
        self.due_order.filter(|(d, _)| *d == day).map(|(_, i)| i)
    }
    /// Forgets where the task lives on a server: it becomes a new one.
    /// Takes what the task window edits from `n`. What came in from the
    /// server while the window was open (its version) stays.
    pub fn take_edits(&mut self, n: Task) {
        self.title = n.title;
        self.notes = n.notes;
        self.place = n.place;
        self.due = n.due;
        self.priority = n.priority;
        self.duration_min = n.duration_min;
        self.done = n.done;
        self.flexible = n.flexible;
        self.list = n.list;
        self.tags = n.tags;
        self.earliest = n.earliest;
        self.repeat = n.repeat;
        self.repeat_end = n.repeat_end;
        self.color = n.color;
    }
    pub fn detach(&mut self) {
        self.dirty = false;
        (self.uid, self.href, self.etag, self.raw) = (None, None, None, None);
    }
    /// The parts a server keeps are the same in both.
    pub fn same_content(&self, o: &Task) -> bool {
        self.title == o.title && self.notes == o.notes && self.place == o.place && self.due == o.due && self.priority == o.priority && self.done == o.done && self.tags == o.tags && self.duration_min == o.duration_min
    }
}
