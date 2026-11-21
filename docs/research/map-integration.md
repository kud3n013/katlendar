# Map integration for katlendar

Research notes, gathered 2026-11-21. Scope: how calendars store an event's place, which map
providers a desktop egui app can use, and a plan for a place picker, provider settings and
round-trip compatibility with Google, Microsoft, CalDAV and Proton. Nothing here is built yet.

Citation conventions used below:

- A bare link after a claim is the **primary source** and was read for this note.
- **(unverified)** means it comes from memory or a search snippet only. Check it before relying on it.

---

## 1. TL;DR

- **The shared format is one line of text.** Every calendar has a free-text location, and none
  of them reads another app's coordinates reliably. A good place is a **readable name plus full
  address** in that text line, which Google, Outlook and Apple all turn into a map link
  themselves. Coordinates are extras, written where the format has a slot for them.
- **Google Calendar has nothing but text.** `location` is a string, "free-form text"; the event
  has no geo field. So a place picked here shows up in Google Calendar exactly as well as its text
  is written: `Name, street, city`.
- **Microsoft has the richest slot.** Graph's `location` has `displayName`, a structured `address`,
  `coordinates` and `locationUri`.
- **CalDAV, Nextcloud, Proton and `.ics`:** `LOCATION` text, `GEO:lat;lon` (RFC 5545), and Apple's
  `X-APPLE-STRUCTURED-LOCATION` (unverified), which makes Apple Calendar show a map.
- **Default provider: OpenStreetMap tiles plus Photon search.** Both are free with no key. Photon
  allows search-as-you-type; Nominatim forbids it. **Google Maps is an opt-in provider with the
  user's own API key**, the same bring-your-own-registration model katlendar already uses for
  Google sign-in.
- **Draw the map ourselves** with `ureq` and `image`, which are already dependencies, instead of
  the `walkers` crate. `walkers` matches egui 0.37 but brings tokio, reqwest and a second TLS
  stack.

## 2. How katlendar handles places today

- There is no place field. The editor's Location row (`src/dialogs.rs`, `join_place`) stores the
  place as a first line `Where: …` inside `Event::notes`.
- On sync, `place_and_notes` (`src/sync/mod.rs:180`) splits it back out into iCalendar `LOCATION`
  (CalDAV, Google CalDAV, Nextcloud, Proton) and Graph `location.displayName`
  (`src/sync/graph.rs:76,112`).
- ICS feeds read `LOCATION` into a `Where:` line too (`src/ics.rs:525`).
- The server's raw event is kept (`Event::raw`) and only edited fields are rewritten, so unknown
  properties such as `GEO` survive a round-trip today. That is also a trap: if the place text
  changes and a stale `GEO` line stays, other apps show the old pin (see §6).
- Tasks (`Task::notes`) have no place, and the task editor has no Location row.

## 3. How other calendars store a place

| Calendar | Place field | Coordinates | Map shown from |
| --- | --- | --- | --- |
| Google Calendar (API v3 and CalDAV) | `location`: string, "Geographic location of the event as free-form text" | None on the event | Google geocodes the text and links to Google Maps |
| Outlook / Microsoft 365 (Graph) | `location` object: `displayName`, `address` (physicalAddress), `coordinates`, `locationUri`, `locationType`; plus a `locations` list | `coordinates` (outlookGeoCoordinates) | The address or coordinates |
| Apple Calendar / iCloud | `LOCATION` text plus `X-APPLE-STRUCTURED-LOCATION;X-TITLE=…;X-ADDRESS=…:geo:lat,lon` **(unverified)** | In the geo URI | The structured location, else the text |
| Generic CalDAV, Nextcloud, Proton, `.ics` | `LOCATION` text | `GEO:lat;lon` (RFC 5545) | Depends on the client |
| Todoist | No place field (location reminders exist only in its own apps, **unverified**) | None | Nothing |

Sources:

