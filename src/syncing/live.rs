//! Checks against the real accounts set up in katlendar, run by hand:
//!
//!     cargo test live_ -- --ignored --nocapture
//!
//! Each makes one test event a week or so ahead, named "katlendar map
//! check (delete me)", changes it, and deletes it again, also when a step
//! fails. They go through the same code as the app: the keyring's secret,
//! `open_account`, `sync::push` and `pull`. data.json and config.json are
//! only read, never written, so the app may stay open meanwhile.
//! KATLENDAR_LIVE_CALENDAR names the calendar to use (else the first one
//! that can be written to).

use super::{open_account, TokenCache};
use crate::model::*;
use crate::secrets;
use crate::sync::{self, PushJob, PushResult};
use chrono::{Duration, Local, NaiveDateTime, Utc};

const TITLE: &str = "katlendar map check (delete me)";

/// A place found with OpenStreetMap, so its spot is shared with the server.
fn osm_place() -> Option<Place> {
    Place::of("Kunsthaus Zürich, Heimplatz 1, 8001 Zürich", Some((47.37028, 8.54806)))
}

/// One account kind, end to end. Returns what each step saw.
fn run(kind: &str, pick: impl Fn(&Account) -> bool) -> Vec<String> {
    let data = Data::from_json(&std::fs::read_to_string(data_path()).expect("no data.json")).expect("data.json unreadable");
    let prefs: Prefs = std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let Some(a) = data.accounts.iter().find(|a| pick(a)) else {
        return vec![format!("{kind}: no such account set up; skipped")];
    };
    let wanted = std::env::var("KATLENDAR_LIVE_CALENDAR").ok();
    let local = data
        .calendars
        .iter()
        .filter(|c| c.account == Some(a.id) && !c.read_only && c.remote_id.is_some())
        .find(|c| wanted.as_deref().is_none_or(|w| c.name == w))
        .expect("no calendar to write to");
    let allow_file = prefs.plaintext_secrets;
    let secret = secrets::get(&secrets::account_key(a.id), allow_file).expect("keyring");
    let tokens = TokenCache::default();
    let mut p = open_account(a, &secret, &tokens, allow_file).expect("sign-in");
    let cals = p.list_calendars().expect("list calendars");
    let cal = cals.iter().find(|c| Some(&c.id) == local.remote_id.as_ref()).expect("calendar on the server");

    let now = Local::now().naive_local();
    let day = now.date() + Duration::days(9);
    let start: NaiveDateTime = day.and_hms_opt(7, 0, 0).unwrap();
    let mut e = Event { title: TITLE.into(), start, end: start + Duration::minutes(30), calendar: local.id, place: osm_place(), remote: true, ..Default::default() };
    let mut log = vec![format!("{kind}: calendar \"{}\"", local.name)];

    let steps = |p: &mut Box<dyn sync::Provider>, e: &mut Event, log: &mut Vec<String>| -> Result<(), String> {
        let save = |p: &mut Box<dyn sync::Provider>, e: &mut Event| -> Result<(), String> {
            match sync::push(p.as_mut(), cal, &[PushJob::Save(e.clone())], Utc::now().naive_utc()).remove(0) {
                PushResult::Saved { uid, href, etag, .. } => {
                    (e.uid, e.href, e.etag) = (Some(uid), Some(href), etag);
                    Ok(())
                }
                other => Err(format!("save: {other:?}")),
            }
        };
        let back = |p: &mut Box<dyn sync::Provider>, e: &Event| -> Result<Option<Place>, String> {
            let d = p.pull(cal, None, now).map_err(|err| format!("pull: {err:?}"))?;
            let found = d.upserts.into_iter().find(|f| Some(&f.uid) == e.uid.as_ref()).ok_or("the event did not come back")?;
            Ok(found.place)
        };
        let show = |p: &Option<Place>| match p {
            Some(p) => format!("{:?} spot {:?}", p.text, p.geo),
            None => "no place".into(),
        };

        save(p, e)?;
        let got = back(p, e)?;
        log.push(format!("made with a spot      -> {}", show(&got)));
        let raw = p.get(e.href.as_deref().unwrap()).map_err(|err| format!("get: {err:?}"))?.0;
        let lines: Vec<&str> = raw.lines().filter(|l| ["LOCATION", "GEO", "X-APPLE-STRUCTURED-LOCATION"].iter().any(|k| l.starts_with(k))).collect();
        log.push(format!("  the server holds    -> {lines:?}"));

        e.title = format!("{TITLE} renamed");
        save(p, e)?;
        log.push(format!("title changed         -> {}", show(&back(p, e)?)));

        e.place = None;
        save(p, e)?;
        log.push(format!("place cleared         -> {}", show(&back(p, e)?)));
        Ok(())
    };
    if let Err(err) = steps(&mut p, &mut e, &mut log) {
        log.push(format!("FAILED: {err}"));
    }
    // whatever happened, the test event goes
    if let Some(href) = e.href.clone() {
        let r = sync::push(p.as_mut(), cal, &[PushJob::Delete { href, etag: None }], Utc::now().naive_utc()).remove(0);
        log.push(format!("deleted               -> {}", matches!(r, PushResult::Deleted { .. })));
        if !matches!(r, PushResult::Deleted { .. }) {
            log.push(format!("  DELETE BY HAND: \"{}\" on {day} ({r:?})", e.title));
        }
    }
    log
}

#[test]
#[ignore = "talks to the real Proton account; run by hand"]
fn live_proton_places() {
    let log = run("Proton", |a| a.proton);
    println!("{}", log.join("\n"));
    assert!(!log.iter().any(|l| l.starts_with("FAILED") || l.contains("DELETE BY HAND")), "{}", log.join("\n"));
}
