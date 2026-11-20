//! Todoist, through its API v1 with the person's own API token. Each
//! project is a task list; tasks are read through the Sync endpoint, which
//! hands back only what changed since the last read, and written as
//! commands to the same endpoint.
//!
//! katlendar is not created by, affiliated with, or supported by Todoist.

use crate::strings::{tr, trf};
use super::oauth::url_encode;
use super::{Delta, Provider, PushResult, RemoteCalendar, SyncError};
use crate::ics::{FeedTask, TaskExtra};
use crate::model::{RemoteKey, Task};
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{json, Value};

pub const API: &str = "https://api.todoist.com/api/v1";
/// Where a person finds their API token.
pub const TOKEN_PAGE: &str = "https://app.todoist.com/app/settings/integrations/developer";

pub struct Todoist {
    /// where the API is reached; another address only in tests
    pub base: String,
    pub token: String,
}

/// Todoist's named project colours.
const COLORS: [(&str, [u8; 3]); 20] = [
    ("berry_red", [0xb8, 0x25, 0x6f]),
    ("red", [0xdb, 0x40, 0x35]),
    ("orange", [0xff, 0x99, 0x33]),
    ("yellow", [0xfa, 0xd0, 0x00]),
    ("olive_green", [0xaf, 0xb8, 0x3b]),
    ("lime_green", [0x7e, 0xcc, 0x49]),
    ("green", [0x29, 0x94, 0x38]),
    ("mint_green", [0x6a, 0xcc, 0xbc]),
    ("teal", [0x15, 0x8f, 0xad]),
    ("sky_blue", [0x14, 0xaa, 0xf5]),
    ("light_blue", [0x96, 0xc3, 0xeb]),
    ("blue", [0x40, 0x73, 0xff]),
    ("grape", [0x88, 0x4d, 0xff]),
    ("violet", [0xaf, 0x38, 0xeb]),
    ("lavender", [0xeb, 0x96, 0xeb]),
    ("magenta", [0xe0, 0x51, 0x94]),
    ("salmon", [0xff, 0x8d, 0x85]),
    ("charcoal", [0x80, 0x80, 0x80]),
    ("grey", [0xb8, 0xb8, 0xb8]),
    ("taupe", [0xcc, 0xac, 0x93]),
];

/// Todoist's priority, 4 the most urgent and 1 none, as katlendar counts
/// it: 0 none, 1 highest, 9 lowest.
fn priority_in(p: i64) -> u8 {
    match p {
        4 => 1,
        3 => 3,
        2 => 5,
        _ => 0,
    }
}

fn priority_out(p: u8) -> i64 {
    match p {
        1..=2 => 4,
        3 => 3,
        4..=5 => 2,
        _ => 1,
    }
}

/// The day of a due as Todoist writes it: a date, or a date and a time.
fn due_day(due: &Value) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(due["date"].as_str()?.get(..10)?, "%Y-%m-%d").ok()
}

/// A duration in minutes; a day counts as a whole day's minutes.
fn minutes(d: &Value) -> Option<i64> {
    let n = d["amount"].as_i64()?;
    match d["unit"].as_str()? {
        "day" => Some(n * 24 * 60),
        _ => Some(n),
    }
}

