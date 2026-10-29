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
- [ ] Dropped items and other entities saved (with Phase 4). Dropped items are saved
      with mobs in `entities/` region files; arrows, orbs and falling blocks are not.
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
- [x] Blast furnace and smoker: their own recipe lists, cooking twice as fast and
      burning fuel twice as fast
- [x] Campfire (`world/campfire.rs`: four foods cook in 30 seconds, shown on top;
      burns players standing on it; shovels put it out) and stonecutter (recipes sent
      to clients in the recipe update packet)
- [x] Smithing table. Smithing recipes come from the data (`recipes.rs`): netherite
      upgrades keep the item's enchantments and wear; armor trims take their material
      from the addition (`minecraft:trim` on the stack, saved and sent to clients).
      Template, base and addition are each used up once.
- [ ] Special crafting recipes: dyeing, fireworks, map and book cloning, banners,
      shields, repairing, decorated pots, transmute recipes
- [ ] Signs (editing text), item frames, armor stands, flower pots, jukebox, note blocks.
      Signs and hanging signs work (`world/signs.rs`): placing one opens its editor,
      either side can be edited again by right-clicking it, dyes colour a side's text,
      glow ink sacs make it glow and ink sacs undo that, honeycomb waxes it for good.
      Text is saved in vanilla's format (older worlds' JSON lines are read too). Note
      blocks work; item frames, armor stands, flower pots and jukeboxes do not yet.

## Phase 3: Fluids, gravity and light

- [x] Water and lava flowing, sources, waterlogging, buckets. Flow follows the wiki:
      levels, falling columns, infinite water sources, the nearest way down within 4
      (water) or 2 (lava) blocks; plants and torches wash away. Fluid ticks are saved.
      Water breaks falls. Known gaps: placing a waterloggable block into water does
      not waterlog it, and flowing water does not push dropped items.
- [x] Lava and water interaction (cobblestone, stone, obsidian, basalt)
- [x] Drowning, fire and lava damage, burning: 300 ticks of air, 2 drowning damage a
      second; lava 4 and fire 1 (soul fire 2) every half second, setting players alight;
      magma hurts unless sneaking; burning hurts once a second until water or rain puts it
      out. Armor follows the `bypasses_armor` tag. Mobs burn in lava; items burn up.
- [x] Fire (`world/fire.rs`): flint and steel lights fire, TNT, campfires and candles;
      fire ages, goes out without fuel or in the rain, burns flammable blocks away by
      the wiki's odds and spreads to nearby air; netherrack and magma burn forever.
      Lava does not set things alight yet.
- [ ] Swimming physics
- [x] Falling blocks: sand, gravel, concrete powder, anvils, pointed dripstone.
      Gravity blocks schedule a 2-tick block tick on update, spawn a falling block entity
      (type 49, with state ID), simulate gravity and drag, and land or shatter into items
      (e.g. on torches). Concrete powder hardens into concrete touching water (vertically,
      horizontally, or falling through water). Anvils squash entities, deal falling anvil
      damage, crush fragile non-solid blocks, and degrade or break. Pointed dripstone
      stalactites fall when ungrounded from ceiling, deal stalactite damage, and drop items;
      stalagmites pop off without floor and double player fall damage.
- [x] Real lighting: sky light and block light, propagated and updated on change
      (`levitas_world::lighting`), sent to players as light updates and saved.
      Known gaps: slabs and stairs let light through like air, and light is not
      re-spread across the border when a neighboring chunk loads.
- [ ] Ice melting, snow layers, frosted ice

## Phase 4: Combat and basic entities

- [x] Attacking: damage, attack cooldown, critical hits, knockback, sweeping
      (`world/combat.rs`). Mobs and players are hit the same way: held item's attack
      damage and speed from the data generator, charge, critical hits, sprint knockback,
      sweeping sword blows, weapon wear, attack sounds. Players get vanilla's short
      invulnerability after a hit.
- [ ] Armor and armor toughness, shields, damage types and immunities. Worn armor
      reduces mob and explosion damage by the wiki's formula and wears down;
      protection enchantments work. Shields (`world/shields.rs`) block hits, arrows,
      fireballs and explosions from the front once raised for 5 ticks, wear down on
      hits of 3 or more, and are knocked down for 5 seconds by an axe; other players
      see them raised. Missing: blocked arrows bouncing off, attackers knocked back.
