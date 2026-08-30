# kbmd

`kbmd` is a local-first kanban for people who want Trello-like cards without moving project state out of Git. Each card is a YAML-frontmatter Markdown file: a small set of fields powers the board, while the rest of the frontmatter and every Markdown section belong to you.

Backlog.md established a compelling Markdown-native workflow for humans and coding agents. `kbmd` keeps that plain-file foundation and explores a different interaction model: a small Rust CLI, a live mouse-aware terminal board, arbitrary custom fields, and checklists under any heading. It is an early MVP, not a drop-in Backlog.md replacement.

## What is flexible?

- Columns are an ordered list in `.kbmd/config.yml`, with optional colors and work-in-progress limits.
- Only `id`, `title`, and `status` are required on a card. Labels, assignees, ordering, and timestamps are optional canonical fields.
- Any other JSON-compatible YAML value is a custom field: strings, numbers, booleans, lists, or nested maps.
- Any ATX heading can be a section. `## Implementation plan`, `## Customer evidence`, and `### Rollout` are conventions you choose, not slots in a fixed template.
- Any Markdown task list outside a code block is a checklist. A card can have as many independently named checklist sections as it needs.
- The CLI and TUI share the same model and write the same files.

See the tracked [demo board](examples/demo/.kbmd) for three differently shaped cards.

## Install

The MVP currently builds from source and requires Rust 1.88 or newer.

```sh
git clone git@github.com:lhr0909/kbmd.git
cd kbmd
cargo install --path . --locked
```

For a repository-local build instead:

```sh
cargo build --release --locked
./target/release/kbmd --help
```

No account, database, daemon, or hosted service is required.

## Quick start

From the project you want to manage:

```sh
kbmd init "Acme launch" --prefix ACME \
  --statuses "Ideas,Ready,Doing,Review,Done"

kbmd add "Prototype the import flow" \
  --status Ready \
  --label mvp \
  --field priority=high \
  --field-yaml 'estimate={points: 3, confidence: 0.7}' \
  --section 'Context=Users need to preview mappings before import.' \
  --check 'Implementation plan=Parse a sample file' \
  --check 'Implementation plan=Render the mapping preview'

kbmd board
kbmd tui
```

Running `kbmd` with no subcommand also opens the TUI. Commands discover the nearest `.kbmd/config.yml` by walking up from the current directory, or you can target a project explicitly with `kbmd --project PATH …`.

## CLI workflows

### Create, inspect, edit, and move cards

```sh
kbmd add "Document the release" --status Ready --assignee simon
kbmd list
kbmd list --status Doing --label mvp
kbmd show ACME-1
kbmd show ACME-1 --raw
kbmd edit ACME-1 --add-label docs --remove-assignee simon
kbmd move ACME-1 Review
kbmd validate
```

Card IDs and supplied status names are matched case-insensitively; the configured spelling is written back to disk. `kbmd validate` checks config, parseability, required fields, status membership, WIP limits, and duplicate IDs. Use `--json` on `add`, `list`, `show`, `edit`, `move`, and `board` for a versioned JSON envelope suitable for scripts.

A complete body can come from a file or standard input:

```sh
kbmd add "Investigate latency" --body-file investigation.md
printf '## Notes\n\nCaptured from a script.\n' | \
  kbmd add "Piped card" --body-file -
```

### Arbitrary frontmatter fields

Values are strings by default. Pass `--yaml` (or `--field-yaml` during creation) for a typed YAML value.

```sh
kbmd field set ACME-1 owner-team platform
kbmd field set ACME-1 estimate '{points: 5, confidence: medium}' --yaml
kbmd field set ACME-1 blocked-by '[ACME-7, ACME-9]' --yaml
kbmd field list ACME-1
kbmd field get ACME-1 estimate
kbmd field unset ACME-1 blocked-by
```

Custom fields may not reuse reserved names: `id`, `title`, `status`, `labels`, `assignee`, `assignees`, `ordinal`, `created_date`, or `updated_date`.

### Arbitrary Markdown sections

Section names are case-insensitive when addressed by the CLI and must be unique within a card.

```sh
kbmd section list ACME-1
kbmd section set ACME-1 "Implementation plan" --file plan.md
kbmd section append ACME-1 Notes "Measure p95 after rollout."
printf '%s\n' '1. Enable for the pilot team.' '2. Watch error rate.' | \
  kbmd section set ACME-1 Rollout --file -
kbmd section show ACME-1 Rollout
kbmd section remove ACME-1 "Old experiment"
```

New sections are created as level-two headings. Existing `#` through `######` ATX headings retain their level.

### Checklists in any section

Checklist indexes are one-based within their section. `check list` also prints a one-based global document-order index, which is useful for preamble items or scripts. The item text and surrounding Markdown remain ordinary text you can edit directly.

```sh
kbmd check add ACME-1 "Implementation plan" "Add a migration test"
kbmd check add ACME-1 "Release ritual" "Notify support"
kbmd check list ACME-1
kbmd check list ACME-1 --section "Release ritual"
kbmd check toggle ACME-1 "Implementation plan" 1
kbmd check check ACME-1 "Release ritual" 1
kbmd check uncheck ACME-1 "Release ritual" 1
kbmd check remove ACME-1 "Implementation plan" 1

# Use the first index printed by `check list`, regardless of section:
kbmd check toggle-global ACME-1 3
kbmd check check-global ACME-1 3
kbmd check uncheck-global ACME-1 3
kbmd check remove-global ACME-1 3
```

## Live TUI

Launch it with `kbmd` or `kbmd tui`. Wide terminals show the board and selected-card detail side by side; compact terminals use `Tab` to switch between the two panes.

