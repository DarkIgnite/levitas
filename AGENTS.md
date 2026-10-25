# Levitas: notes for AI agents

Start with [docs/HANDOFF.md](docs/HANDOFF.md): the full context, what works, what to do
next and the pitfalls. The project rules are in [CLAUDE.md](CLAUDE.md) and apply to every
agent, not only Claude:

- Reply to the owner (DarkIgnite) in casual Indonesian; code, comments, commits and docs
  in English.
- Follow [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md): no Mojang/Paper/Leaf code; Pumpkin
  (GPL-3.0) only with attribution; SteelMC (AGPL) never copied.
- `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test --workspace` pass before every commit; commit and push each verified change.
- Tick off [docs/ROADMAP.md](docs/ROADMAP.md) items as they land.
