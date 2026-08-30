---
id: DEMO-2
title: Build the keyboard-first board
status: Doing
labels:
- mvp
- tui
assignees:
- simon
ordinal: 1024
created_date: '2026-08-27T02:00:00Z'
updated_date: '2026-08-27T04:15:00Z'
priority: high
component: tui
estimate:
  points: 5
  confidence: 0.7
feature_flags:
  mouse_drag: true
  inline_editor: false
---

## Implementation plan

1. Render ordered status columns from config.
2. Keep navigation and selection in a testable state model.
3. Send every mutation through the shared project store.

### Interaction checklist

- [x] Arrow and `hjkl` navigation
- [x] Quick-add prompt
- [ ] Card drag and drop
- [ ] Click-to-toggle checklists

## Terminal notes

The scanner ignores examples inside fences:

```markdown
## Not a real card section
- [ ] not a live checklist item
```

Mouse support is additive; every core operation needs a keyboard path.