| Input | Action |
| --- | --- |
| Arrow keys or `h` `j` `k` `l` | Navigate columns, cards, and the focused checklist |
| `Tab` | Switch focus between the board and detail checklist |
| `Enter` | Focus the selected card’s detail pane |
| `Space` | Toggle the selected checklist item in detail focus |
| `PageUp` / `PageDown` | Scroll the detail pane |
| `[` / `]` | Move the selected card one status left / right |
| `n` | Quick-add a title in the active column; `Enter` saves, `Esc` cancels |
| `r` | Reload the project from disk |
| `?` | Toggle the in-app help |
| `q` | Quit |
| Mouse click | Select a card, or select and toggle a checklist row |
| Mouse wheel | Scroll the region under the pointer |
| Left-button down, drag, release | Drop a card into the target status column |

The TUI watches `.kbmd` and coalesces filesystem events before reloading. It also reconciles from disk periodically, so edits made in an editor or another `kbmd` process appear without restarting. Filesystem notifications can be unreliable on some network-mounted filesystems; press `r` for an immediate refresh if an edit is not visible.

The MVP’s rich editing path remains the CLI or your Markdown editor. The TUI currently focuses on capture, navigation, status changes, and checklist completion.

## On-disk format

`kbmd init` creates this layout:

```text
your-project/
└── .kbmd/
    ├── .gitignore       # created if absent; defaults ignore lock/temp files
    ├── config.yml       # ordered board definition
    └── cards/           # one non-recursive Markdown file per card
        ├── ACME-1.md
        └── ACME-2.md
```

The cards directory is configurable, but it must be a relative path contained inside `.kbmd`.

### Config

```yaml
version: 1
name: Acme launch
cards_dir: cards
id_prefix: ACME
default_status: Ideas
columns:
- name: Ideas
  color: gray
- name: Ready
  color: blue
- name: Doing
  color: yellow
  wip_limit: 3
- name: Review
  color: magenta
  wip_limit: 2
- name: Done
  color: green
```

Moving or creating a card in a full WIP-limited column fails without changing the file.
Column colors may be a terminal color name such as `blue` or `magenta`, or a quoted `'#RRGGBB'` value; an unsupported value falls back to the default active-column highlight.

### Card

```markdown
---
id: ACME-12
title: Make imports observable
status: Doing
labels:
- mvp
- telemetry
assignees:
- simon
ordinal: 2048
created_date: '2026-08-27T09:00:00Z'
updated_date: '2026-08-27T10:15:00Z'
priority: high
estimate:
  points: 5
  confidence: medium
stakeholders:
- support
- data
---

## Why this matters

An operator should be able to explain a failed import without reproducing it.

## Implementation plan

- [x] Define structured events
- [ ] Add correlation IDs
- [ ] Document the troubleshooting query

### Rollout

- [ ] Enable for the pilot workspace
- [ ] Compare error rates for 24 hours

## Notes

The section names and nesting are project conventions, not `kbmd` schema.
```

`ordinal` controls ordering within a column. New cards and cards moved to a new status are appended after the largest ordinal there.

The canonical card schema has eight fields: `id`, `title`, `status`, `labels`, `assignees`, `ordinal`, `created_date`, and `updated_date`; only the first three are required. `assignee` is also reserved as a legacy read alias for `assignees`. Existing cards with a top-level `due_date` keep that value as ordinary custom metadata when rewritten, so removing the canonical field does not discard project data. `kbmd` no longer gives it date validation, scheduling, reminders, or other special behavior.

## Git and concurrent writers

Track `.kbmd/config.yml`, `.kbmd/.gitignore`, and the card Markdown files. In a fresh project, `.kbmd/.gitignore` excludes the runtime `.lock` and `*.tmp` files. If that ignore file already exists, initialization preserves it untouched; add those two patterns yourself if needed. One card per file keeps most board changes in independent Git diffs, although two people editing the same card can still produce an ordinary merge conflict.

Every `kbmd` mutation takes a project lock, reopens the config, re-reads the current cards, validates the collection, and writes through an atomic replacement. Before replacing an existing card, it checks that the source it read has not already changed and asks you to retry instead of knowingly overwriting that change. The lock coordinates `kbmd` processes on the same filesystem; editors and other tools do not participate in it, so Git review and normal conflict resolution still matter.

`kbmd` does not commit, merge, push, or fetch for you.

## Current limitations

- This is format version 1 of an early MVP; keep the board in Git and review upgrades.
- Unknown frontmatter is preserved semantically only when it fits JSON’s value model: null, booleans, numbers, strings, arrays, and string-keyed maps. YAML tags, anchors, aliases, and non-string mapping keys are not a supported preservation contract.
- Rewriting a card normalizes frontmatter formatting, key order, quoting, delimiters, and comments. Custom values survive semantically, but YAML presentation and comments do not. The Markdown body is kept separately and unrelated sections are left alone by targeted mutations.
- Section and checklist tools recognize CommonMark ATX headings (`# Heading`) and task-list markers outside code blocks. Setext headings are not sections for CLI purposes. Duplicate case-insensitive section names fail closed.
- The TUI does not yet provide full Markdown or custom-field editing, and status drops do not implement free ordering within a column.
- There is no web UI, cloud sync, multi-device service, search index, attachments store, authentication, or Git automation.
- Backlog.md files are not fully compatible, and there is no importer in this MVP.

## Develop and test

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

CI runs the same checks on Ubuntu, macOS, and Windows. For the product and storage rationale, dependency research, MVP boundaries, and manual TUI acceptance checks, see [docs/design.md](docs/design.md).
