# Comments

`kbmd` comments are flat, chronological, and append-only. A comment cannot reply to another comment, and the format has no parent ID or threading relationship. This keeps concurrent Git changes understandable and makes physical file order the single conversation order.

## CLI

Add Markdown directly, from a file, or from standard input:

```sh
kbmd comment add ACME-12 "Ready for another review." --author "Ada Lovelace"
kbmd comments add ACME-12 --file review.md
printf '%s\n' 'One concern:' '- [ ] Should retries have a limit?' | \
  kbmd comment add ACME-12 --file -
```

`comments` is an alias for `comment`. The positional form is convenient for a short comment; `--file PATH` and `--file -` preserve multiline Markdown. Empty comments are rejected.

List comments in document order or show one by its stable ID:

```sh
kbmd comment list ACME-12
kbmd comment show ACME-12 CMT-0198ef9f-5b52-7a01-b939-54e41a5016b0

kbmd comment add ACME-12 "Ship it." --json
kbmd comment list ACME-12 --json
kbmd comment show ACME-12 CMT-0198ef9f-5b52-7a01-b939-54e41a5016b0 --json
```

There are no comment reply, edit, or delete commands.

## Authors

The author is selected in this order:

1. `--author NAME`
2. `KBMD_AUTHOR`
3. the effective Git `user.name` for the project

If none is configured, adding a comment fails with guidance. An explicit but invalid value also fails instead of falling through to the next source.

The stored name is attribution, not authentication: `kbmd` does not prove who typed it. Review the repository's Git history when authorship matters.

## Markdown format

The first comment creates a `## Comments` section. If the card already has one unique section named `Comments` (case-insensitively), `kbmd` adopts it and preserves its existing prose. Multiple matching sections are ambiguous and must be resolved before adding a comment.

Each managed section contains one registry marker followed by complete comment blocks:

```markdown
## Comments

<!-- kbmd:comments:v1 -->

<!-- kbmd:comment:v1
version: 1
id: CMT-0198ef9f-5b52-7a01-b939-54e41a5016b0
author: Ada Lovelace
created_at: 2026-08-28T02:00:00.000Z
-->
Comment by **Ada Lovelace** · 2026-08-28T02:00:00.000Z

Ready for another review.

<!-- kbmd:comment:end CMT-0198ef9f-5b52-7a01-b939-54e41a5016b0 -->
```

The attribution and body are ordinary, human-visible Markdown. Hidden YAML records the format version, a UUIDv7-based stable ID, author, and UTC creation time. The registry marker gives the section its meaning, so renaming its heading does not disconnect the comments.

Markdown inside a comment is discussion. Headings in a comment do not become card sections, and task-list examples do not affect checklist progress or become actionable checklist items. Generic section and checklist mutations fail when they would damage the managed comments area; use `kbmd comment` commands to add to it.

`kbmd validate` rejects malformed or unsupported markers, invalid metadata, duplicate IDs, incomplete or nested blocks, empty bodies, and comments outside the registry. Marker-looking text inside code, quotes, lists, or inline prose remains ordinary content rather than a control record.

## TUI

Select a card and press `c` to open the single-line comment composer. `Enter` appends the comment, `Esc` cancels, and `Ctrl-U` clears the draft. Mouse input is blocked while the composer is open, so the selected card cannot change underneath the draft. `r` remains the normal reload key outside the composer.

Use the CLI with `--file PATH` or `--file -` when the comment needs multiple lines or richer Markdown.

## Git merges

Comment timestamps are metadata; physical file order is authoritative. Two branches that append at the same tail of one card may conflict. Resolve that conflict by keeping:

- exactly one `<!-- kbmd:comments:v1 -->` registry marker;
- both complete comment blocks, including each start metadata record and matching end marker;
- the blocks in the order you want readers and `kbmd comment list` to see them.

Then run:

```sh
kbmd validate
```

Git history provides the audit trail. `kbmd` does not synthesize threads, reorder comments by timestamp, or silently repair conflicting blocks.