- Google: https://developers.google.com/workspace/calendar/api/v3/reference/events
- Graph: https://learn.microsoft.com/en-us/graph/api/resources/location?view=graph-rest-1.0.
  `uniqueId` and `uniqueIdType` are "internal use only". Rooms often come back with empty
  `coordinates`. Setting `location` and `locations` overwrite each other.
- Structured locations in iCalendar: RFC 9073 (`STRUCTURED-DATA`, `VLOCATION`),
  https://www.rfc-editor.org/rfc/rfc9073. Few clients write it.
- A current IETF draft deprecates `GEO` in favour of a `COORDINATES` property with a `geo:` URI:
  https://www.ietf.org/archive/id/draft-ietf-calext-icalendar-jscalendar-extensions-06.html. It is
  a draft, so write `GEO` for now and read both.
- Apple's EventKit has `EKEvent.structuredLocation` with a geo coordinate:
  https://developer.apple.com/documentation/eventkit/ekevent/structuredlocation. The exact `.ics`
  syntax above is from memory. Confirm it by exporting an event from Apple Calendar.

**Implication.** Write the text so it stands alone, as `Name, street, postcode city, country`.
Never write only coordinates as the text. Add coordinates only in the slots that exist.

## 4. Map providers

A "provider" covers three jobs: **search** (geocoding as you type, and reverse geocoding when the
pin is dropped), **tiles** (the map image), and an **Open in** link to an outside map. A provider's
terms can tie these together, so katlendar should pick them as a bundle.

| Provider | Key | Search as you type | Tiles | Cost | Notes |
| --- | --- | --- | --- | --- | --- |
| **OpenStreetMap + Photon** (default) | None | Photon: yes | OSM raster PNG | Free, fair use | See the policy notes below. |
| OSM + Nominatim | None | **Forbidden** | OSM raster | Free | Fine for a single lookup on Enter, not for typing. |
| OpenFreeMap | None | No search service | Vector only (MapLibre styles) | Free, no limits | Needs a vector renderer. Good later for a dark map style. |
| Google Maps Platform | User's own key, billing enabled | Places Autocomplete | Map Tiles API (2D) | Monthly free caps per SKU, then paid | Best match for Google Calendar users. |
| MapTiler / Stadia / Geoapify | User's own key | Yes | Raster | Free tiers | One "keyed OSM" provider covers these later. |
| Apple Maps | Developer account | MapKit JS (web) | No native tiles for an egui app | $99/year | **Open in link only.** |
| Bing Maps | — | — | — | — | Being retired in favour of Azure Maps **(unverified)**. **Open in link only.** |

### Policy notes (these shape the code)