/// A task out of a Todoist item, with the item kept to write back from.
fn task_of(v: &Value) -> Option<FeedTask> {
    let id = v["id"].as_str()?.to_string();
    Some(FeedTask {
        uid: id.clone(),
        title: v["content"].as_str().unwrap_or("").to_string(),
        notes: v["description"].as_str().unwrap_or("").to_string(),
        due: due_day(&v["due"]),
        priority: priority_in(v["priority"].as_i64().unwrap_or(1)),
        done: v["checked"].as_bool().unwrap_or(false),
        href: Some(id),
        etag: None,
        raw: Some(v.to_string()),
        extra: Some(TaskExtra {
            tags: v["labels"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            duration_min: minutes(&v["duration"]),
        }),
    })
}

/// The fields of an item that a task here sets. A due that repeats is
/// left as it is: a date alone would end the repeat. A due whose day did
/// not change keeps its time.
fn fields(t: &Task, old: &Value) -> Value {
    let mut a = json!({
        "content": t.title,
        "description": t.notes,
        "priority": priority_out(t.priority),
        "labels": t.tags,
    });
    let repeats = old["due"]["is_recurring"].as_bool() == Some(true);
    if !repeats && due_day(&old["due"]) != t.due {
        a["due"] = match t.due {
            Some(d) => json!({ "date": d.format("%Y-%m-%d").to_string() }),
            None => Value::Null,
        };
    }
    if minutes(&old["duration"]) != t.duration_min {
        a["duration"] = match t.duration_min.filter(|m| *m > 0) {
            Some(m) => json!({ "amount": m, "unit": "minute" }),
            None => Value::Null,
        };
    }
    a
}

fn command(kind: &str, args: Value) -> Value {
    json!({ "type": kind, "uuid": super::new_uid(), "args": args })
}

impl Todoist {
    pub fn new(token: &str) -> Todoist {
        Todoist { base: API.into(), token: token.trim().to_string() }
    }

    fn call(&self, method: &str, path: &str, form: &[(&str, String)]) -> Result<Value, SyncError> {
        // the token only goes to Todoist itself
        if !(self.base.starts_with("https://") || self.base.starts_with("http://127.0.0.1:")) {
            return Err(SyncError::Other(tr("The server sent an address that is not its own.").into()));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(30))).http_status_as_error(false).max_redirects(0).build().into();
        let body = form.iter().map(|(k, v)| format!("{k}={}", url_encode(v))).collect::<Vec<_>>().join("&");
        let mut req = ureq::http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base))
            .header("User-Agent", "katlendar")
            .header("Authorization", format!("Bearer {}", self.token));
        if method == "POST" {
            req = req.header("Content-Type", "application/x-www-form-urlencoded");
        }
        let req = req.body(body).map_err(|e| SyncError::Other(e.to_string()))?;
        let mut resp = agent.run(req).map_err(|e| SyncError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        match status {
            200..=299 => Ok(body),
            401 | 403 => Err(SyncError::Auth),
            404 => Err(SyncError::Gone),
            429 | 502..=504 => Err(SyncError::RateLimited),
            code => {
                let why = body["error"].as_str().map(str::to_string).unwrap_or_else(|| text.chars().take(200).collect());
                Err(SyncError::Other(trf("Todoist answered with error {}: {}", &[&code, &why])))
            }
        }
    }

    /// Sends commands; returns the reply with each command's fate.
    fn send(&self, commands: &[Value]) -> Result<Value, SyncError> {
        self.call("POST", "/sync", &[("commands", Value::from(commands.to_vec()).to_string())])
    }

    /// How a command fared, from the reply's `sync_status`.
    fn status(reply: &Value, cmd: &Value) -> Result<(), SyncError> {
        let s = &reply["sync_status"][cmd["uuid"].as_str().unwrap_or("")];
        if s.as_str() == Some("ok") {
            return Ok(());
        }
        match s["http_code"].as_i64() {
            Some(401 | 403) => Err(SyncError::Auth),
            Some(404) => Err(SyncError::Gone),
            Some(429) => Err(SyncError::RateLimited),
            _ => Err(SyncError::Other(trf("Todoist did not take the change: {}", &[&s["error"].as_str().unwrap_or("no reason given")]))),
        }
    }
}

impl Provider for Todoist {
    /// The account is listed once it has somewhere it was found.
    fn home(&self) -> Option<String> {
        Some(self.base.clone())
    }

