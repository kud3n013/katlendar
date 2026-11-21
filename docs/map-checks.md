# Map checks against the real services

The tests in `src/ics.rs`, `src/sync/graph.rs` and `src/sync/proton.rs` read and write the example
events in `src/sync/fixtures/`. Those files are modelled on what each service sends. They are
**not** captures from real accounts, so they cannot answer what a server keeps or throws away.
This checklist does that, by hand, with real accounts.

Use a calendar you don't mind cluttering, and delete the test events afterwards.

## What we most need to know

1. **Does Google keep `GEO` and `X-APPLE-STRUCTURED-LOCATION` over CalDAV?**
   If it drops them, a spot found with OpenStreetMap is lost at the next sync: katlendar keeps its
   own spot when the server sends the same words back only for places found on Google Maps
   (`place::from_server`). A spot from Google Maps stays either way.
2. **Does Proton keep them in the encrypted part?** We write them there; we don't know if Proton's
   web app keeps them when the event is edited there.
3. **Does Outlook keep `coordinates`** when the event is edited in Outlook web, or does it replace
   them with its own lookup?
4. **Does iCloud keep Apple's line** with its map handle when katlendar changes something other
   than the place (the tests say we do; the server may not)?

## From katlendar out

For each account (Google, Outlook, iCloud or Nextcloud, Proton):

- [ ] Make an event with a place found with OpenStreetMap, so it has a spot. Sync.
- [ ] Open it in the service's web app. Is the place shown? Does it open the right spot on a map?
- [ ] On a phone (Google Calendar, Outlook, Apple Calendar): the same.
- [ ] Sync again in katlendar, then reopen the event. Is the spot still there? (Question 1 for
      Google.)
- [ ] Change only the title in katlendar and sync. Is the spot still there in the web app?
- [ ] Change only the title in the web app, sync in katlendar. Is the spot still there here?
- [ ] Clear the place in katlendar and sync. Is it gone everywhere, with no stale spot left?
- [ ] For Google only: repeat with a place found with Google Maps.

## From the services in

- [ ] **Google Calendar web:** pick a place from Google's suggestions. In katlendar: are the words
      right (commas, no `\,`)? There will be no spot; Open in should still find it.
- [ ] **Outlook web:** pick a place from Bing's suggestions. In katlendar: words and spot. Also a
      Teams meeting (should read "Microsoft Teams Meeting" with no spot) and a meeting room.
- [ ] **Apple Calendar (iPhone or Mac) on iCloud:** pick a place from Apple Maps. In katlendar:
      the name and address (two lines) and a spot.
- [ ] **Nextcloud web and Thunderbird:** type a place. In katlendar: the words, no spot.
- [ ] **Proton web:** type a place. In katlendar: the words.

## Notes

Write down what each service did, with the date and the app's version, under "As built in phase 7"
in `docs/research/map-integration.md`. If a server drops the spot lines, a raw copy of its event
(what katlendar received) makes a better fixture than the ones we have now.
