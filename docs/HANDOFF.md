# Handoff: continuing Levitas

This is the full context for whoever (human or AI agent) picks up Levitas next. Read it
together with [ROADMAP.md](ROADMAP.md), [ARCHITECTURE.md](ARCHITECTURE.md),
[CLEAN_ROOM.md](CLEAN_ROOM.md) and the root `CLAUDE.md`. State as of commit `cab7616`
(2026-10-25).

## 1. What Levitas is and who it is for

- A from-scratch Rust server for **Minecraft: Java Edition 1.21.8** (protocol **772**,
  DataVersion **4440**), license **GPL-3.0-only**, repo
  <https://github.com/DarkIgnite/levitas> (public).
- Owner: **DarkIgnite**. They run a Leaf 1.21.8 survival-economy server today and want
  Levitas to be much lighter than Leaf while staying vanilla-compatible, then grow a
  plugin/economy layer on top.
- **North star:** the owner wants to *play* on Levitas like a normal, working vanilla
  survival world. Prioritise what makes the world playable over polish. When unsure what
  to do next, pick the gap the owner would notice first in game.

### How to work with the owner

- They speak **Indonesian**: reply in casual Indonesian. Code, comments, commit messages
  and docs are in **English**.
- **Commit and push every verified change without asking** (standing instruction).
- They test in the real game client and report with screenshots ("pohonnya aneh",
  "grass ngambang"). Find the root cause, add a test that fails without the fix, fix,
  redeploy.
- Tick items off in ROADMAP.md as they land; list known gaps there rather than hiding them.

## 2. Hard rules

- **Clean room** (docs/CLEAN_ROOM.md): never copy, translate or paraphrase decompiled
  Mojang code or Paper/Purpur/Leaf/Folia sources. Behaviour comes from minecraft.wiki,
  the data generator output, protocol docs and observing a real server.
