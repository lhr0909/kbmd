# kbmd MVP design

## Outcome

The MVP is a local-first Markdown kanban with two equal interfaces:

1. A scriptable CLI for exact mutations of card metadata, user-defined sections, and checklists.
2. A live terminal board for visual navigation, quick capture, status movement, and checklist completion with keyboard or mouse.

The durable source of truth is `.kbmd`, not TUI state. A user can stop the program, open any card in a text editor, understand it without `kbmd`, commit it, and merge it with normal Git tools.

## Research findings

### Product precedents

[Backlog.md](https://github.com/MrLesk/Backlog.md) demonstrates that project-local Markdown tasks work well as a shared source of truth for humans, agents, a CLI, and a visual board. Its documented strengths include structured review checkpoints, stable JSON output, Git-friendly task files, search, and a browser interface. `kbmd` adopts the durable plain-file premise while deliberately targeting a smaller, terminal-first product with a minimal required card schema. The projects are complementary references; file and command compatibility is not an MVP goal.

Trello’s official model describes [horizontally ordered lists containing movable cards](https://developer.atlassian.com/cloud/trello/guides/rest-api/api-introduction/). Its [checklist documentation](https://support.atlassian.com/trello/docs/adding-checklists-to-cards/) treats a checklist as a named collection inside a card and allows multiple independently useful groups. Those observations lead to two mappings:

- Configured status → board column.
- Markdown heading containing task-list items → named checklist group.

Unlike a database-backed Trello clone, `kbmd` does not introduce a second nested checklist schema. Headings and task-list markers already express the structure readably in Markdown. Non-checklist sections use the same mechanism, so a team can invent `Implementation plan`, `Customer evidence`, `Experiment log`, or anything else without a migration.

### Terminal and watcher stack

[Ratatui leaves event collection to its backend](https://ratatui.rs/concepts/event-handling/), which fits a centralized application loop: render from current state, consume one input or filesystem signal, mutate or reload, then render again. [Crossterm exposes key, mouse, resize, polling, and mouse-capture events](https://docs.rs/crossterm/0.29.0/crossterm/event/index.html) across the target desktop platforms.

[notify](https://docs.rs/notify/8.2.0/notify/) selects a recommended native filesystem watcher per platform, but its own documentation notes that network filesystems may emit no events and that large watches can miss events. The TUI therefore treats events as invalidation hints rather than an authoritative change journal:

- Coalesce bursts for roughly 100–150 ms, then reload the full config and card collection.
- Reconcile from disk roughly every two seconds even when no event arrives.
- Expose `r` as an explicit recovery path.

This is simpler and more correct for a small set of plain files than trying to replay backend-specific create/rename/write sequences into UI state.

### Serialization and safe mutation

[serde-saphyr](https://docs.rs/serde-saphyr/1.1.0/serde_saphyr/) provides Serde-based YAML parsing and serialization. Canonical board fields deserialize into Rust structs; flattened custom fields deserialize into `serde_json::Value`. That type intentionally limits the preservation promise to [JSON’s recursive value model](https://docs.rs/serde_json/1.0.149/serde_json/value/enum.Value.html). The tradeoff is predictable typed fields and stable machine-readable output rather than lossless YAML syntax trees.

[atomicwrites](https://docs.rs/atomicwrites/0.4.4/atomicwrites/) creates or replaces a target through an atomic-file operation. [fs4](https://docs.rs/fs4/1.1.0/fs4/) provides cross-platform file locking. `kbmd` composes them with validation and an optimistic content check; neither library is treated as a substitute for Git or coordination with nonparticipating editors.

## File format

```text
<project>/
└── .kbmd/
    ├── .gitignore       created with lock/temp defaults when absent
    ├── .lock             runtime only
    ├── config.yml
    └── <cards_dir>/
        └── <card>.md     direct children only in the MVP
```

`config.yml` is a versioned strict structure:

- `version`: currently `1`.
- `name`: non-empty display name.
- `cards_dir`: relative path that cannot escape `.kbmd`.
- `id_prefix`: ASCII letters, digits, and underscores; IDs are allocated as `<PREFIX>-<number>`.
- `default_status`: must resolve to a column.
- `columns`: non-empty ordered collection of unique case-insensitive names, with optional `color` and positive `wip_limit`.

Lexical validation rejects absolute or parent-traversing card paths, and project open canonicalizes both `.kbmd` and the configured cards directory to reject a symlink that resolves outside the internal directory. Initialization creates the default `.gitignore` only when it is absent, preserving an existing project-owned ignore file.

Each card is a single YAML-frontmatter Markdown document. The reserved metadata is:

| Field | Purpose |
| --- | --- |
| `id` | Stable human-readable identity; case-insensitively unique |
| `title` | Non-empty card title |
| `status` | One configured column name |
| `labels` | Optional string list |
| `assignees` | Optional string list; singular `assignee` is accepted on read |
| `ordinal` | Optional signed integer used to order a column |
| `created_date` | UTC RFC 3339 timestamp for CLI-created cards |
| `updated_date` | UTC RFC 3339 timestamp touched by mutations |

These are eight canonical serialized fields, of which only `id`, `title`, and `status` are required. The singular `assignee` spelling is additionally reserved as a read alias for `assignees`.

All other top-level keys are flattened custom data. Custom keys cannot case-insensitively collide with a reserved field. They are stored in a `BTreeMap`, making top-level serialization deterministic rather than preserving source key placement. Existing top-level `due_date` values therefore migrate losslessly into custom metadata when a card is rewritten; `kbmd` no longer gives them date validation, scheduling, reminders, or other special behavior.

The body remains an opaque string except during an explicit section or checklist operation. [pulldown-cmark](https://docs.rs/pulldown-cmark/0.13.4/pulldown_cmark/) supplies CommonMark events and source ranges; a narrow line scanner then locates the exact source bytes to mutate. This hybrid avoids treating indented code or fenced examples as live card structure without rendering and reserializing the user’s Markdown. It:

- recognizes `#` through `######` CommonMark ATX headings while excluding code blocks;
- addresses headings case-insensitively and refuses ambiguous duplicates;
- associates an item with its most deeply nested containing heading;
- recognizes unordered (`-`, `*`, `+`) and ordered (`1.`, `1)`) task-list markers with ordinary Markdown spacing;
- changes only the checkbox state byte when toggling an item; and
- replaces the smallest section range needed for section operations.

New sections use `##` because it is a useful card-level default without claiming a fixed template.

## Mutation pipeline

All CLI and TUI writes go through the shared `Project` store:

```text
discover project
      ↓
take .kbmd/.lock
      ↓
reopen config + rescan cards
      ↓
validate IDs, statuses, and WIP limit
      ↓
apply one semantic mutation + touch timestamp
      ↓
confirm the original card bytes still match
      ↓
serialize to a temporary file + atomic replace
```

Creation happens under the same project lock, computes the next numeric ID from current files, refuses an existing destination, and allocates ordinals in steps of 1024. A status change appends the card after the current maximum ordinal in its target column. Large gaps leave room for a future within-column reorder algorithm without changing the version-1 format.

The content check is important because editors do not honor `.kbmd/.lock`: if the file has already changed since it was read, the operation aborts and tells the caller to retry. It narrows the lost-update window but is not a distributed transaction or a substitute for resolving Git conflicts.

## TUI interaction model

The screen has board and detail regions. Layout rectangles are retained for hit-testing mouse coordinates against visible columns, card rows, checklist rows, and scrollable areas.

- Arrow keys and `h`/`j`/`k`/`l` navigate without requiring a mouse.
- `Tab` changes focus, and `Space` toggles a selected detail checklist item.
- `[` and `]` move a card one configured status at a time.
- `n` opens a minimal title prompt in the active column; `Enter` creates and `Esc` cancels.
- `?`, `r`, and `q` provide discoverability, deterministic reload, and exit.
- A card click selects it; a checklist click selects and toggles that item; the wheel navigates or scrolls the pane under the pointer; left-button down/drag/up moves a card to the column under the release point.

A drag changes status only. It does not claim pixel-precise ordering within a target column in the MVP. All TUI mutations call the store and reload from disk; the renderer does not optimistically invent file state.

## Dependency choices

The implementation uses established crates with narrow responsibilities:

| Crate | MVP responsibility |
| --- | --- |
| [clap](https://docs.rs/clap/latest/clap/) | Derived command tree, validation, aliases, and help |
| [anyhow](https://docs.rs/anyhow/latest/anyhow/) | Context-rich application errors |
| [serde](https://serde.rs/) + [serde-saphyr](https://docs.rs/serde-saphyr/1.1.0/serde_saphyr/) | Typed config/card schema and YAML frontmatter |
| [serde_json](https://docs.rs/serde_json/1.0.149/serde_json/) | Dynamic JSON-compatible custom fields and CLI JSON |
| [pulldown-cmark](https://docs.rs/pulldown-cmark/0.13.4/pulldown_cmark/) | CommonMark-aware heading/task validation and source offsets |
| [chrono](https://docs.rs/chrono/latest/chrono/) | UTC RFC 3339 timestamps |
| [atomicwrites](https://docs.rs/atomicwrites/0.4.4/atomicwrites/) | Atomic create and replace operations |
| [fs4](https://docs.rs/fs4/1.1.0/fs4/) | Cross-platform cooperative project lock |
| [ratatui](https://ratatui.rs/) | Immediate-mode terminal layout and rendering |
| [crossterm](https://docs.rs/crossterm/0.29.0/crossterm/) | Cross-platform terminal setup and input events |
| [notify](https://docs.rs/notify/8.2.0/notify/) | Native filesystem invalidation signals |
| [unicode-width](https://docs.rs/unicode-width/latest/unicode_width/) | Terminal-width-aware text handling |

`Cargo.lock` is tracked so builds and CI resolve the reviewed dependency graph.

## MVP scope

Included:

- Project initialization and upward discovery.
- Configurable ordered columns, colors, default status, ID prefix, cards directory, and WIP limits.
- Create/list/show/edit/move/validate/board commands.
- Typed arbitrary frontmatter set/get/list/unset.
- Arbitrary section list/show/set/append/remove.
- Checklist list/add/toggle/check/uncheck/remove in any section, plus document-global mutations for preamble and TUI items.
- Versioned JSON output for core card and board commands.
- Keyboard- and mouse-driven TUI with quick add, status drops, and checklist toggles.
- Live reload, periodic reconciliation, cooperative locking, optimistic conflict detection, and atomic replacement.
- Unit, integration, and cross-platform CI coverage.

Explicit non-goals for version 1:

- A fixed task template or reserved plan/notes/acceptance-criteria sections.
- Lossless YAML concrete-syntax preservation.
- Full CommonMark parsing, Setext-heading section mutation, or a Markdown editor inside the TUI.
- Arbitrary within-column drag ordering.
- Web/browser UI, accounts, hosted sync, remote collaboration protocol, or mobile client.
- Search indexing, comments/activity history, attachments, dependencies, recurring work, or archive lifecycle.
- Automatic Git commits, cross-branch aggregation, merging, fetching, or pushing.
- Backlog.md format, CLI, browser, or workflow compatibility and migration.

## Verification strategy

Automated coverage is layered:

- Frontmatter tests cover delimiters, CRLF input, nested custom values, malformed documents, and semantic round trips.
- Markdown tests cover nested headings, fenced and indented-code exclusion, neighboring-section preservation, independent checklists, duplicate headings, unsupported control bytes, and single-byte toggles.
- Config/store tests cover discovery, contained paths, case-insensitive uniqueness, IDs, WIP limits, ordering, atomic updates, unknown fields, concurrent creation, and malformed collections.
- CLI integration tests execute real binaries in temporary projects and assert human output, JSON envelopes, stdin bodies, flexible sections/checklists, fields, and validation failures.
- TUI state tests should exercise navigation, focus, prompt editing, hit-testing, card moves, checklist selection, and reload-state repair without requiring an interactive terminal.
- GitHub Actions runs formatting, Clippy with warnings denied, and all targets/features tests on Ubuntu, macOS, and Windows.

Run the full local gate:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

Manual acceptance checks for a real terminal:

1. Open `examples/demo` with `kbmd --project examples/demo tui` and verify all five columns and three varied cards render.
2. Use arrows and `h`/`j`/`k`/`l`, then click cards, scroll each pane, and switch focus with `Tab`.
3. Toggle a checklist by click and by `Space`; inspect the Markdown diff to confirm only its marker and timestamp/frontmatter normalization changed.
4. Move a card with `[`/`]`, then drag it to another column and confirm `status` changed on disk.
5. Press `n`, test both `Esc` cancellation and `Enter` creation, and confirm the new card uses the active status.
6. While the TUI is running, edit a title, section, and custom field on disk. Confirm the view refreshes automatically; test `r` as a forced reload.
7. Open `?`, resize the terminal, quit with `q`, and confirm raw mode, mouse capture, and the alternate screen are restored.