    /// Each project, but for archived ones, is a task list.
    fn list_calendars(&mut self) -> Result<Vec<RemoteCalendar>, SyncError> {
        let mut out = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let path = match &cursor {
                Some(c) => format!("/projects?limit=200&cursor={}", url_encode(c)),
                None => "/projects?limit=200".to_string(),
            };
            let page = self.call("GET", &path, &[])?;
            for p in page["results"].as_array().into_iter().flatten() {
                let Some(id) = p["id"].as_str() else { continue };
                if p["is_archived"].as_bool() == Some(true) || p["is_deleted"].as_bool() == Some(true) {
                    continue;
                }
                let color = p["color"].as_str().and_then(|c| COLORS.iter().find(|(n, _)| *n == c)).map(|(_, rgb)| *rgb);
                out.push(RemoteCalendar {
                    id: id.to_string(),
                    name: p["name"].as_str().unwrap_or("Todoist").to_string(),
                    color,
                    tasks: true,
                    events: false,
                    // someone who may only look at a shared project
                    read_only: p["role"].as_str().is_some_and(|r| r.eq_ignore_ascii_case("read_only")),
                    ..Default::default()
                });
            }
            match page["next_cursor"].as_str() {
                Some(c) if !c.is_empty() => cursor = Some(c.to_string()),
                _ => return Ok(out),
            }
        }
        Err(SyncError::Other(tr("The server's list did not end.").into()))
    }

    fn pull(&mut self, _cal: &RemoteCalendar, _state: Option<&str>, _now: NaiveDateTime) -> Result<Delta, SyncError> {
        Err(SyncError::Other(tr("Todoist holds tasks, not events.").into()))
    }

    /// `state` is Todoist's sync token. It covers the whole account, so
    /// items of other projects come along too: those that are here have
    /// moved away.
    fn pull_tasks(&mut self, cal: &RemoteCalendar, state: Option<&str>, _now: NaiveDateTime) -> Result<Delta, SyncError> {
        let reply = self.call("POST", "/sync", &[("sync_token", state.unwrap_or("*").to_string()), ("resource_types", r#"["items"]"#.to_string())])?;
        let full = reply["full_sync"].as_bool().unwrap_or(state.is_none());
        let mut d = Delta { full, state: reply["sync_token"].as_str().map(str::to_string), ..Default::default() };
        for v in reply["items"].as_array().into_iter().flatten() {
            let Some(id) = v["id"].as_str() else { continue };
            let here = v["project_id"].as_str() == Some(cal.id.as_str()) && v["is_deleted"].as_bool() != Some(true);
            match task_of(v).filter(|_| here) {
                Some(t) => d.tasks.push(t),
                None if !full => d.deletes.push(RemoteKey::Href(id.to_string())),
                None => {}
            }
        }
        Ok(d)
    }

    fn save_task_native(&mut self, cal: &RemoteCalendar, t: &Task) -> Option<PushResult> {
        let old: Value = t.raw.as_deref().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
        let mut cmds = vec![];
        let id = match &t.href {
            Some(id) => {
                let mut a = fields(t, &old);
                a["id"] = json!(id);
                cmds.push(command("item_update", a));
                id.clone()
            }
            None => {
                let temp = super::new_uid();
                let mut a = fields(t, &Value::Null);
                a["project_id"] = json!(cal.id);
                let mut c = command("item_add", a);
                c["temp_id"] = json!(temp);
                cmds.push(c);
                temp
            }
        };
        let was_done = old["checked"].as_bool().unwrap_or(false);
        if t.done && !was_done {
            // a repeating task moves on to its next day; others are done
            cmds.push(command("item_close", json!({ "id": id })));
        } else if !t.done && was_done {
            cmds.push(command("item_uncomplete", json!({ "id": id })));
        }
        let reply = match self.send(&cmds) {
            Ok(r) => r,
            Err(error) => return Some(PushResult::Failed { error }),
        };
        let id = reply["temp_id_mapping"][&id].as_str().map(str::to_string).unwrap_or(id);
        for c in &cmds {
            match Self::status(&reply, c) {
                Ok(()) => {}
                Err(SyncError::Gone) => return Some(PushResult::TaskConflict { task: t.id }),
                Err(error) => return Some(PushResult::Failed { error }),
            }
        }
        Some(PushResult::TaskSaved { sent: t.clone(), uid: id.clone(), href: id, etag: None })
    }

    fn delete(&mut self, _cal: &RemoteCalendar, href: &str, _etag: Option<&str>) -> Result<(), SyncError> {
        let c = command("item_delete", json!({ "id": href }));
        let reply = self.send(std::slice::from_ref(&c))?;
        Self::status(&reply, &c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// How the stand-in answers one request, from the body it was sent.
    type Reply = fn(&str) -> (&'static str, String);

    /// A stand-in for Todoist: answers requests in order, and keeps what
    /// it was sent ("METHOD path\nheaders\n\nbody", the body decoded).
    fn stand_in(replies: Vec<Reply>) -> (Todoist, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}/api/v1", listener.local_addr().unwrap().port());
        let got = Arc::new(Mutex::new(vec![]));
        let seen = got.clone();
        std::thread::spawn(move || {
            for answer in replies {
                let (mut s, _) = listener.accept().unwrap();
                let (head, body) = super::super::read_request(&mut s);
                let body = decode(&body);
                let (status, reply) = answer(&body);
                seen.lock().unwrap().push(format!("{head}\n\n{body}"));
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            }
        });
        (Todoist { base, token: "test-token".into() }, got)
    }

    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let (mut out, mut i) = (vec![], 0);
        while i < b.len() {
            match b[i] {
                b'%' if i + 2 < b.len() => {
                    out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                    i += 3;
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).unwrap()
    }

    /// Takes every command sent, and maps each temporary id to "NEW1".
    fn all_ok(body: &str) -> (&'static str, String) {
        let cmds: Value = serde_json::from_str(body.strip_prefix("commands=").unwrap()).unwrap();
        let status: serde_json::Map<String, Value> = cmds.as_array().unwrap().iter().map(|c| (c["uuid"].as_str().unwrap().to_string(), json!("ok"))).collect();
        let temps: serde_json::Map<String, Value> = cmds.as_array().unwrap().iter().filter_map(|c| c["temp_id"].as_str()).map(|t| (t.to_string(), json!("NEW1"))).collect();
        ("200 OK", json!({ "sync_status": status, "temp_id_mapping": temps }).to_string())
    }

    fn commands(sent: &str) -> Value {
        serde_json::from_str(sent.split("\n\n").nth(1).unwrap().strip_prefix("commands=").unwrap()).unwrap()
    }

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 20).unwrap().and_hms_opt(12, 0, 0).unwrap()
    }

    fn list(id: &str) -> RemoteCalendar {
        RemoteCalendar { id: id.into(), tasks: true, ..Default::default() }
    }

    #[test]
    fn priorities_go_both_ways() {
        for p in 1..=4 {
            assert_eq!(priority_out(priority_in(p)), p);
        }
        assert_eq!(priority_out(0), 1);
        assert_eq!(priority_out(9), 1);
    }

    #[test]
    fn projects_become_task_lists() {
        let (mut t, got) = stand_in(vec![
            |_| ("200 OK", r#"{"results":[{"id":"P1","name":"Inbox","color":"grey","inbox_project":true},{"id":"P2","name":"Old","is_archived":true}],"next_cursor":"c2"}"#.into()),
            |_| ("200 OK", r#"{"results":[{"id":"P3","name":"Work","color":"berry_red","role":"READ_ONLY"}],"next_cursor":null}"#.into()),
        ]);
        let lists = t.list_calendars().unwrap();
        assert_eq!(lists.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["P1", "P3"]);
        assert!(lists.iter().all(|l| l.tasks && !l.events));
        assert_eq!(lists[1].color, Some([0xb8, 0x25, 0x6f]));
        assert!(lists[1].read_only && !lists[0].read_only);
        let got = got.lock().unwrap();
        assert!(got[0].starts_with("GET /api/v1/projects?limit=200 "));
        assert!(got[0].contains("Bearer test-token"));
        assert!(got[1].contains("cursor=c2"));
    }

    #[test]
    fn items_of_the_project_are_read() {
        let (mut t, got) = stand_in(vec![
            |_| ("200 OK", r#"{"sync_token":"S1","full_sync":true,"items":[
                {"id":"I1","project_id":"P1","content":"Write report","description":"two pages","priority":4,"checked":false,
                 "due":{"date":"2026-11-25T10:00:00","is_recurring":false},"labels":["work"],"duration":{"amount":45,"unit":"minute"}},
                {"id":"I2","project_id":"P9","content":"Elsewhere","priority":1}]}"#.into()),
            |_| ("200 OK", r#"{"sync_token":"S2","full_sync":false,"items":[
                {"id":"I1","project_id":"P1","content":"Write report","priority":1,"checked":true},
                {"id":"I3","project_id":"P1","content":"Gone","is_deleted":true},
                {"id":"I4","project_id":"P9","content":"Moved away"}]}"#.into()),
        ]);
        let d = t.pull_tasks(&list("P1"), None, now()).unwrap();
        assert!(d.full);
        assert_eq!(d.state.as_deref(), Some("S1"));
        assert_eq!(d.tasks.len(), 1);
        let f = &d.tasks[0];
        assert_eq!((f.uid.as_str(), f.title.as_str(), f.notes.as_str(), f.priority, f.done), ("I1", "Write report", "two pages", 1, false));
        assert_eq!(f.due, NaiveDate::from_ymd_opt(2026, 11, 25));
        assert_eq!(f.extra, Some(TaskExtra { tags: vec!["work".into()], duration_min: Some(45) }));
        assert!(d.deletes.is_empty());

        let d = t.pull_tasks(&list("P1"), Some("S1"), now()).unwrap();
        assert!(!d.full);
        assert!(d.tasks[0].done);
        assert_eq!(d.deletes, vec![RemoteKey::Href("I3".into()), RemoteKey::Href("I4".into())]);
        let got = got.lock().unwrap();
        assert!(got[0].starts_with("POST /api/v1/sync "));
        assert!(got[0].contains("sync_token=*") && got[0].contains(r#"resource_types=["items"]"#));
        assert!(got[1].contains("sync_token=S1"));
    }

    #[test]
    fn a_new_task_is_added_and_done() {
        let (mut t, got) = stand_in(vec![all_ok]);
        let task = Task { id: 7, title: "Call Sam".into(), priority: 1, due: NaiveDate::from_ymd_opt(2026, 12, 1), done: true, tags: vec!["phone".into()], ..Default::default() };
        match t.save_task_native(&list("P1"), &task) {
            Some(PushResult::TaskSaved { uid, href, sent, .. }) => assert_eq!((uid.as_str(), href.as_str(), sent.id), ("NEW1", "NEW1", 7)),
            other => panic!("{other:?}"),
        }
        let cmds = commands(&got.lock().unwrap()[0]);
        assert_eq!(cmds[0]["type"], "item_add");
        assert_eq!(cmds[0]["args"]["content"], "Call Sam");
        assert_eq!(cmds[0]["args"]["project_id"], "P1");
        assert_eq!(cmds[0]["args"]["priority"], 4);
        assert_eq!(cmds[0]["args"]["due"]["date"], "2026-12-01");
        assert_eq!(cmds[0]["args"]["labels"], json!(["phone"]));
        assert_eq!(cmds[1]["type"], "item_close");
        assert_eq!(cmds[1]["args"]["id"], cmds[0]["temp_id"]);
    }

    #[test]
    fn a_task_done_elsewhere_can_be_undone_here() {
        let old = json!({"id":"I1","project_id":"P1","content":"Pay rent","priority":1,"checked":true});
        let (mut t, got) = stand_in(vec![all_ok, |_| ("200 OK", r#"{"sync_status":{}}"#.into())]);
        let task = Task { id: 3, title: "Pay rent".into(), href: Some("I1".into()), raw: Some(old.to_string()), ..Default::default() };
        assert!(matches!(t.save_task_native(&list("P1"), &task), Some(PushResult::TaskSaved { .. })));
        let cmds = commands(&got.lock().unwrap()[0]);
        assert_eq!((cmds[0]["type"].as_str(), cmds[0]["args"]["id"].as_str()), (Some("item_update"), Some("I1")));
        assert_eq!(cmds[1]["type"], "item_uncomplete");
        // a task deleted on Todoist meanwhile is a conflict
        let gone = |b: &str| {
            let cmds: Value = serde_json::from_str(b.strip_prefix("commands=").unwrap()).unwrap();
            let u = cmds[0]["uuid"].as_str().unwrap().to_string();
            ("200 OK", json!({"sync_status": {u: {"error": "Item not found", "http_code": 404}}}).to_string())
        };
        let (mut t, _) = stand_in(vec![gone]);
        assert!(matches!(t.save_task_native(&list("P1"), &task), Some(PushResult::TaskConflict { task: 3 })));
    }

    #[test]
    fn a_task_is_deleted() {
        let (mut t, got) = stand_in(vec![all_ok]);
        t.delete(&list("P1"), "I9", None).unwrap();
        let cmds = commands(&got.lock().unwrap()[0]);
        assert_eq!((cmds[0]["type"].as_str(), cmds[0]["args"]["id"].as_str()), (Some("item_delete"), Some("I9")));
    }

    #[test]
    fn a_change_keeps_what_was_not_touched() {
        // the server's copy: a due with a time, repeating, not done
        let old = json!({"id":"I1","project_id":"P1","content":"Water plants","priority":1,"checked":false,
            "due":{"date":"2026-11-25T08:00:00","is_recurring":true,"string":"every day at 8"},"duration":{"amount":10,"unit":"minute"}});
        let task = Task { title: "Water the plants".into(), due: NaiveDate::from_ymd_opt(2026, 11, 26), duration_min: Some(10), href: Some("I1".into()), raw: Some(old.to_string()), ..Default::default() };
        let a = fields(&task, &old);
        assert_eq!(a["content"], "Water the plants");
        // a repeating due is not overwritten by a date
        assert!(a.get("due").is_none());
        assert!(a.get("duration").is_none());

        // a plain due whose day is the same keeps its time; a cleared one goes
        let old = json!({"due":{"date":"2026-11-25T08:00:00","is_recurring":false},"duration":{"amount":10,"unit":"minute"}});
        let mut task = Task { due: NaiveDate::from_ymd_opt(2026, 11, 25), duration_min: Some(10), ..Default::default() };
        assert!(fields(&task, &old).get("due").is_none());
        task.due = None;
        task.duration_min = None;
        let a = fields(&task, &old);
        assert_eq!((a["due"].clone(), a["duration"].clone()), (Value::Null, Value::Null));
        assert!(a.get("due").is_some());
    }

    #[test]
    fn failures_are_told_apart() {
        let (mut t, _) = stand_in(vec![|_| ("401 Unauthorized", "{}".into()), |_| ("429 Too Many Requests", "{}".into())]);
        assert_eq!(t.list_calendars().unwrap_err(), SyncError::Auth);
        assert_eq!(t.pull_tasks(&list("P1"), None, now()).unwrap_err(), SyncError::RateLimited);
        let reply = json!({"sync_status": {"u1": {"error": "Item not found", "http_code": 404}, "u2": "ok"}});
        assert_eq!(Todoist::status(&reply, &json!({"uuid": "u1"})), Err(SyncError::Gone));
        assert_eq!(Todoist::status(&reply, &json!({"uuid": "u2"})), Ok(()));
    }
}
