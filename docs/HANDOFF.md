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
| `levitas-registry` | `build.rs` turns `generated/<version>` (26.3) into tables: packet IDs, blocks/states (`block::*` consts, `BlockInfo`, properties, hardness, light emission/filter, requires_tool), items (`ItemInfo`, tools, food, durability), tags, synced registries, collision shapes, sound events, loot tables JSON, recipes JSON. Hardness/shapes/light come from PrismarineJS minecraft-data (MIT, pinned commit) because the data generator lacks them; blocks newer than its tables, and every block's map colour, come from Pumpkin's `pumpkin-data` (a build dependency). |
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
- `world/advancements.rs` – the advancement tree, criteria triggers (`World::advance`
  with a `Trigger`), the update packet, toasts and announcements, saving progress.
- `world/stats.rs` – statistics: `Stats` per player, `World::stat`, distance
  counting in `move_player`, the award packet, saving.
- `world/farming.rs` – stems and fruit, cocoa, sweet berries, kelp, bamboo, vines,
  mushrooms, composters, trampling (called from `nature.rs`'s random ticks and bone
  meal, and from using blocks).
- `world/commands_extra.rs` – player selectors (`World::targets`) and the commands that
  use them (give, clear, tp, msg, setblock, fill, enchant, ...); `commands.rs` keeps the
  older forms and the Brigadier tree sent for tab completion (add new commands to both).
- `world/tablist.rs` – the server-wide player list (one `TabList` shared by the three
  dimension worlds): add on login, remove on leaving the game, never on changing
  dimension. Cross-dimension messages go through `Event::Announce` (everyone) and
  `Event::Direct` (one player by name).
- `world/maps.rs` – filled maps: grid squares, drawing while held, `map_<id>.dat`, the
  shared map ID counter, cartography locking and zooming.
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
- **Loot numbers**: a `uniform` count in a loot table is a whole number with both ends
  included (`0` to `2` gives 0, 1 or 2), as vanilla counts; until 2026-11-03 it was read
  as a fraction and rounded down, which halved drops (`loot.rs`: `int_number`).
- **Animals**: cows, pigs, sheep and chickens spawn with new chunks, wander, panic,
  can be killed for vanilla loot, sheared, milked, and are saved; `/summon`.
- **Day/night and weather**: time sync every second, rain/thunder cycle with fading,
  `/time`, `/weather`, sleeping skips the night, beds are respawn points.

## 6b. Moving a Leaf or vanilla world to Levitas