- **Nominatim** (https://operations.osmfoundation.org/policies/nominatim/): "Auto-complete search"
  is listed as unacceptable use, and the policy says you must not implement it on the client side.
  It allows "an absolute maximum of 1 request per second" across all users of an app, and requires
  a User-Agent naming the app. Library defaults are refused. So it cannot be the picker's search
  box, only a fallback lookup on Enter.
- **OSM tiles** (https://operations.osmfoundation.org/policies/tiles/):
  - Native apps may use them with "a clear, unique User-Agent string that names your app", for
    example `katlendar/0.1.0 (+https://github.com/kud3n013/katlendar)`.
  - Attribution must be on the map, bottom right: "© OpenStreetMap contributors".
  - Honour `Cache-Control`, `Expires` and `ETag`. If a cache cannot read them, keep each tile at
    least 7 days. Use conditional requests.
  - **No prefetching and no offline download**, so tiles are fetched only as the user pans.
- **Photon** (https://photon.komoot.io/): the public instance is free with "please be fair"
  throttling, and has no availability guarantee. It supports typeahead, `lat`/`lon` bias, `lang`,
  and `/reverse`. Debounce typing (about 300 ms) and cache the answers. Let the user point at a
  self-hosted Photon URL.
- **OpenFreeMap** (https://openfreemap.org/): no key, no limits, commercial use allowed. It serves
  vector tiles only, so it is not usable until katlendar can draw vector tiles.
- **Google Maps URLs** (https://developers.google.com/maps/documentation/urls/get-started): no key
  needed.
  - Search: `https://www.google.com/maps/search/?api=1&query=<urlencoded>`
  - A specific place: add `&query_place_id=<id>`
  - Coordinates: `query=<lat>%2C<lon>`
- **Google Maps Platform pricing** (https://developers.google.com/maps/billing-and-pricing/pricing):
  Autocomplete gets 10,000 free requests a month and 2D Map Tiles 100,000, then paid. Figures
  were not checked for currency.
- **Google Maps Platform terms (unverified, the pages would not load).** From memory: Google
  results may only be shown on a Google map; coordinates may be cached for up to 30 days; place
  ids may be stored indefinitely. If that holds:
  - The Google provider must use Google tiles as well as Google search.
  - Stored coordinates from a Google search need a refresh rule, or store only the place id and
    the text.

  **As built in phase 5.** The Service Specific Terms page still would not load, so the code
  follows the rules above, which match Google's caching guidance in the Places API docs:
  - The place id is kept for good (`Place::google_id`); "Open in Google Maps" uses it as
    `query_place_id`.
  - A spot from Google is marked with the day it was had (`Place::geo_from_google`) and dropped
    after 30 days (`Place::expire`, run at start-up). The words and the place id stay.
  - A spot from Google is never written to a calendar server (`Place::shared_geo`), so only spots
    put down by hand or found through OpenStreetMap leave the app.
  - With Google as provider, the map under a place is Google's too (Map Tiles API, 2D roadmap),
    kept in memory only, never in the disk cache. "Google Maps" and the viewport's copyright are
    shown bottom right. The Google logo image is not drawn yet: the text stands in for it.
  - A search and the Place Details call for the place picked share one session token, then a
    new one is made. Details ask only `id,formattedAddress,location,viewport` (Essentials).
  - The address at a spot dropped by hand comes from the Geocoding API.
  - The key is read from the keyring at start-up and added to a request only as it is sent.
  - Not yet checked against a live key: the Details `viewport` field names and the Map Tiles
    viewport `copyright` reply. Both are read defensively and fall back to no extent and no
    copyright line.

### Recommendation

1. **Default: "OpenStreetMap"**, which is OSM raster tiles plus Photon search and reverse lookup.
   It needs no key and no account, which fits "everything is local, no account".
2. **Opt-in: "Google Maps"** with the user's own API key, kept in the keyring through
   `src/secrets.rs`. It uses Places Autocomplete with session tokens, Place Details for coordinates
   and the formatted address, and Map Tiles for the map.
3. **Later: one "OSM with a key" provider** (MapTiler or Stadia) for people who want a nicer or
   dark style without Google.
4. **The Open in target is chosen separately** from the search provider: Google Maps, Apple Maps,
   OpenStreetMap, or the system default (`geo:` URI on Linux). It costs nothing and needs no key.

### Drawing the map in egui

| Option | For | Against |
| --- | --- | --- |
| `walkers` 0.66 (egui `^0.37`, matches) | Panning, zooming and markers done; plugins | Pulls in tokio, reqwest, reqwest-middleware, http-cache and rustls beside katlendar's ureq stack: a bigger binary and two HTTP clients |
| **Own raster tile widget** (recommended) | Uses `ureq`, `image` and the existing worker thread; disk cache next to `data.json`; full control of User-Agent and cache headers | About 300–400 lines: tile maths, pan and zoom, texture LRU, attribution |

The own widget follows the project's habit of small, owned pieces, as with the hand-written Proton
SRP and OAuth. It needs the `jpeg` feature on `image`, because some keyed providers serve JPEG.

## 5. Design

### Data

```rust
/// where an event or task happens
pub struct Place {
    /// the one line every calendar shows: "Name, street, city"
    pub text: String,
    /// a short name, when the text starts with one ("Café Lisboa")
    pub name: Option<String>,
    pub lat_lon: Option<(f64, f64)>,
    /// the provider's id for it, such as a Google place id or OSM N123
    pub source: Option<PlaceRef>,
}
```

- Add `place: Option<Place>` to `Event` and `Task` with `#[serde(default)]`.
- `Data::migrate` moves a leading `Where: …` line out of `notes` into `place.text`.
  `place_and_notes` stays only for reading old files.

### Writing to each server (and reading back)

| Target | Write | Read |
| --- | --- | --- |
| Google (CalDAV) | `LOCATION` = full text. `GEO` too (Google probably drops it; harmless) | `LOCATION` |
| Microsoft Graph | `location.displayName` = name or text; `address` from the parts; `coordinates`; `locationUri` = Open in link | All of these; fill `lat_lon` from `coordinates` when present |
| CalDAV, Nextcloud, Proton | `LOCATION`, `GEO:lat;lon`, `X-APPLE-STRUCTURED-LOCATION` | `X-APPLE-STRUCTURED-LOCATION`, then `GEO`, then `COORDINATES`, then text only |
| `.ics` feeds | — | As CalDAV |
| Todoist | Nothing. The place stays in katlendar, like side notes | — |
| CalDAV tasks (VTODO), if added later | `LOCATION` and `GEO` are valid on VTODO too | Same |

- Events with text but no coordinates are **not** geocoded in the background, because that would
  break Photon's fair use and Nominatim's limits.
- A place is looked up only when the user opens the picker or presses Open in. Open in works from
  text alone.

### Screens

- **Event editor and flyout, Location row** (`src/dialogs.rs`, near line 1525):
  - The text field stays.
  - A map-pin button opens the picker.
  - When a place has coordinates, the row shows "Open in <target>".
  - Read-only events get Open in only.
- **Task editor** (`task_fields`, `src/dialogs.rs:1764`): the same Location row.
- **Picker dialog:**
  - A search box with debounced results showing name and address, biased to the map's centre.
  - The map has a pin. Clicking or dragging the pin looks up the address there.
  - "Use this place" fills `Place`. "Text only" keeps what was typed and drops the coordinates.
  - Attribution sits bottom right.
  - Esc closes the dialog. Arrow keys and Enter move through and pick results.
- **Settings, new "Maps" page** (`src/settings.rs`):
  - Provider: OpenStreetMap or Google Maps. Choosing Google asks for an API key (keyring).
  - Open places in: Google Maps, Apple Maps, OpenStreetMap or the system default.
  - Photon server URL (advanced) for self-hosting.
  - A short note on privacy: searches go to the chosen provider.
- **Per calendar** (`page_calendars`, `src/settings.rs:1204`):
  - "Map provider" and "Open in": Same as default, or a choice.
  - Stored in `Prefs` the way `buffers: Vec<CalBuffer>` is: `calendar_maps: Vec<CalMap>`.
  - Calendars of a Google account start out preferring Google Maps for Open in, and for search
    when a key is set.
- **As built in phase 6:**
  - The per-calendar choices sit on the Maps page, under "Maps for each calendar", not in
    `page_calendars`. That page is already full of accounts and sign-in, and the two choices
    read better next to the defaults they override.
  - A calendar with nothing set follows its start-out value, which is worked out each time from
    its account. `calendar_maps` keeps only the calendars set to something else, so a later
    change of the defaults reaches every calendar left alone.
  - The Google key row shows when the app default or any calendar finds places with Google.
    Without a key, those calendars still search OpenStreetMap.
  - Tasks have no calendar, so the task editor uses the app defaults.
  - An address that is not a web address in "Photon server" is ignored, and the page says so.

### Code layout

- `src/maps/mod.rs`
  - `MapProvider` enum with `search`, `reverse`, `tile_url`, `attribution` and `open_url`.
  - A rate limiter.
  - The User-Agent string.
  - `Place` ⇄ iCalendar and Graph conversion.
- `src/maps/osm.rs`, `src/maps/google.rs`: the providers.
- `src/maps/tiles.rs`: fetching on the worker thread, a disk cache under the data folder that
  honours cache headers (7-day floor), and a texture LRU.
- `src/maps/widget.rs`: the slippy map widget.
- `src/maps/picker.rs`: the dialog.
- Changes elsewhere:
  - `src/model/calendar.rs`, `task.rs`: the `place` field.
  - `src/model/data.rs`: migration.
  - `src/model/prefs.rs`: the new prefs.
  - `src/ics.rs`: `GEO` and `X-APPLE-STRUCTURED-LOCATION` read and write.
  - `src/sync/mod.rs`, `graph.rs`, `proton.rs`: use `Place` in place of `place_and_notes`.
  - `src/dialogs.rs`, `src/settings.rs`, `src/strings.rs`.

## 6. Plan

Each phase leaves the app working and can ship alone.

1. **Place in the model and in sync, with no map yet.**
   - `Place` on `Event` and `Task`, and migration from `Where:`.
   - `ics.rs` reads and writes `GEO` and `X-APPLE-STRUCTURED-LOCATION`.
   - Graph `address`, `coordinates` and `locationUri`.
   - When the text changes and no new coordinates come with it, **remove** stale `GEO` and
     structured-location lines from `raw`.
   - Tests: round-trip each format; migrate an old data file; a stale `GEO` is dropped.
2. **Open in links.**
   - Build Google, Apple, OSM and `geo:` links from text and coordinates.
   - An "Open in" button in the editors.
   - An "Open places in" setting.
   - No network needed, so it is useful at once.
3. **Tiles and map widget.**
   - Worker fetch, disk cache, User-Agent, attribution.
   - Pan, zoom and pin. Tested by hand on all three systems.
4. **Picker dialog with OSM and Photon.**
   - Debounced search, reverse lookup on pin drop, keyboard use.
   - Hooked into the event editor, the flyout and the task editor.
5. **Google Maps provider.**
   - Read the Maps Platform terms first.
   - API key in the keyring, Places Autocomplete (New) with session tokens, Place Details,
     Map Tiles session.
   - Follow the caching rules for stored coordinates.
6. **Settings.**
   - A Maps page.
   - Per-calendar provider and Open in in `page_calendars`.
   - Google-account calendars start out preferring Google.
   - Search words for the settings search.
7. **Check against the real services.**
   - Make an event in katlendar and check it in Google Calendar (web and phone), Outlook web,
     Apple Calendar through iCloud or Nextcloud, and Proton.
   - Make events in each of those and check that they come back here with coordinates where the
     format allows.
   - **As built in phase 7:**
     - Example events, modelled on what each service sends, are in `src/sync/fixtures/`:
       Google over CalDAV, iCloud from an iPhone, Nextcloud web, Thunderbird, five Outlook places
       (Bing, Teams, none, a meeting room, an address only) and Proton's three decrypted parts.
     - Tests read them and write them back: other apps' lines, reminders and Apple's map handle
       stay; an added spot reads back; Proton keeps the place and spot in the encrypted part.
     - They found one bug: Apple's `X-TITLE` holds only the place's name (the first line of
       `LOCATION`), so an unchanged iPhone place looked changed and lost its map handle. Fixed;
       Apple's line is now also written with the first line only.
     - The live checks need real accounts. The checklist is `docs/map-checks.md`, and
       `src/syncing/live.rs` runs them by machine where an account is set up.
     - Proton, checked live on 2026-11-21: the place, its spot and Apple's line are kept, and
       survive a title change. Google, Outlook and iCloud or Nextcloud are not checked yet.

## 7. Open questions

- **Todoist tasks:** keep the place in katlendar only (proposed), or also append it to the
  description so Todoist shows it?
- **Map drawing:** own widget (proposed) or `walkers` for speed of delivery?
- **Google provider:** worth building in the first round, or wait until someone asks? The OSM
  default plus a "Open in Google Maps" link already covers most Google Calendar users, because
  Google geocodes the text itself.
- Confirm by testing: does Google's CalDAV keep or drop `GEO` and `X-APPLE-STRUCTURED-LOCATION`?
  Does Proton keep them in the encrypted part? (Proton's server does; its web app, when the
  event is edited there, is not checked yet.)