- [ ] Bows, crossbows, tridents, arrows, snowballs, eggs, ender pearls. Bows and arrows
      work (`world/arrows.rs`): draw strength, gravity and drag, damage from speed,
      critical shots, sticking in blocks, pickup. Ender pearls, snowballs and eggs are
      thrown (`world/thrown.rs`): pearls teleport their thrower for 5 damage with a
      one-second cooldown, snowballs hurt blazes, eggs hatch chicks. Missing:
      crossbows, tridents.
- [x] Status effects on players (`world/effects.rs`): given by food (golden apples,
      rotten flesh, pufferfish, ...) and `/effect`, cleared by milk and death, removed
      by honey, shown to the client and saved. Regeneration, poison, wither, hunger,
      saturation, instant health and damage, absorption, resistance, fire resistance,
      water breathing, strength, weakness, haste, mining fatigue, jump boost and slow
      falling act on the server; the rest are carried out by the client. Food leaves
      its bowl, bucket or bottle behind. Missing: effects on mobs, health boost,
      chorus fruit teleporting.
- [ ] Entity framework: tracking, physics, collisions, saving entities to disk.
      Mobs (`world/mobs.rs`) are shown to players who have their chunk loaded, move
      with gravity and block collisions, and are saved in vanilla `entities/` region
      files. Items and falling blocks are not tracked per player yet.
- [ ] Minecarts, boats, item frames, paintings, armor stands, experience orbs
- [x] TNT and explosions. Explosions (`world/explosion.rs`) cast rays like vanilla,
      break blocks by blast resistance (dropping them 1 in `power` times, containers
      spilling), and damage and push entities; creepers use them. TNT (`world/tnt.rs`)
      is lit by flint and steel, fire or other explosions (on a short fuse), and falls
      and bounces until it goes off. Redstone does not light it yet (Phase 7).

## Phase 5: Mobs

- [ ] Natural spawning by light level, biome and time; despawning rules. Animals spawn
      once when a chunk is generated. Monsters spawn in groups from the biome's list
      where block light is 0 and the night-dimmed sky light is low, 24+ blocks from
      players, up to 70 per player, and despawn beyond 128 blocks (sometimes beyond 32).
      Missing: animals respawning later, structure-specific spawns, spawners.
- [ ] Pathfinding and AI goals. A* over walkable blocks (`world/pathfinding.rs`):
      walking, one-block jumps, drops of up to three blocks, avoiding lava; monsters
      (except climbing spiders) follow paths to their target. Animals still wander
      straight, and there are no other AI goals (doors, avoiding water, ...).
- [ ] Passive mobs: cow, pig, sheep (shearing, dye), chicken, rabbit, horse family,
      wolf, cat, fox, parrot, turtle, fish, squid, axolotl, frog, bee, goat, ...
      Done: cows (milking), pigs, sheep (natural wool colours, shearing, regrowing wool
      by eating grass), chickens (eggs, slow falling). They wander, panic when hit,
      swim, look at players and make their sounds. Missing: breeding, babies, temptation
      by food, dyeing, biome variants (all are temperate), and every other animal.
- [ ] Hostile mobs: zombie variants, skeleton variants, creeper, spider, enderman,
      slime, witch, phantom, drowned, pillager, silverfish, ...
      Done: zombies (burn in sunlight), spiders (climb walls, neutral in daylight),
      creepers (hiss and explode). They chase the nearest survival player straight on
      (no pathfinding yet), hit with knockback, and keep players from sleeping nearby.
      Skeletons keep their distance and shoot arrows when they can see their target.
      Nether: zombified piglins (neutral until hit, then their whole group within 20
      blocks hunts the attacker for 20 to 39 seconds; golden swords) and magma cubes
      (hop towards players, three sizes, split when they die, no fall damage); both
      ignore fire and lava and spawn by the Nether biomes' lists regardless of light.
      Ghasts and blazes fly (`world/fireballs.rs` for their fireballs): ghasts charge
      for a second when they see a player and spit large fireballs (power 1 explosions
      with fire) that players can hit back; blazes glow, then throw three small
      fireballs that set things alight, and hit in melee up close. Ghasts spawn in the
      Nether one time in twenty; blazes come from fortress spawners. Endermen stay
      calm until a player looks them in the eyes (a carved pumpkin hides that) or
      hits them, then hunt, now and then teleporting closer; they dodge arrows, thrown
      things and fireballs by teleporting, and teleport away from water and rain.
      Monster spawners (`world/spawners.rs`) in generated chunks spawn blazes in the
      Nether and zombies, skeletons or spiders in the overworld, with players near.
      Missing: baby/husk/drowned/stray variants, piglins, hoglins, striders, wither
      skeletons, endermen, and the rest.
