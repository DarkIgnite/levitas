# Roadmap

The plan: make every vanilla survival feature work, then the plugin API for
survival-economy servers. Real terrain came early (Phase 12, through Pumpkin's generator)
so the owner can play a normal world while the remaining phases land. Each phase ends with
something that can be tested in a real 1.21.8 client.

## Definition of done

A feature is only ticked off when:

- it behaves like vanilla 1.21.8, checked against public documentation (minecraft.wiki),
  the data generator output, and by observing a real server, never by translating
  Mojang, Paper or Leaf code (see [CLEAN_ROOM.md](CLEAN_ROOM.md));
- it has automated tests: unit tests for the rules, and an end-to-end test that drives
  the server through the network protocol like a client would;
- it has been tried in the real game client;
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass.

Known gaps are listed under each phase instead of being hidden.

## Done so far

- [x] Project name, license (GPL-3.0), clean-room policy, Cargo workspace
- [x] Data-generator pipeline: packets, blocks and states, items, tags, synced
      registries, loot tables and recipes generated from the official server.jar
- [x] Handshake, server list status and ping
- [x] Offline-mode login, configuration phase (registries, tags, known packs)
- [x] Flat test world streamed around players as they move
- [x] Shared world: players see each other move, look and swing; chat broadcast
- [x] Paletted chunk storage matching the 1.21.5+ network format
- [x] Breaking and placing blocks, sent to every player nearby
- [x] Survival mode: mining with loot-table drops and correct-tool checks
- [x] Dropped items: physics, pickup, throwing (Q, click outside), death drops
- [x] Health, fall/void/starvation damage, hunger, regeneration, eating
- [x] Death screen and respawn
- [x] Server-side player inventory; clicks validated so items cannot appear from nowhere
- [x] Crafting in the 2x2 grid and crafting tables (shaped, mirrored, shapeless)
- [x] Chests and barrels: shared contents, lid animation, contents drop when broken
- [x] Furnaces: fuel, smelting, progress bars, lit block state
- [x] Commands: /gamemode, /give, /kill

## Phase 1: Persistence

- [x] NBT reader and writer (file and network forms, modified UTF-8)
- [x] Anvil region files: read and write chunks, including oversized external chunks
- [x] Chunk sections, block palettes, biomes and block entities saved in vanilla format
- [x] level.dat: spawn point and world time (game rules come with Phase 11)
- [x] Player data: position, rotation, inventory, equipment, health, hunger, game mode
- [x] Autosave every 5 minutes, save on shutdown (Ctrl-C), unload unused chunks
- [ ] Dropped items and other entities saved (with Phase 4)
- [ ] LZ4-compressed region files (vanilla's optional format)
- [ ] Item components preserved when loading worlds from other servers (with Phase 11)

## Phase 2: Blocks behave correctly

- [x] Placement orientation: facing, axis, slab halves and double slabs, stairs, doors
      (with hinges and double doors), beds, double chests, wall torches/signs/banners/heads,
      buttons, levers, ladders, pistons, observers, hoppers, rods, rails, sign rotation
- [x] Placement rules: blocks that need support (torches, flowers, crops, rails, carpets,
      doors, wall-mounted blocks), popping off with drops when their support goes; no
      placing blocks inside players
- [x] Neighbour shapes: fences, panes, iron bars and walls connect; stairs form corners
- [x] Interactive blocks: doors, trapdoors, fence gates, buttons (springing back), levers,
      with sounds for other players
- [x] Double chests (54 slots), trapped chests
- [ ] Block placement sounds for other players (sound types are not in the data yet)
- [ ] Bells, cocoa, hanging signs, crafters and walls' tall sides: check against vanilla
- [x] Mining speed from block hardness and tool speed, with server validation
- [x] Tool durability and breaking (damage component on the network and on disk)
- [x] Server-side click simulation: shift-click (including crafting many at once),
      drag splitting, double-click collect, number keys, offhand swap, throwing, creative
      cloning; the client is corrected when it predicted something else
- [ ] Blast furnace, smoker, campfire, stonecutter, smithing table
- [ ] Special crafting recipes: dyeing, fireworks, map and book cloning, banners,
      shields, repairing, decorated pots, transmute recipes
- [ ] Signs (editing text), item frames, armor stands, flower pots, jukebox, note blocks

## Phase 3: Fluids, gravity and light

- [ ] Water and lava flowing, sources, waterlogging, buckets
- [ ] Lava and water interaction (cobblestone, stone, obsidian, basalt)
- [ ] Swimming, drowning, fire and lava damage, burning, fire spread
- [ ] Falling blocks: sand, gravel, concrete powder, anvils, pointed dripstone
- [x] Real lighting: sky light and block light, propagated and updated on change
      (`levitas_world::lighting`), sent to players as light updates and saved.
      Known gaps: slabs and stairs let light through like air, and light is not
      re-spread across the border when a neighboring chunk loads.
- [ ] Ice melting, snow layers, frosted ice

## Phase 4: Combat and basic entities

- [ ] Attacking players: damage, attack cooldown, critical hits, knockback, sweeping
- [ ] Armor and armor toughness, shields, damage types and immunities
- [ ] Bows, crossbows, tridents, arrows, snowballs, eggs, ender pearls
- [ ] Status effects (needed by food, potions and mobs)
- [ ] Entity framework: tracking, physics, collisions, saving entities to disk
- [ ] Minecarts, boats, item frames, paintings, armor stands, experience orbs
- [ ] TNT and explosions

## Phase 5: Mobs

- [ ] Natural spawning by light level, biome and time; despawning rules
- [ ] Pathfinding and AI goals
- [ ] Passive mobs: cow, pig, sheep (shearing, dye), chicken, rabbit, horse family,
      wolf, cat, fox, parrot, turtle, fish, squid, axolotl, frog, bee, goat, ...
- [ ] Hostile mobs: zombie variants, skeleton variants, creeper, spider, enderman,
      slime, witch, phantom, drowned, pillager, silverfish, ...
- [ ] Mob drops, experience, equipment, babies, riding
- [ ] Breeding, taming, leashes, name tags

## Phase 6: Farming, nature and time

- [ ] Day/night cycle, weather (rain, thunder, lightning), sleeping in beds
- [ ] Random ticks: crops grow, saplings become trees, grass and mycelium spread
- [ ] Bone meal, composters, farmland and trampling, sugar cane, cactus, bamboo,
      kelp, vines, melons and pumpkins, cocoa, sweet berries, mushrooms
- [ ] Leaf decay

## Phase 7: Redstone

- [ ] Redstone wire, torches, repeaters, comparators, blocks of redstone
- [ ] Buttons, levers, pressure plates, tripwires, daylight sensors, observers,
      target blocks, sculk sensors
- [ ] Pistons and sticky pistons (moving blocks and entities)
- [ ] Hoppers, droppers, dispensers (with their item behaviours), crafters
- [ ] Rails: powered, detector, activator
- [ ] Doors, trapdoors, lamps, TNT, note blocks and bells powered by redstone
- [ ] Update order and timing faithful enough that common farms and machines work

## Phase 8: Progression

- [ ] Experience: orbs, levels, sources
- [ ] Enchanting table and bookshelves, enchantment effects
- [ ] Anvil (repair, rename, combine), grindstone
- [ ] Brewing stand, potions, splash and lingering potions, tipped arrows
- [ ] Beacons, conduits
- [ ] Maps, compasses, recovery compasses, clocks, spyglass

## Phase 9: Dimensions

- [ ] Multiple worlds: Nether and End
- [ ] Nether portals (lighting, linking, travelling)
- [ ] End portals, strongholds' frames, the End and its exit portal
- [ ] Ender dragon fight, end crystals, end gateways
- [ ] Wither

## Phase 10: Villagers and raids

- [ ] Villagers: professions, workstations, trading, restocking, gossip
- [ ] Wandering traders, iron golems, zombie villagers and curing
- [ ] Raids and bad omen

## Phase 11: Everything else

- [ ] Advancements and statistics
- [ ] Brigadier command tree with suggestions; console commands
- [ ] Vanilla commands: /tp, /time, /weather, /gamerule, /give with components,
      /effect, /enchant, /summon, /setblock, /fill, /kill, /clear, /spawnpoint, ...
- [ ] Game rules and difficulty
- [ ] Scoreboards, teams, boss bars, titles
- [ ] Item components on the network (enchantments, names, damage, ...)
- [ ] Online mode (encryption and Mojang session check), compression
- [ ] Server configuration file, whitelist, bans, operators

## Phase 12: World generation

- [x] Noise terrain, biomes and climate, carvers (caves, ravines), aquifers: generated
      by [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin)'s `pumpkin-world` (GPL-3.0)
      through `levitas-pumpkin`, on worker threads, 8x8 chunks at a time so trees and
      other features crossing chunk borders come out whole. New worlds use it by default;
      `LEVITAS_WORLD_TYPE=flat` makes the flat test world, `LEVITAS_SEED` picks the seed.
      Pumpkin targets a newer Minecraft version, so its newer blocks and biomes become
      their closest 1.21.8 relatives (poplar wood as birch, sulfur as sandstone, ...).
- [x] Features: trees, ores, flowers, lakes, ...
- [ ] Structures: villages, strongholds, mineshafts, temples, monuments, ...
      Pumpkin places them, but their chests, spawners and other block entities are not
      created yet, so structure chests are empty.
- [ ] Nether and End generation
- [ ] Loading existing Leaf/vanilla worlds (pre-generated terrain)

## Phase 13: Plugins and economy

- [ ] Plugin API: events, commands, permissions, scheduler, storage, GUIs
- [ ] Native Rust and WASM plugins
- [ ] Economy service; first plugins: permissions, economy, homes/warps/tpa, shops, claims

## Infrastructure

- [ ] CI: fmt, clippy and tests on every push (needs the data generator in CI)
- [ ] Region-based parallel ticking (see ARCHITECTURE.md)
- [ ] Benchmarks against Leaf: memory per player, tick time, chunk throughput