Stop the old server, back the world up, and copy its folders next to `levitas.exe`:
`world` (it replaces Levitas's own), and for Paper or Leaf also `world_nether` and
`world_the_end`. On start Levitas moves them into the 26.3 layout (the log says what it
moved), renames blocks and items that were renamed since, and keeps players, inventories,
statistics and advancements. The world must come from 1.20.5 or later (items with
components). Leaf's plugin data (economy, claims, ...) is not read; the Phase 13 plugins
will need their own importers.

## 7. What to do next

The owner's standing request is to keep going until the world plays like vanilla. The
open items, with what each still lacks, are in [ROADMAP.md](ROADMAP.md); the progress
table in the README shows where the most is missing. Recently done: the move to 26.3,
the blocks new in 26.3, special crafting recipes (repairing, leather dyeing, transmute,
tipped arrows), status effects on mobs, maps drawn as players carry them
(`world/maps.rs`), compasses, cartography tables, spyglasses, lingering clouds, beacons,
conduits, raids, wandering traders and zombie villagers. Good next candidates for a
playable world: more advancement triggers (Phase 11), the rest of Phase 5's mobs,
storage and TNT minecarts, and the Phase 13 plugin API and economy the owner wants for
their server.

## 8. Known gaps and pitfalls

- Work done by other agents gets reviewed like any change: on 2026-11-01 a review of
  six commits found a dragon's breath particle missing its 26.3 `power` field (a client
  disconnect), map IDs counted per dimension, and maps that were never drawn. Tests that
  only check a packet is non-empty miss layout errors; check the bytes against the wiki.

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
  registries, slot display IDs, item stack templates, and advancements: their screen
  position now follows the telemetry flag, and icons are item stack templates). When a real client disconnects,
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
- Wandering Traders, Trader Llamas, Zombie Villagers, Curing & Golem Spawning (2026-10-30):
  - Wandering Traders (`world/villagers.rs`, `world/mobs.rs`): Protocol entity ID 145. Spawns naturally every 24,000 ticks near players in Overworld with 48,000-tick despawn countdown. Drinks Invisibility potions at night (`entity.wandering_trader.drink_potion`, `entity.wandering_trader.disappeared`) and milk at sunrise (`entity.wandering_trader.drink_milk`, `entity.wandering_trader.reappeared`). Offers 5 regular trades + 1 special trade for emeralds.
  - Trader Llamas: Protocol entity ID 138. Spawn in pairs/singles accompanying the wandering trader, follow their leader, and spit projectile spit (`minecraft:llama_spit`, ID 80) dealing 1.0 damage at monsters attacking the trader or llama.
  - Zombie Villagers: Protocol entity ID 157. Hostile undead mob burning in daylight and hunting villagers/golems. Spawns naturally (5% chance of zombie spawns) and generated when zombies kill villagers (retaining the victim's profession and trades).
  - Zombie Villager Curing: Feeding a golden apple to a Weakened zombie villager triggers conversion (3600 ticks, Strength I effect, conversion sound `entity.zombie_villager.cure`, shakes metadata index 19). Completes into a cured villager (`entity.zombie_villager.converted`) with a permanent 20-emerald discount (`cured_discount`).
  - Village Iron Golem Spawning: When 3 or more villagers panic (monster within 16 blocks) within a 32-block radius and no golem is present, an Iron Golem automatically spawns to defend the village. Iron golems and monsters actively battle.
  - Phase 10 is now complete.
- Wolves (2026-11-03, `world/wolves.rs`): `MobKind::Wolf`, `Mob.owner/sitting/collar`. Entity data 18 (flags
  0x01 sitting, 0x04 tamed), 19 (owner: optional living entity reference, serializer 13), 21 (collar, VarInt),
  22 (anger, VarLong, -1 when calm) is written by `wolf_data`; the layout comes from the wiki and the
  vanilla probe could NOT be made to show a wolf, so it is unverified against a real client: if the client
  disconnects when a wolf appears, start there. Saved with Owner (int array), Sitting, CollarColor (byte)
  as vanilla does; vanilla keeps anger as `anger_end_time` (a game time), which Levitas does not save yet.
  `rally_wolves` is called from `hit_mob` (owner hits) and `damage_from` (mob hits the owner).


- Rabbits (2026-11-03): `MobKind::Rabbit` in `world/mobs.rs`; no entity data is sent (the default coat is
  brown), so there is no layout to get wrong. Shyness is in `think` (`RABBIT_SHY`).


- Skin layers (2026-11-03): the client's Client Information (configuration and play) is read in
  `connection::client_settings`; `Event::Settings` stores `Player.skin_parts/main_hand` and `player_data` sends
  them as entity data 15 (main hand, Humanoid Arm) and 16 (skin parts, byte) with every player spawn and on change.
  In 26.3 these are the Avatar indices 15/16 (they were 17/18 in 1.21.8).


- Water mobs (2026-11-03): squid, cod, salmon swim in `swim_mob` (`world/mobs.rs`). Natural spawning is thinned to
  one chunk in three because every swimmer sends a movement packet per tick and more of them made
  `generated_world_streams_real_terrain` lose its connection (the 4096-frame outgoing queue overflowed). If
  that test starts failing again, look at movement packet volume first.

- Audit of the work since 2026-10-29 (2026-11-05): 21 auditors were run; 77 candidate bugs came back from 13
  areas, but the session limit stopped the verifiers, so each fix below was checked by hand against the code
  and the 26.3 wiki before it went in (commits c2988d9, 4f30b26, eabf846, a61559b and the one after). Fixed: chunks,
  entity chunks and player files keep what Levitas does not handle (unknown block entities and entities,
  structures, InhabitedTime, EnderItems), unreadable chunks are never saved over, old armour slots, players in the
  Nether/End keep their position, swing_animation and 26.3 ANIMATE ids, sign front/back, creative stack
  validation, mob tracking every third tick, ghost corpses, vanilla zombie villagers and traders, missing 1.21.8
  game rules, raids (rule, Peaceful, despawning, live difficulty), duplicate-UUID tab list, item frame/armor stand
  data, reach checks, advancement recipe entries, renamed statistics, the armor stand swap losing an item.
  The auditors for commands, farming, new mobs, monster variants and loot, entity metadata, persistence, packets
  after the migration and hot paths did not run at all. These 49 findings are still UNVERIFIED and unfixed
  (some may be wrong):
  - Dyed leather armour from 1.20.5-1.21.4 worlds loses its colour on load (old {rgb:N} form of dyed_color not read) (`inventory.rs:285`, data_loss)
  - Tipped arrows are spawned without the Arrow colour metadata, so they show no potion particle trail (`world/arrows.rs:465`, minor)
  - Effect amplifiers 128-255 are reset to 0 on save/load (players and mobs) (`world/effects.rs:458`, data_loss)
  - Tipped-arrow and stray-slowness effects land on mobs even when the hit was dodged or blocked (`world/arrows.rs:288`, wrong_behavior)
  - Lingering water (and other effect-less) potions spawn an area effect cloud that does nothing (`world/thrown.rs:313`, wrong_behavior)
  - Dragon's breath cloud hurts undead mobs instead of healing them (`world/clouds.rs:297`, minor)
  - Bedrock blocks the beacon beam, so beacons under the Nether roof never activate (`world/beacon.rs:115`, wrong_behavior)
  - Conduits loaded from vanilla worlds start out 'active' because vanilla saves no 'active' key (`world/conduit.rs:35`, minor)
  - Beacon and conduit effects are sent as non-ambient (grey HUD border, full particles) (`world/effects.rs:96`, minor)
  - Raids are not saved: a restart mid-raid drops the raid, and raid IDs restart at 1 and collide with saved raiders (`world/mod.rs:752`, data_loss)
  - A second raid can start on a village that already has one, with duplicate boss bars and double waves (`world/raids.rs:384`, wrong_behavior)
  - Evoker fangs never get entity event 4, so clients never show the bite (`world/raids.rs:698`, wrong_behavior)
  - Raiders spawn inside hills when the ground is more than 10 blocks above the raid center, and the wave cannot finish (`world/raids.rs:220`, wrong_behavior)
  - Hero of the Village goes to every player who was ever within 96 blocks, including spectators and idle players (`world/raids.rs:570`, wrong_behavior)
  - Armor stands can be placed inside other entities, so any number can stack in one block (`world/armor_stands.rs:211`, wrong_behavior)
  - Zombie villagers that are being cured, or that came from traded villagers, still despawn like ordinary monsters (`world/monsters.rs:684`, data_loss)
  - Cure discount is a flat 20 items off every trade for every player, so many trades drop to 1 item (emerald farm exploit) (`world/villagers.rs:417`, exploit)
  - Hero of the Village discount uses the wrong formula and also discounts wandering trader trades (`world/villagers.rs:1068`, wrong_behavior)
  - Trader llamas never despawn and their leader link is not saved, so they pile up (`world/villagers.rs:1322`, wrong_behavior)
  - Zombie-killed villagers always convert regardless of difficulty, and lose UUID, name, age and the open trade link (`world/monsters.rs:275`, wrong_behavior)
  - Wandering trader's invisibility is server-side only; clients still see it at night (`world/mobs.rs:2415`, wrong_behavior)
  - Every villager scans every mob in the world every tick for its panic check (`world/villagers.rs:899`, performance)
  - Pistons push straw beds instead of breaking them, leaving lone bed halves (`blocks/piston.rs:92`, wrong_behavior)
  - Short and tall dry grass can no longer be placed on sand or terracotta, and natural ones pop (`blocks/mod.rs:347`, wrong_behavior)
  - Periodic geysers erupt every 10 s for 3 s instead of vanilla's 15-60 s cycle with 1-5 s eruptions (`world/sulfur.rs:22`, wrong_behavior)
  - Sulfur spikes (like dripstone) never update thickness, so stacked spikes all stay 'tip' (`blocks/place.rs:270`, minor)
  - A failed or skipped folder move during upgrade silently hides the old world data for good (`world/storage.rs:375`, data_loss)
  - 1.21.8 maps and idcounts.dat in data/ are not moved, so map IDs restart at 0 and collide with existing map items (`world/storage.rs:83`, data_loss)
  - Map files and the map ID counter use pre-26.1 paths/names, so maps do not carry between vanilla 26.3 and Levitas, and IDs get reused (`world/maps.rs:150`, data_loss)
  - Cartography table turns a single paper into an empty map (Bedrock-only recipe), and shift-click puts paper in the map slot (`click.rs:433`, exploit)
  - Empty map + compass in the cartography table eats the compass and gives back the same empty map (`click.rs:438`, wrong_behavior)
  - Zooming a max-scale or locked map, or locking an already locked map, shows a result and uses up the paper or glass pane (`click.rs:423`, wrong_behavior)
  - Stained glass panes lock maps; vanilla accepts only the plain glass pane (`click.rs:414`, wrong_behavior)
  - A cartography result that is thrown never becomes the new locked or zoomed map (`world/containers.rs:787`, wrong_behavior)
  - Lodestone compasses keep pointing at a lodestone that was broken (`world/nature.rs:333`, wrong_behavior)
  - Each dimension task keeps its own map cache: maps not yet saved are missing in other dimensions and cached copies go stale (`world/maps.rs:221`, wrong_behavior)
  - Maps held in the Nether draw the bedrock roof instead of vanilla's static (`world/maps.rs:324`, minor)
  - Login skin/hand settings are dropped for players who log back in to the Nether or End (`play.rs:55`, wrong_behavior)
  - Skin layers and main hand are never sent to the player themselves (`world/mod.rs:960`, wrong_behavior)
  - Tab list add/remove/mode updates to other dimensions can arrive out of order (`world/travel.rs:209`, minor)
  - Stats file is written non-atomically; a crash mid-write resets all statistics (`world/stats.rs:246`, data_loss)
  - Boat and minecart rides count as walking/sprinting (or swimming) distance (`world/mod.rs:1242`, wrong_behavior)
  - Every jump's air time is counted as flown and fallen distance; climbing down counts as falling (`world/stats.rs:173`, wrong_behavior)
  - Wading or walking in water is counted as swim_one_cm instead of walk_on_water / walk_under_water (`world/stats.rs:158`, wrong_behavior)
  - killed_by credits a mob that hit the player long before an unrelated death (`world/survival.rs:414`, wrong_behavior)
  - Opening a trapped chest counts open_chest instead of trigger_trapped_chest (`world/stats.rs:114`, minor)
  - Crafting-grid preview and grid slots count as owned items for inventory_changed advancements (`world/advancements.rs:731`, wrong_behavior)
  - Advancement chat announcement only reaches players in the same dimension (`world/advancements.rs:795`, wrong_behavior)
  - Advancement file written non-atomically, and an unreadable file silently resets all progress (`world/advancements.rs:881`, data_loss)


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
