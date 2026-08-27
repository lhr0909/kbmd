# Flexible demo board

This is a complete tracked `kbmd` project with five columns and three cards that intentionally use different sections, checklist groups, and custom YAML shapes.

From the repository root:

```sh
cargo run -- --project examples/demo validate
cargo run -- --project examples/demo board
cargo run -- --project examples/demo tui
```

The TUI and CLI discover the same [`.kbmd`](.kbmd) directory. Mutations change these tracked example files, so use `git diff` to inspect the exact Markdown write behavior and restore or commit the result when finished.
