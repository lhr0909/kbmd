---
id: DEMO-3
title: Prepare the first release
status: Review
labels:
- release
assignees:
- maintainer
ordinal: 1024
created_date: '2026-08-27T03:00:00Z'
updated_date: '2026-08-27T05:30:00Z'
risk: low
release:
  channel: internal
  artifacts:
  - checksums
  - changelog
  - demo recording
reviewers:
- macos
- linux
- windows
---

## Definition of done

- [x] Unit and CLI integration tests pass
- [ ] Mouse flow is checked in a real terminal
- [ ] Fresh-clone instructions are verified

## Release ritual

- [ ] Tag the reviewed commit
- [ ] Publish checksums
- [ ] Announce the known limitations

## Rollback plan

The board is plain text: revert the release commit and rebuild the binary.

## Decision log

The MVP intentionally keeps rich Markdown editing in the user’s editor.
