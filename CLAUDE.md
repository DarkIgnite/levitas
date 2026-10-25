# Levitas: notes for Claude

Levitas is a from-scratch Rust server for Minecraft: Java Edition 1.21.8 (protocol 772),
GPL-3.0-only. The owner (DarkIgnite) runs a Leaf 1.21.8 survival-economy server today and
wants Levitas to be much lighter while staying vanilla-compatible. They speak Indonesian;
reply in casual Indonesian. Code, comments, commits and docs are in English.

## Before writing code

- Read docs/ROADMAP.md to see the current milestone, and docs/ARCHITECTURE.md for design rules.
- Follow docs/CLEAN_ROOM.md strictly. Never copy or translate decompiled Mojang code or
  Paper/Leaf patched sources. Generated data never goes into git.

## Conventions

- Workspace crates live in `crates/`, named `levitas-*`.
- Avoid `Arc<Mutex<_>>` graphs and per-tick allocation in hot paths (see ARCHITECTURE.md).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- Tick off ROADMAP.md items as they land.
