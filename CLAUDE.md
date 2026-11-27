# Katlendar

A desktop calendar and task planner in Rust and egui.

## Siblings

Most features come in pairs that do one job for events and for tasks. Before changing either half of a pair, open its **sibling** and match it: placement, row order, spacing, label and tooltip wording, and the helper it is built with. A change is finished when a picture of each sibling shows the same layout (see Checks).

Known pairs:

- Settings › Active calendars (`page_active`) ↔ Settings › Task lists (`page_lists`), both topped by `collection_top`
- Default calendar ↔ Default task list
- Event editor (`event_form`) ↔ task editor (`task_form`), both in `src/dialogs.rs`
- The Event tab ↔ the Task tab of the new block window (`dlg_create`)

A new feature that has an events half and a tasks half gets both, built alike, and a line here.

## Settings pages

Build settings rows from the helpers in `src/settings.rs`: `heading`, `row` (a named setting with its tooltip), `combo`, `add_row` (a name field with a "+ Add …" button) and `collection_top` (title, description, default picker, add row). When two pages need the same widget, it becomes a helper there.

## Checks

`cargo test` passes. `settings::tests::calendars_and_task_lists_pages_line_up` draws sibling pages headless and fails when their rows are out of order or at different heights; each new sibling page goes into it.

To look at the UI, take a picture and read the PNG:

```bash
KATLENDAR_SHOW_SETTINGS=lists cargo run -q -- --screenshot /tmp/lists.png
```

A picture uses sample data and saves nothing, so it runs beside the user's open copy. `src/testaid.rs` lists every `KATLENDAR_*` aid (open editors, menus, pages, window size, theme).