- [ ] Mob drops, experience, equipment, babies, riding. Drops come from the vanilla
      entity loot tables (sheep drop their colour's wool); no experience yet.
- [ ] Breeding, taming, leashes, name tags. Breeding works (`world/breeding.rs`): food
      puts adults in love, partners meet and make a baby that grows up in 20 minutes
      (faster when fed), parents rest 5 minutes, breeding drops experience, and animals
      follow players holding their food. Taming, leashes and name tags are missing.

## Phase 6: Farming, nature and time

- [x] Day/night cycle, weather (rain, thunder), sleeping in beds (`world/time.rs`):
      time and weather saved in level.dat, `/time` and `/weather`, beds set the
      respawn point and skip the night once everyone has slept 100 ticks.
      Known gaps: no lightning bolts, rain does not put out fires or fill cauldrons,
      and sleeping ignores nearby monsters (there are none yet).
- [ ] Random ticks: crops grow, saplings become trees, grass and mycelium spread.
      Done (`world/nature.rs`): three random ticks per section per tick; wheat,
      carrots, potatoes and beetroots ripen by the wiki's growth formula; oak, birch,
      spruce, jungle, acacia, cherry and dark oak (2x2) saplings grow trees; grass and
      mycelium spread and die under cover; farmland dries out; sugar cane and cacti
      grow. Missing: stems, melons and pumpkins, vines, bamboo, kelp, mushrooms.
- [ ] Bone meal (crops, saplings, grass), hoes tilling farmland, seeds and crops that
      plant under another name: done. Composters, trampling, sugar cane, cactus, bamboo,
      kelp, vines, melons and pumpkins, cocoa, sweet berries, mushrooms
- [x] Leaf decay: leaves track their distance from a log through neighbour updates and
      rot (dropping saplings, sticks and apples) once out of reach; leaves players place
      never decay.

## Phase 7: Redstone

- [ ] Redstone wire, torches, repeaters, comparators, blocks of redstone. Working
      (`world/redstone.rs`, `blocks/redstone.rs`): strong and weak power through solid
      blocks, dust networks losing one power per block and connecting up and down
      steps, torches inverting after 2 ticks, repeaters with their delays (set by
      using them), repeater locking, comparators (compare and subtract, reading how
      full chests, barrels and furnaces are, also through a block), blocks of
      redstone. Missing: torch burnout, comparators reading other blocks (cake,
      composters, item frames, ...).
- [ ] Buttons, levers, pressure plates, tripwires, daylight sensors, observers,
      target blocks, sculk sensors. Buttons, levers, pressure plates (pressed by
      players), observers and daylight detectors (inverted by using them) give power;
      tripwires, target blocks and sculk sensors are missing.
- [ ] Pistons and sticky pistons (moving blocks and entities). Pistons
      (`world/piston.rs`) push up to 12 blocks, break fragile ones, stop at obsidian,
      unbreakable blocks and blocks holding things, answer quasi-connectivity, and
      sticky ones pull a block back; clients animate the move. Missing: moving
      entities, slime and honey blocks, zero-tick behaviour.
- [ ] Hoppers, droppers, dispensers (with their item behaviours), crafters. Hoppers
      (`world/hoppers.rs`) move an item every 8 ticks between containers (furnaces by
      side), pick up items on top and lock when powered. Droppers fill containers or
      drop; dispensers shoot arrows, pour and scoop fluids, use bone meal, flint and
      steel and TNT, and drop the rest. Missing: crafters, minecart hoppers, other
      dispenser items (armor, shulker boxes, spawn eggs, boats, ...).
- [ ] Rails: powered, detector, activator
- [x] Doors, trapdoors, lamps, TNT, note blocks and bells powered by redstone. Doors
      (iron ones too), trapdoors, fence gates, lamps and TNT answer power. Note blocks
      (`world/devices.rs`) take their instrument from the block below, are tuned by
      using them and play when hit or powered; bells ring when used or powered.
- [ ] Update order and timing faithful enough that common farms and machines work

## Phase 8: Progression

- [x] Experience: orbs, levels, sources. Orbs (`world/experience.rs`) drift to players
      within 8 blocks; mobs killed by players (1-3 for animals, 5 for monsters) and ores
      mined without silk touch give experience; levels follow the wiki's costs, are
      saved, and dying drops 7 per level (at most 100). `/xp add|query`. Missing:
      experience from smelting, breeding, fishing and trading.
- [x] Enchanting table and bookshelves, enchantment effects. The table
      (`world/enchanting_table.rs`, `enchanting.rs`) counts up to 15 bookshelves,
      offers three slots by the wiki's costs, hints at what each gives, and picks
      enchantments from the data generator's definitions (weights, cost ranges,
      supported items, exclusive sets); books become enchanted books. Enchantments are
      sent to clients and saved. Working effects: sharpness, smite, bane of
      arthropods, knockback, fire aspect, sweeping edge, efficiency, unbreaking,
      fortune, silk touch, looting, the protection family, feather falling,
      respiration, power, flame, infinity and mending. Missing: thorns, punch,
      frost walker, depth strider (client side), aqua affinity, curses.
