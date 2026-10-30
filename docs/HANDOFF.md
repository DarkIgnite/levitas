# Handoff: continuing Levitas

This is the full context for whoever (human or AI agent) picks up Levitas next. Read it
together with [ROADMAP.md](ROADMAP.md), [ARCHITECTURE.md](ARCHITECTURE.md),
[CLEAN_ROOM.md](CLEAN_ROOM.md) and the root `CLAUDE.md`. State as of commit `cab7616`
(2026-10-25).

## 1. What Levitas is and who it is for

- A from-scratch Rust server for **Minecraft: Java Edition 26.3** (protocol **777**),
  license **GPL-3.0-only**, repo <https://github.com/DarkIgnite/levitas> (public).
- **Version change (2026-10-29):** the owner moved the target from 1.21.8 (protocol 772,
  DataVersion 4440) to **26.3**. The code still speaks 1.21.8 until the
  [migration milestone](ROADMAP.md#migration-to-263) is done, and that migration comes
  first. 26.3's DataVersion is whatever its server.jar's `version.json` says; read it
  from there when regenerating the data rather than from memory.
- Owner: **DarkIgnite**. They run a Leaf survival-economy server today and want
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

## Migrating to 26.3

Done (2026-10-30); the checklist is in [ROADMAP.md](ROADMAP.md#migration-to-263): data,
protocol 777, entity data, chunk sections (fluid count), synced registries, loot and
recipe formats, biome spawn lists (now the `minecraft:gameplay/natural_mob_spawns`
attribute), the recipe packet (item stack templates, brewing property sets), the 26.3
world layout with the upgrade of 1.21.8 worlds (`storage::upgrade_layout`), game rules
by their 26.3 names, terrain without fallbacks, and the blocks new in 26.3. The owner
joins and plays with a 26.3 client. Blocks minecraft-data has no rows for (it stops at
26.1) take hardness, light and shapes from `pumpkin-data`, a build dependency of
`levitas-registry`, and tool needs from `tool_stand_in`. 26.3 took grass, podzol,
mycelium and moss out of the `dirt` tag; plants are placed by the `supports_*` tags.
26.3 wiki revisions used: packets `oldid=3868170`, entity metadata `oldid=3792654`, data
types `oldid=3855787`, chunk format `oldid=3661930`. Those pages lag behind 26.3 in two
places a real client caught: light masks in chunk and light packets are now length-
prefixed little-endian byte arrays (not longs), and the synced registries include
`block_transformer`, `decorated_pot_pattern` and `worldgen/block_state_provider`. When
the wiki and the client disagree, log in to a throwaway vanilla server (offline mode,
whitelist off) and compare what it sends with what Levitas sends. What to know going in:

- **Data first.** Point `tools/datagen.py` at 26.3, regenerate `generated/26.3`, and point
  `levitas-registry`'s `build.rs` there. Packet IDs, block states, items, registries,
  tags, sounds, loot tables and recipes all come from that output, so most tables follow
  on their own; the build errors show what moved or was renamed.
- **The protocol changed between 772 and 777.** Check every packet Levitas writes or
  reads against the current minecraft.wiki page `Java Edition protocol/Packets` (it
  documents 777) and keep the revision IDs you used in commit messages. The 1.21.8
  revisions used so far: packets `oldid=3150432`, entity metadata `oldid=3076794`.
- **Entity data moved.** Serializer IDs and field indices differ between versions (the
  villager data disconnect on 1.21.8 came from one off-by-one). The current
  `Java Edition protocol/Entity metadata` page documents 26.3; re-check every
  `SET_ENTITY_DATA` writer (mobs, villagers, golems, boats, minecarts, item frames,
  paintings, armor stands, potions, shields, names) against it, and keep the
  byte-for-byte tests in step.
- **Game rules and saved data changed.** Game rules, `level.dat` (26.3 adds a
  `version_history` list), item components, loot functions (`conditions` became
  `condition`) and entity NBT need checking against 26.3; worlds saved by Levitas on
  1.21.8 must still load and be upgraded on save.
- **New content** (poplar wood, dappled forest, red shrub, shelf mushroom, wool and
  concrete stairs and slabs, straw beds, sulfur and cinnabar, the abandoned camp) arrives
  with the data; Pumpkin's generator places it as it is.
- The owner's world in `C:\dev\levitas-server` was made on 1.21.8; a copy from before
  the move is in `world-backup-1.21.8-20261030-1310` next to it.

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
python tools/datagen.py            # once: downloads the server.jar (VERSION in the script; 1.21.8 until the migration), runs its data generator into generated/<version>
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
| `levitas-registry` | `build.rs` turns `generated/<version>` (1.21.8 until the migration to 26.3) into tables: packet IDs, blocks/states (`block::*` consts, `BlockInfo`, properties, hardness, light emission/filter, requires_tool), items (`ItemInfo`, tools, food, durability), tags, synced registries, collision shapes, sound events, loot tables JSON, recipes JSON. Hardness/shapes/light come from PrismarineJS minecraft-data (MIT, pinned commit) because the data generator lacks them. |
| `levitas-world` | Paletted containers, `Chunk`/`Section` (y -64..320, 24 sections), `light.rs` (`ChunkLight`, 26 light sections incl. one below/above), `lighting.rs` (`LightEngine`: incremental BFS relight), `anvil.rs` (vanilla chunk NBT incl. light, block_ticks, fluid_ticks), `region.rs` (.mca files) |
| `levitas-pumpkin` | `PumpkinTerrain`: drives Pumpkin's generation steps over 8x8-chunk areas and translates results to the target version's states/biomes/light. Every Pumpkin block state and biome matches a 26.3 one by name; it also holds plant placement rules (`Portals::can_place_at`, adapted from Pumpkin with attribution). |
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
- `world/storage.rs` – level.dat and `data/minecraft/`, player data, chunk load/save,
  autosave, unloading, and moving 1.21.8 worlds into the 26.3 layout.
- `world/commands.rs` – `/gamemode`, `/give`, `/kill`, `/time`, `/weather`, and the
  Brigadier tree sent on join for tab completion: **add every new command to
  `command_tree()` too**, or the client shows it as unknown.
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
- Persistence in vanilla 26.3 format (level.dat, players, regions, block entities,
  block/fluid ticks, light).
- **Real terrain** from Pumpkin (biomes, caves, aquifers, trees, ores, structures'
  blocks), generated on worker threads in 8x8-chunk areas.
- **Lighting** updated on every block change and sent as light update packets.
- **Water and lava** flow, infinite water, buckets, obsidian/cobblestone/stone/basalt.
- **Falling blocks** (sand, gravel, concrete powder, anvils, pointed dripstone) with 2-tick
  scheduled ticks, entity gravity/drag, concrete hardening, anvil damage/crushing/degradation,
  and stalactite damage/drops.
- **Heightmaps** in chunk packets (the client needs them for rain, snow and their sounds).
- **Tab completion** for every command.
- **Monsters**: zombies, spiders, creepers and skeletons spawn in the dark, path to
  players, burn in daylight, explode (creepers) or shoot arrows (skeletons); armor
  softens their hits.
- **Bows and experience**: players draw bows and shoot arrows; experience orbs from
  mobs and ores raise levels (saved, dropped on death).
- **Animals**: cows, pigs, sheep and chickens spawn with new chunks, wander, panic,
  can be killed for vanilla loot, sheared, milked, and are saved; `/summon`.
- **Day/night and weather**: time sync every second, rain/thunder cycle with fading,
  `/time`, `/weather`, sleeping skips the night, beds are respawn points.

## 7. What to do next

The owner's standing request is to keep going until the world plays like vanilla. The
open items, with what each still lacks, are in [ROADMAP.md](ROADMAP.md); the progress
table in the README shows where the most is missing. Recently done: the move to 26.3,
the blocks new in 26.3, special crafting recipes (repairing, leather dyeing, transmute,
tipped arrows), status effects on mobs (potions, splash potions, tipped arrows), and
maps, compasses, recovery compasses, lodestone compasses, cartography tables, and spyglass
(Phase 8). Good next candidates for a playable world: lingering potion clouds, beacons,
conduits, advancements (Phase 11), raids (Phase 10), and the Phase 13 plugin API and
economy the owner wants for their server.

## 8. Known gaps and pitfalls

- Pumpkin targets Minecraft 26.x (protocol 777); `levitas-pumpkin` maps by name.
  The test `every_pumpkin_state_has_a_match` fails if a Pumpkin update adds blocks with
  no 26.3 counterpart (map it in `map_state`). Pumpkin is pinned to rev
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
- The minecraft.wiki protocol pages lagged 26.3 in places (light masks, synced
  registries, slot display IDs, item stack templates). When a real client disconnects,
  its report is in the Modrinth profile "Remarkably Optimized" under `debug/`; compare
  with a throwaway vanilla 26.3 server (`generated/server-26.3.jar`, offline mode,
  whitelist off) by logging in to both with the same small probe client.
- 26.3 changed some tags' contents silently (grass is no longer in `dirt`); when a
  tag-based rule misbehaves, check the tag in `generated/26.3/data/minecraft/tags`.
- `tools/deploy.ps1` occasionally fails to build right after the server stops (the exe
  is still locked); run it again.
- Spawn point: Pumpkin's spawn finder plus the highest non-tree block; the player always
  spawns exactly there (no spawn radius).
- Old worlds made before light was stored load with full daylight everywhere.
- Beacons & Conduits (2026-10-30):
  - Beacons (`world/beacon.rs`): mineral pyramids of 1 to 4 layers (`minecraft:beacon_base_blocks`),
    sky clearance checks (`light_filter < 15`), container `menu::BEACON` with transient payment slot
    (`click::Layout::Beacon`), `serverbound::SET_BEACON` (packet ID 53) power selection, beam rendering
    via `BLOCK_ENTITY_DATA`, and 80-tick pulse of Speed, Haste, Resistance, Jump Boost, Strength,
    and Regeneration up to 50 blocks.
  - Conduits (`world/conduit.rs`): 3x3x3 water cage and 16-42 prismarine/sea lantern frame blocks,
    grants Conduit Power to players submerged or in rain, and attacks hostile mobs with 4 magic damage
    every 40 ticks when complete (42 blocks). Prevents drowning in `hazards.rs`.
- Area Effect Clouds, Lingering Potions & Dragon's Breath (2026-10-30):
  - Area Effect Clouds (`world/clouds.rs`, `entity_type::AREA_EFFECT_CLOUD`): spawns when a lingering
    potion impacts (`world/thrown.rs`) or when the ender dragon perches on the exit portal (`world/dragon.rs`).
  - Supports 20-tick initial wait time, 20-tick reapplication delay per entity, shrinking 0.5 blocks per
    application (`radius_on_use`), and radius decay over its 30-second duration.
  - Applies 1/4-duration status effects or scaled instant healing/damage, and deals 3.0 magic damage for dragon's breath.
  - Empty glass bottles collect dragon's breath into `minecraft:dragon_breath` (`world/fluids.rs` `fill_bottle`),
    playing `item.bottle.fill_dragonbreath` and shrinking the cloud by 0.5 blocks.
  - Splash potions emit particle colors matching the potion effects (`potions::color`).
  - Phase 8 is now 100% complete.
- Raids, Bad Omen & Raider Mobs (2026-10-30):
  - Raids (`world/raids.rs`): Drinking `minecraft:ominous_bottle` gives `minecraft:bad_omen` (120,000 ticks). Entering a village converts Bad Omen to `minecraft:raid_omen` (600 ticks). Expiry starts the raid, plays the war horn sound (`event.raid.horn`), and sends a red notched boss bar (`clientbound::BOSS_EVENT`).
  - Waves: Spawns perimeter waves of Pillagers, Vindicators, Ravagers, Witches, and Evokers. Patrol leaders wear ominous banners and drop ominous bottles.
  - Raiders target players, villagers, and iron golems (`monsters.rs`). Iron golems retaliate against raiders.
  - Evokers summon lines of snapping Evoker Fangs (`world/raids.rs` `tick_evoker_fangs`) dealing 6.0 magic damage ignoring armor, and drop Totems of Undying.
  - Victory grants `minecraft:hero_of_the_village` with substantial trading discounts (`special_price` in `containers.rs` and `villagers.rs`). Defeat triggers raider celebration jumping and victory sound.


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
