# Levitas: notes for Claude

Levitas is a from-scratch Rust server for Minecraft: Java Edition **26.3** (protocol 777),
GPL-3.0-only. The owner (DarkIgnite) runs a Leaf survival-economy server today and wants
Levitas to be much lighter while staying vanilla-compatible. They speak Indonesian; reply
in casual Indonesian. Code, comments, commits and docs are in English.

**Target version: 26.3.** On 2026-10-29 the owner moved the target from 1.21.8 to 26.3,
and the migration was finished on 2026-10-30: the server speaks 26.3 and saves worlds
as 26.3 does. Check packet and entity data layouts against the minecraft.wiki protocol
pages for 26.3 (the current revisions), and against a real vanilla 26.3 server where the
wiki is behind (see docs/HANDOFF.md).

## Before writing code

- docs/HANDOFF.md has the full current state, next steps and pitfalls.
- Read docs/ROADMAP.md to see the current milestone, and docs/ARCHITECTURE.md for design rules.
- Follow docs/CLEAN_ROOM.md strictly. Never copy or translate decompiled Mojang code or
  Paper/Leaf patched sources. Generated data never goes into git.

## Goal and workflow

- North star: the owner wants to play on Levitas like a normal, working vanilla world.
  Prioritise reaching a playable world over polish.
- Repo: github.com/DarkIgnite/levitas (public). Commit and push every verified change
  without asking.
- Game data comes from `python tools/datagen.py` (downloads the official server.jar,
  runs its data generator into `generated/`). `levitas-registry`'s build script needs it.
- The playable server lives apart from the code, in `C:\dev\levitas-server` (levitas.exe,
  start.bat, world/). `tools/deploy.ps1` builds a release and installs it there; stop the
  server first. Stop it with Ctrl-C so it saves; killing it loses up to 5 minutes.
- Pumpkin (github.com/Pumpkin-MC/Pumpkin, GPL-3.0) may be ported from, with attribution
  (see docs/CLEAN_ROOM.md). SteelMC is AGPL-3.0: read for ideas only, never copy.
- Avast on the owner's PC kills and quarantines freshly built `levitas.exe`. If the
  server disappears with no log, that is the likely cause; they need an Avast exception
  for C:\dev\levitas.

## Conventions

- Workspace crates live in `crates/`, named `levitas-*`.
- Avoid `Arc<Mutex<_>>` graphs and per-tick allocation in hot paths (see ARCHITECTURE.md).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- Tick off ROADMAP.md items as they land.