- [ ] Anvil (repair, rename, combine), grindstone. Anvils (`repair.rs`) repair with
      materials or a second item, merge enchantments from items and books by the
      wiki's rules, charge levels with the prior work penalty, refuse 40 levels or
      more in survival, and wear out. Grindstones strip all but curses, merge two
      items and give experience back. Renaming is missing (item names are not
      tracked yet).
- [ ] Brewing stand, potions, splash and lingering potions, tipped arrows
- [ ] Beacons, conduits
- [ ] Maps, compasses, recovery compasses, clocks, spyglass

## Phase 9: Dimensions

- [ ] Multiple worlds: Nether and End. Each dimension runs as its own world task
      (`world/dimension.rs`, `world/travel.rs`); players are handed over whole, their
      connection switching to the new task, and keep their dimension across logins.
      The Nether is generated by Pumpkin and kept in `DIM-1/` in the vanilla layout
      (chunks hold the overworld's 24 sections; only y = 0..255 is sent and saved).
      Chat and join/leave/death messages reach every dimension. Dying elsewhere
      respawns in the overworld. `/execute in <dimension> run tp` and `/tp` move
      players. In the Nether water boils away when poured, beds blow up (power 5,
      with fire), and lava runs as far as water, three times as often. The End is a
      third dimension (see below). Missing: players in other dimensions in the tab
      list.
- [x] Nether portals (lighting, linking, travelling). `blocks/portal.rs` finds obsidian
      frames (2 to 21 wide, 3 to 21 high, corners optional) and fire in one lights it;
      portals go out when their frame breaks. `world/portal.rs`: standing in a portal
      for 4 seconds (at once in creative) travels, at coordinates divided or multiplied
      by 8, out of the nearest known portal within 16 (Nether) or 128 (overworld)
      blocks, or a new one built on open ground nearby or on a carved-out obsidian
      ledge. Each dimension lists its portals in `levitas_portals.dat`. Missing: mobs
      and items going through.
- [ ] End portals, strongholds' frames, the End and its exit portal. Eyes of ender
      (`world/end_portal.rs`) float towards the nearest stronghold (found with Pumpkin's
      stronghold placement) and break or drop; twelve frames with eyes, facing in,
      open the portal; the End is generated by Pumpkin, kept in `DIM1/`, and players
      arrive on a 5 x 5 obsidian platform at (100, 49, 0); the exit portal opens when
      the dragon dies. Missing: the end credits.
- [ ] Ender dragon fight, end crystals, end gateways. `world/dragon.rs`: crystals on
      the pillars' bedrock tops, the exit portal's bedrock frame, and the dragon (200
      health, boss bar) circling, diving at players and landing on the portal to
      rest; crystals within 32 blocks heal it, breaking one blows it up and hurts the
      dragon if it was healing from it; head hits count in full, others a quarter
      plus one, and arrows hit it. Dying, it rises, bursts into 12,000 experience the
      first time, opens the exit portal and leaves the egg. The fight is saved.
      Missing: dragon breath and fireballs, perching attacks, respawning the dragon,
      end gateways and the outer islands' cities, the egg teleporting.
- [ ] Wither

## Phase 10: Villagers and raids

- [ ] Villagers: professions, workstations, trading, restocking, gossip
- [ ] Wandering traders, iron golems, zombie villagers and curing
- [ ] Raids and bad omen

## Phase 11: Everything else

- [ ] Advancements and statistics
- [ ] Brigadier command tree with suggestions; console commands. The built-in commands
      are declared to the client (`commands::command_tree`) for tab completion; there is
      no command framework or console input yet.
- [ ] Vanilla commands: /tp, /time, /weather, /gamerule, /give with components,
      /effect, /enchant, /summon, /setblock, /fill, /kill, /clear, /spawnpoint, ...
- [x] Game rules and difficulty. Every 1.21.8 game rule is kept and saved in
      `level.dat` as vanilla saves it (`rules.rs`), set with `/gamerule` in game or
      at the console. Enforced: keepInventory, doDaylightCycle, doWeatherCycle,
      doMobSpawning, doMobLoot, doTileDrops, doFireTick, mobGriefing (creeper and
      fireball blasts), naturalRegeneration, fallDamage, fireDamage, drowningDamage,
      showDeathMessages, randomTickSpeed, playersSleepingPercentage,
      doImmediateRespawn, reducedDebugInfo and doLimitedCrafting (told to clients).
      Difficulty comes from `server.properties` (`difficulty`, easy by default) and
      `/difficulty`: monsters hit softer on Easy and harder on Hard, starving stops
      at 5 hearts on Easy and kills on Hard, fire spreads more readily the harder
      the game, and Peaceful has no monsters and no hunger.
- [ ] Scoreboards, teams, boss bars, titles
- [ ] Item components on the network (enchantments, names, damage, ...)
- [x] Online mode (encryption and Mojang session check), compression. Compression
      works (zlib above `network-compression-threshold`, 256 by default). With
      `online-mode=true` (`auth.rs`) the server sends its RSA key and a token, takes
      the client's shared secret, encrypts the connection with AES-128-CFB8, and asks
      Mojang's session server for the player's real UUID and skin, which other
      players then see.
- [x] Server configuration file, whitelist, bans, operators. `server.properties`
      (`config.rs`, vanilla keys, written with defaults if missing), `ops.json`,
      `whitelist.json` and `banned-players.json` (`access.rs`, vanilla formats).
      Only operators may use commands beyond `/help` and `/list`; `/op`, `/deop`,
      `/whitelist`, `/ban`, `/pardon`, `/kick`, `/list` and `/say` work in game and in
      the server console, which also takes `stop`. Banned, unlisted and over-limit
      players are turned away at login.

## Phase 12: World generation

- [x] Noise terrain, biomes and climate, carvers (caves, ravines), aquifers: generated
      by [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin)'s `pumpkin-world` (GPL-3.0)
      through `levitas-pumpkin`, on worker threads, 8x8 chunks at a time so trees and
      other features crossing chunk borders come out whole. New worlds use it by default;
      `LEVITAS_WORLD_TYPE=flat` makes the flat test world, `LEVITAS_SEED` picks the seed.
      Pumpkin targets a newer Minecraft version, so its newer blocks and biomes become
      their closest 1.21.8 relatives (poplar wood as birch, sulfur as sandstone, ...).
- [x] Features: trees, ores, flowers, lakes, ...
- [x] Structures: villages, strongholds, mineshafts, temples, monuments, ...
      Pumpkin places them along with their block entities, which come out with each
      generated chunk. Chests, barrels and dispensers naming a loot table are filled
      from vanilla's chest tables when their chunk arrives (`loot.rs`: random slots,
      split stacks, enchanted and worn gear, enchanted books); spawners keep the mob
      the generator gave them. Saved chests from vanilla worlds that were never opened
      are filled when loaded. Known gaps: decorated pots, suspicious sand and gravel,
      mineshaft chest minecarts, and potions, maps and names in loot.
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