- **Pumpkin** (<https://github.com/Pumpkin-MC/Pumpkin>, GPL-3.0) may be used and ported
  from **with attribution**: a header comment naming the source file and "Copyright the
  Pumpkin contributors, GPL-3.0". **SteelMC is AGPL-3.0: read for ideas only, never
  copy.**
- Generated data (`generated/`) never goes into git.
- Never enter credentials on the owner's behalf, never change their security settings
  (Avast exceptions are theirs to add).
- Before a commit: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --workspace` must all pass.
- Avoid `Arc<Mutex<_>>` graphs and per-tick allocation in hot paths.

## 3. Machine and environment (owner's Windows PC)

- Repo: `C:\dev\levitas`. Playable server: `C:\dev\levitas-server` (separate folder:
  `levitas.exe`, `start.bat`, `world\`, plus backups `world-flat\`, `world-cut-trees\`).
- Toolchain: Rust 1.99 (workspace `rust-version = "1.96"`, Pumpkin's minimum), Python 3,
  Java 21+ (only for data generation).
- **Avast** kills and quarantines freshly built `levitas.exe`. Exceptions exist for
  `C:\dev\levitas`; the owner should also add `C:\dev\levitas-server`. If the server
  vanishes without a log line, suspect Avast.
- Windows keeps a running test/exe locked; if a build fails with "access denied", stop
  the old process (or use another `--target-dir`).
- Shell gotchas seen so far: bash heredocs containing backslashes and quotes break Python
  patch scripts. Write patch scripts to a file and run them with `python -I file.py`.

## 4. Build, run, deploy

```sh
python tools/datagen.py            # once: downloads server.jar 1.21.8, runs the data generator into generated/1.21.8
cargo build --release
cargo test --workspace             # ~100 tests, about a minute after the first build
cargo clippy --all-targets -- -D warnings
```

- First build compiles Pumpkin (several minutes). `[profile.dev.package.pumpkin-*]`
  sets `opt-level = 3` so world generation is fast enough in tests.
- **Deploy:** stop the running server first (Ctrl-C in its terminal so it saves), then
  `powershell -ExecutionPolicy Bypass -File tools\deploy.ps1`. It builds into
  `target\deploy` and copies `levitas.exe` to `C:\dev\levitas-server`. Then start it
  there: `cd C:\dev\levitas-server; .\levitas.exe` (or `start.bat`).
- Server listens on `127.0.0.1:25565` (first CLI argument overrides). Env vars:
  `LEVITAS_WORLD` (world folder, default `./world`), `LEVITAS_WORLD_TYPE=flat` (superflat
  test world, otherwise real terrain), `LEVITAS_SEED` (seed for a *new* world).
  The current playable world uses seed `-4254893504593453935`.
- **Always stop with Ctrl-C**: it saves. Killing the process loses up to 5 minutes
  (autosave interval). When a fix changes how terrain generates, the already generated
  chunks keep the old result: rename `world` to a backup and start a fresh one with the
  same seed.

## 5. Code map

Workspace crates in `crates/`:

| Crate | What it holds |
|---|---|
| `levitas-protocol` | VarInt, `Reader`/`Writer` packet codec, `BlockPos`, angles |
| `levitas-nbt` | NBT (file, network, gzip), modified UTF-8 |
| `levitas-registry` | `build.rs` turns `generated/1.21.8` into tables: packet IDs, blocks/states (`block::*` consts, `BlockInfo`, properties, hardness, light emission/filter, requires_tool), items (`ItemInfo`, tools, food, durability), tags, synced registries, collision shapes, sound events, loot tables JSON, recipes JSON. Hardness/shapes/light come from PrismarineJS minecraft-data (MIT, pinned commit) because the data generator lacks them. |
| `levitas-world` | Paletted containers, `Chunk`/`Section` (y -64..320, 24 sections), `light.rs` (`ChunkLight`, 26 light sections incl. one below/above), `lighting.rs` (`LightEngine`: incremental BFS relight), `anvil.rs` (vanilla chunk NBT incl. light, block_ticks, fluid_ticks), `region.rs` (.mca files) |
| `levitas-pumpkin` | `PumpkinTerrain`: drives Pumpkin's generation steps over 8x8-chunk areas and translates results to 1.21.8 states/biomes/light. Contains the Pumpkin→1.21.8 block fallback table (`older_name`) and plant placement rules (`Portals::can_place_at`, adapted from Pumpkin with attribution). |
| `levitas-server` | The binary (`levitas`). See below. |

`levitas-server/src`:

- `main.rs` – startup, env vars, Ctrl-C → `Event::Shutdown`.
- `connection.rs`, `play.rs` – per-connection task: handshake/login/configuration, decode
  serverbound play packets into `world::Event`s.
- `world/mod.rs` – **the world actor**: one task owns all chunks, players, entities;
  connections talk to it through an mpsc channel; outgoing packets are `Arc<[u8]>`
  frames. 20 TPS `tick()`. Chunk streaming (`update_view`, `ensure_chunk`,
  `area_generated`), `set_block` (the single place every block change goes through: it
  also triggers light updates and fluid ticks), light update sending.
- `world/building.rs` – placing/using/breaking, neighbour updates (`update_neighbors`,
  `pop_block`), scheduled block ticks (buttons), sounds.
- `world/fluids.rs` – fluid ticks, water/lava flow, lava hardening, buckets, ray cast.
- `world/survival.rs` – health, hunger, damage, death/respawn, eating, `use_item`.
- `world/containers.rs` – chests (incl. double), furnaces, crafting tables, block entities.
- `world/items.rs` – dropped item entities and pickup.
- `world/storage.rs` – level.dat, playerdata, chunk load/save, autosave, unloading.
- `world/commands.rs` – `/gamemode`, `/give`, `/kill`, ...
- `blocks/` – pure block rules: placement orientation (`place.rs`), support/survival,
  connections (fences, walls, stairs), `fluid.rs` (fluid states and rules).
- `click.rs` – server-side container click simulation (compared with the client).
- `inventory.rs`, `loot.rs`, `recipes.rs`, `rng.rs`.
- `terrain.rs` – `Terrain` trait (`generate`, `area`, `generate_area`, `spawn_point`),
  `GenerationPool` worker threads; `flat.rs` – superflat generator.
- `tests.rs` – end-to-end tests: a real server in-process, a fake client over TCP
  (`join`, `give`, `command`, `use_on`, `use_item`, `block_at`, `slot_update`,
  `stand_looking_down`, ...). New features get one of these plus unit tests.

## 6. What works today

Phase 0–1 and most of Phase 2, plus parts of 3 and 12 (details in ROADMAP.md):

- Login, configuration, multiplayer world, chunk streaming (view distance 8), player
  movement/sync, chat commands.
- Survival: health, hunger, regeneration, fall/void/starvation damage, death and respawn,
  item drops and pickup, loot tables, mining speed with server validation, tool
  durability.
- Inventory and containers with full server-side click simulation; crafting (2x2, table),
  furnaces, chests/double chests/barrels with sounds.
- Block placement rules, orientation, support, connections, doors/trapdoors/gates/
  buttons/levers.
- Persistence in vanilla Anvil format (level.dat, playerdata, regions, block entities,
  block/fluid ticks, light).
- **Real terrain** from Pumpkin (biomes, caves, aquifers, trees, ores, structures'
  blocks), generated on worker threads in 8x8-chunk areas.
- **Lighting** updated on every block change and sent as light update packets.
- **Water and lava** flow, infinite water, buckets, obsidian/cobblestone/stone/basalt.
- **Falling blocks** (sand, gravel, concrete powder, anvils, pointed dripstone) with 2-tick
  scheduled ticks, entity gravity/drag, concrete hardening, anvil damage/crushing/degradation,
  and stalactite damage/drops.
- **Day/night and weather**: time sync every second, rain/thunder cycle with fading,
  `/time`, `/weather`, sleeping skips the night, beds are respawn points.

## 7. What to do next (in this order)

The owner's last request was: keep going until the world is playable like vanilla. The
agreed next steps were **falling blocks (done), day/night (done), then mobs**.

1. **Mobs** (Phase 4–5), the biggest gap for "normal survival": entity framework
   (IDs, tracking per player view, movement packets, physics, collisions, saving
   entities to `entities/` region files), then passive mobs (cow, pig, sheep, chicken
   for food), natural spawning by light/biome/time, simple AI (wander, panic, follow
   food), drops; then hostiles (zombie, skeleton, creeper, spider) with melee and
   pathfinding; player combat (attack damage, cooldown, knockback, armor).
2. **Structure loot** (Phase 12): `ProtoChunk::pending_block_entities` from Pumpkin
   holds chest NBT with `LootTable` keys and spawners; convert them into Levitas block
   entities in `levitas-pumpkin` (return them alongside each chunk) and fill chests from
   loot tables on first open (`loot.rs` already evaluates block loot tables).
3. **Random ticks** (Phase 6): crops, saplings (can reuse Pumpkin tree features?),
   grass spread, leaf decay, ice/snow.
4. Remaining Phase 2: blast furnace, smoker, campfire, stonecutter, smithing table,
   special recipes, signs (editing), flower pots, jukebox, note blocks, placement
   sounds for other players.
5. Then Phases 7–11 (redstone, enchanting/anvil/brewing, Nether/End with Pumpkin's
   generator for those dimensions, villagers, commands/config/online mode) and Phase 13
   (plugin API and economy, the owner's original goal for their server).

## 8. Known gaps and pitfalls

- Pumpkin targets Minecraft 26.x (protocol 777); `levitas-pumpkin` maps by name.
  The test `every_pumpkin_state_has_a_match` fails if a Pumpkin update adds blocks with
  no mapping: extend `older_name`. Pumpkin is pinned to rev
  `cd9e98c966919541fe951d78dfdf8d3401e10112`; updating it may change APIs used in
  `levitas-pumpkin` (`Cache`, `ProtoChunk`, `StagedChunkEnum`, `WorldPortalExt`).
- Pumpkin's own `generate_single_chunk` runs terrain and features only for the middle
  chunk; that is why Levitas drives the steps itself over areas (`STEPS` in
  `levitas-pumpkin`). Don't switch back to it.
- Plant placement during generation depends on `Portals::can_place_at`; returning
  `true` everywhere put grass and mushrooms on water.
- Light: slabs/stairs let light through like air; light is not re-spread across a
  border when a neighbouring chunk loads; the flat world stores full sky light
  everywhere (harmless).
- Fluids: placing a waterloggable block into water does not waterlog it; flowing water
  does not push items; fluids never tick in chunks until something changes nearby.
- No entities besides players and dropped items; no mobs, no day/night, no weather.
- Structure chests are empty (see next step 4).
- Spawn point: Pumpkin's spawn finder plus the highest non-tree block; the player always
  spawns exactly there (no spawn radius).
- Old worlds made before light was stored load with full daylight everywhere.

## 9. Testing pattern

- Unit tests next to the rules (`blocks/*`, `levitas-world`, `levitas-pumpkin`).
- End-to-end tests in `levitas-server/src/tests.rs` against a flat world
  (`server()`), or a generated one (`server_with(dir, WorldKind::Noise)`). The fake
  client reads packets until the one it waits for (`read_until`), so tests must wait for
  specific packets rather than assume ordering (light updates for several chunks arrive
  in arbitrary order).
- When fixing a bug the owner saw, first write a test that fails on the old code (for
  example `trees_are_whole_across_chunk_borders`, `plants_grow_only_on_ground`), check
  it fails, then fix.
