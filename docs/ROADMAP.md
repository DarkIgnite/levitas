# Roadmap

The plan: make every vanilla survival feature work, then the plugin API for
survival-economy servers. Real terrain came early (Phase 12, through Pumpkin's generator)
so the owner can play a normal world while the remaining phases land. Each phase ends with
something that can be tested in a real 26.3 client.

The target version is **Minecraft: Java Edition 26.3** (protocol 777). It was 1.21.8
(protocol 772) until 2026-10-29. The features ticked off below were built against 1.21.8
and moved to 26.3 by the migration milestone right below; the owner plays on 26.3 since
2026-10-30.

## Migration to 26.3

Done on 2026-10-30; new features land again. Where the wiki and a real 26.3 client
disagreed, a throwaway vanilla 26.3 server settled it (see
[HANDOFF.md](HANDOFF.md#migrating-to-263)).

- [x] Data: `tools/datagen.py` downloads the 26.3 server.jar and generates
      `generated/26.3`; `levitas-registry` builds from it; DataVersion taken from the
      jar's `version.json`
- [x] Protocol 777: handshake, status, login, configuration and play packets checked
      against the 26.3 protocol pages (packet IDs come from the data; field layouts by
      hand)
- [x] Entity data: every serializer ID and field index re-checked against the 26.3
      entity metadata page, with byte-level tests
- [x] Item components, chunk and light format, registries sent in configuration
- [x] Saved data: `level.dat` (with `version_history`), chunks, entities and player data
      written as 26.3 writes them; 1.21.8 worlds (the owner's included) load and upgrade.
      Dimensions live in `dimensions/minecraft/<name>/`, players in `players/data/`, and
      game rules, weather, clocks and world generation settings in `data/minecraft/`;
      a 1.21.8 world is moved into that layout when it is opened
- [x] Loot tables (`condition`, `modifier`, named predicates) and recipes as 26.3 has them
- [x] Game rules as 26.3 has them: its 59 rules by their new names
      (`minecraft:keep_inventory`), saved as typed values; a 1.21.8 world's rules carry
      over under the new names (`disableRaids` becomes `raids`, inverted). Advancements
      are still to come (Phase 11)
- [x] Terrain: Pumpkin's 26.x blocks and biomes used as they are (dappled forests and
      sulfur caves included); the 1.21.8 fallbacks are gone
- [x] New 26.3 blocks behave. Their hardness, blast resistance, light and collision
      shapes come from Pumpkin's 26.x block data (minecraft-data stops at 26.1), their
      tool needs from a related block. The poplar wood set, wool and concrete stairs and
      slabs and the sulfur and cinnabar families work through 26.3's tags; poplar
      saplings grow poplars (one canopy colour, now and then shelf mushrooms on the
      trunk); straw beds are slept in once, set no respawn point and do not explode
      outside the overworld; red shrubs grow on soil and spread with bone meal; shelf
      mushrooms hang on the side of full blocks, grow large with bone meal and halve a
      fall onto them; sulfur spikes hang and stand like dripstone and fall, but do not
      make a fall onto them worse; potent sulfur under shallow water gives off Nausea
      gas and, over magma or lava, becomes a geyser that throws players up
      (`world/sulfur.rs`). Plants are placed by 26.3's support tags
      (`supports_vegetation`, ...), since grass is no longer in the `dirt` tag.
      Sulfur spikes and pointed dripstone take their thickness from the column (tip,
      frustum, middle, base, tip_merge), and geysers keep the wiki's rhythm (15 to 60
      seconds by depth, each block its own). Known gaps: sulfur spikes (like dripstone)
      do not grow yet; the poplar's shape
      and the geyser's push are approximations; mobs are not thrown by geysers
- [x] Every end-to-end test passes against the new protocol
- [x] Tried in a real 26.3 client: the owner joins and plays (2026-10-30)

## Definition of done

A feature is only ticked off when:

- it behaves like vanilla 26.3, checked against public documentation (minecraft.wiki),
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
- [x] level.dat: spawn point, world time, weather and game rules (the last three in
      `data/minecraft/`, as 26.3 keeps them)
- [x] Player data: position, rotation, inventory, equipment, health, hunger, game mode
- [x] Autosave every 5 minutes, save on shutdown (Ctrl-C), unload unused chunks
- [x] Dropped items and other entities saved (with Phase 4). Mobs, dropped items,
      boats, minecarts, item frames, paintings and armor stands are saved in `entities/`
      region files, and so are arrows and thrown tridents (stuck or flying, with their
      owner, pickup, damage, piercing and item), experience orbs (value, age) and
      falling blocks (block state, time) (`world/saved_entities.rs`), as vanilla saves
      them.
- [x] LZ4-compressed region files (vanilla's optional format). Chunks are read however
      they were written (zlib, gzip, none or LZ4); `region-file-compression` in
      `server.properties` (deflate, lz4 or none, as vanilla) picks how new ones are
      written. LZ4 is lz4-java's block stream with its xxHash32 checksums
      (`levitas-world/src/lz4.rs`, no new dependency).
- [x] Item components preserved when loading worlds from other servers. Components
      Levitas does not handle (custom data from plugins, rarity, unbreakable, ...) are
      kept with the stack (`item_extras.rs`) and saved back unchanged; stacks with
      different ones do not merge. Clients are sent only the components the server can
      encode.

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
- [x] Block placement sounds for other players. The generator gives no sound types, so
      `blocks::place_sound` works them out from the block's own sound, its family, its
      tags or its name (stone otherwise).
- [x] Bells, cocoa, hanging signs, crafters and walls' tall sides: checked against the
      wiki. Bells stand on floors, hang from ceilings and from one or two walls, facing
      away from the player or their wall, drop without their support and break under
      pistons; they ring only when struck on a side they swing to and below their beam,
      by redstone and by arrows, count the bell_ring statistic, send villagers within 32
      blocks home for a while and make raiders within 48 glow when one is within 32.
      Cocoa grows a stage 20% of random ticks, takes bone meal, drops 3 beans ripe and
      1 before, and bursts under flowing water and pistons. Hanging signs under a full
      face hang on straight chains turning in four steps, under something narrow (or
      when sneaking) on meeting chains in sixteen; they drop with nothing above, while
      wall ones need a full face to be put up, stick out facing along the wall and keep
      their bracket after. Walls rise only under a full face or a wall running the same
      way and keep a post only under `#wall_post_override`; crafters (`world/crafter.rs`)
      craft on a pulse with switched-off slots, comparators and hoppers.
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
- [x] Special crafting recipes: dyeing, fireworks, map and book cloning, banners,
      shields, repairing, decorated pots, transmute recipes (`recipes.rs`): repairing two
      worn items (uses added plus 5%, only curses kept); dyeing leather armor by the wiki's
      mix; transmute recipes with material counts (shulker boxes and bundles changing
      colour, map cloning with up to eight maps); map extending; tipped arrows; decorated
      pots with sherds (`minecraft:pot_decorations`, kept when placed and broken); book
      cloning (written books, one generation on, the original staying); banner copying and
      shield decoration (`minecraft:banner_patterns`, `minecraft:base_color`); firework
      stars (shape, colours, trail, twinkle), fades and rockets (flight 1 to 3).
      Alongside: the loom (patterns from the tags, in order, pattern items, up to six
      layers), banners keeping their layers when placed and broken, cauldrons (buckets,
      bottles, washing armor, banners and shulker boxes), books (writing, signing,
      reading, lecterns with page turns, pulses and comparators) and firework rockets in
      flight (`world/rockets.rs`: launched from blocks, dispensers and crossbows, boosting
      gliding players, bursting with damage by their stars).
- [x] Signs (editing text), item frames, armor stands, flower pots, jukebox, note blocks.
      Signs and hanging signs work (`world/signs.rs`): placing one opens its editor,
      either side can be edited again by right-clicking it, dyes colour a side's text,
      glow ink sacs make it glow and ink sacs undo that, honeycomb waxes it for good.
      Text is saved in vanilla's format (older worlds' JSON lines are read too). Note
      blocks, item frames and armor stands work, and so do flower pots (any plant with a
      potted block) and jukeboxes (`world/jukebox.rs`: songs for their length, ejecting,
      comparators, hoppers).

## Phase 3: Fluids, gravity and light

- [x] Water and lava flowing, sources, waterlogging, buckets. Flow follows the wiki:
      levels, falling columns, infinite water sources, the nearest way down within 4
      (water) or 2 (lava) blocks; plants and torches wash away. Fluid ticks are saved.
      Water breaks falls. Waterloggable blocks placed into a water source are
      waterlogged; flowing water carries dropped items along and they float up
      through water (`World::flow_vector`).
- [x] Lava and water interaction (cobblestone, stone, obsidian, basalt)
- [x] Drowning, fire and lava damage, burning: 300 ticks of air, 2 drowning damage a
      second; lava 4 and fire 1 (soul fire 2) every half second, setting players alight;
      magma hurts unless sneaking; burning hurts once a second until water or rain puts it
      out. Armor follows the `bypasses_armor` tag. Mobs burn in lava; items burn up.
- [x] Fire (`world/fire.rs`): flint and steel lights fire, TNT, campfires and candles;
      fire ages, goes out without fuel or in the rain, burns flammable blocks away by
      the wiki's odds and spreads to nearby air; netherrack and magma burn forever.
      Lava sets fires on its random ticks: air next to something flammable in the
      3x3 layer above it, or the 5x5 layer above that when the way up is open, and
      the tops of flammable blocks beside it (only with fire spreading on).
- [x] Swimming physics: sprinting with the head under water starts swimming and it
      lasts while the player sprints in water; the swimming flag and pose go to
      everyone (`World::update_swimming`). Other players now also see sneaking
      (crouching pose), sprinting and elytra gliding (fall-flying pose).
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
      Faces that are solid all over (a bottom slab's underside, the back and bottom of
      stairs, farmland's underside) stop light both ways (`lighting::full_faces`).
      When a chunk loads, light runs across its borders both ways
      (`LightEngine::chunk_loaded`, scanning only sections that differ).
- [x] Ice melting, snow layers, frosted ice: ice melts in block light above 11 on
      any side, into water (nothing in the Nether); snow layers melt all at once in
      block light of 12 or more; frost walker's frosted ice cracks and melts back.

## Phase 4: Combat and basic entities

- [x] Attacking: damage, attack cooldown, critical hits, knockback, sweeping
      (`world/combat.rs`). Mobs and players are hit the same way: held item's attack
      damage and speed from the data generator, charge, critical hits, sprint knockback,
      sweeping sword blows, weapon wear, attack sounds. Players get vanilla's short
      invulnerability after a hit.
- [x] Armor and armor toughness, shields, damage types and immunities. Worn armor
      reduces mob and explosion damage by the wiki's formula and wears down;
      protection enchantments work. Shields (`world/shields.rs`) block hits, arrows,
      fireballs and explosions from the front once raised for 5 ticks, wear down on
      hits of 3 or more, and are knocked down for 5 seconds by an axe; other players
      see them raised. Blocked arrows bounce off and fall; a blocked melee attacker is
      pushed back.
- [x] Bows, crossbows, tridents, arrows, snowballs, eggs, ender pearls. Bows and arrows
      work (`world/arrows.rs`): draw strength, gravity and drag, damage from speed,
      critical shots, sticking in blocks, pickup. Ender pearls, snowballs and eggs are
      thrown (`world/thrown.rs`): pearls teleport their thrower for 5 damage with a
      one-second cooldown, snowballs hurt blazes, eggs hatch chicks. Crossbows
      (`world/ranged.rs`) load in 1.25 s (less with Quick Charge) and keep their arrow
      as `minecraft:charged_projectiles`; they shoot at 3.15 blocks a tick, three arrows
      with Multishot, through more mobs with Piercing. Tridents are thrown after half a
      second for 8 damage (Impaling adds against water mobs), drop away from what they
      hit and are picked up by their thrower; Loyalty brings them back, Channeling calls
      lightning in a thunderstorm, Riptide flings the player in water or rain. Crossbows
      shoot arrows only, since fireworks are not in yet.
- [x] Status effects on players and mobs (`world/effects.rs`): given by food (golden apples,
      rotten flesh, pufferfish, ...), splash potions, tipped arrows and `/effect`, cleared
      by milk and death, removed by honey, shown to the client and saved. Regeneration,
      poison, wither, hunger, saturation, instant health and damage, absorption, resistance,
      fire resistance, water breathing, strength, weakness, haste, mining fatigue, jump boost
      and slow falling act on the server; the rest are carried out by the client. Food leaves
      its bowl, bucket or bottle behind. Mobs take potion effects: instant health damages
      undead and heals the living; instant damage heals undead and damages the living; spiders
      and undead ignore poison; undead ignore regeneration; strength/weakness adjust attack
      damage; speed/slowness scale walk speed; resistance and fire resistance protect mobs.
      Health Boost raises the most health by 4 a level; chorus fruit teleports the eater
      up to 8 blocks away.
- [x] Entity framework: tracking, physics, collisions, saving entities to disk.
      Mobs (`world/mobs.rs`) are shown to players who have their chunk loaded, move
      with gravity and block collisions, and are saved in vanilla `entities/` region
      files. Dropped items and falling blocks are tracked the same way: shown to the
      players with their chunk loaded, checked every third tick.
- [x] Minecarts, boats, item frames, paintings, armor stands, experience orbs.
      Experience orbs work. Boats, rafts and chest boats of every wood (`world/boats.rs`)
      are placed on water or ground, ridden by two (the driver's client steers, the server
      checks each move and shows it to others, paddles included; the back seat moves up
      when the driver gets off), left by sneaking, broken by hits into their item, float up
      to the surface when empty, and are saved with the chunk's entities; chest boats have
      one seat and 27 slots, opened by sneaking. Mobs that wander into a boat with a free
      seat sit down (not water mobs, flyers, golems or the too wide), and get out when it
      breaks. Minecarts (`world/minecarts.rs`) are placed on rails, ridden and left,
      broken into their item, and moved by the server along straight rails, curves and
      slopes, pushed by powered rails (and off a wall from standing), braked by unpowered
      ones, and nudged by a rider holding forward; off the rails they fall and slide to a
      stop; they push each other apart when they run into each other, pick up mobs they
      run into, and are saved like boats. Chest minecarts (27 slots, loot from mineshafts),
      hopper minecarts (5 slots, taking from the container above and the items on them;
      stopped by a powered activator rail), furnace minecarts (coal for fuel, pushing
      themselves along) and TNT minecarts (lit by an activator rail, fire or lava, blowing
      up harder the faster they go) work; hoppers fill and empty chest and hopper
      minecarts, and a powered activator rail puts riders off. Item frames and glow item
      frames (`world/hanging.rs`) hang on any face, take the held item, turn it an eighth
      per click, give it back when hit and break on the next hit; maps in them are drawn,
      and comparators read their rotation. Paintings pick at random among the biggest
      placeable ones that fit the wall. Both fall off when their wall goes and are saved in
      vanilla's form. Armor stands (`world/armor_stands.rs`) are placed facing the player,
      dressed with armor (swapping what was worn), undressed by clicking the part with an
      empty hand, broken by two quick hits (dropping what they wore), fall when unsupported
      and are saved with their equipment; saved stands show their arms, size, base plate,
      marker and pose, and stands with arms hold any item. Known gaps: mobs riding a
      vehicle are saved standing beside it; command block minecarts.
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
      Inside structures their own monsters spawn instead (`spawn_overrides` of the
      structure data): fortresses on their nether brick floors (blazes, wither
      skeletons, ...), swamp huts on their spruce floors (witches), and ocean
      monuments and pillager outposts in the space around them (guardians in water
      and pillagers, neither needing the dark). Mobs a structure places (elder
      guardians, mansion illagers, hut witches, end city shulkers) never despawn.
      Guardians lock a beam on their prey for four seconds (elder three) and hurt it
      for 6 (8); elder guardians give Mining Fatigue III to players within 50 blocks
      every minute. Missing: animals respawning later.
      Shulkers (`world/shulkers.rs`) sit fixed to their face, open up at players in
      sight within 16 blocks and fire homing bullets every 1 to 5.5 seconds (4 damage
      and Levitation for 10 seconds; players can knock them down); a closed shell has
      20 armour; hurt below half health one may teleport within 8 blocks. Missing:
      shulkers duplicating, dye colours, attaching to walls and ceilings when they
      teleport.
      Bees (`world/bees.rs`): nests and hives hold up to three bees (saved as
      vanilla's `bees` list); nests in generated trees get two or three. By day in
      fair weather bees go out to `bee_attractive` flowers within 5 blocks, gather
      nectar, help `bee_growables` crops on the way back and raise the honey level;
      at night and in the rain they stay in or go home. Full hives give three
      honeycombs to shears or a honey bottle to a glass bottle; without a campfire
      below, or when the hive is broken (not with silk touch), the bees come out
      angry; hitting a bee angers those near. A bee stings once (poison 10 s on
      Normal, 18 s on Hard) and dies some time later. Missing: bees kept in a silk
      touch hive item, pollen particles, dispensers harvesting.
      Foxes, parrots, turtles and axolotls (`world/critters.rs`): foxes are red or
      white (snowy biomes), sleep by day, keep away from players and hunt chickens,
      rabbits and fish at night. Parrots come in five colours; seeds tame one in ten
      tries; tamed ones sit or follow their owner, flying, and a cookie kills them.
      Turtles remember their home beach; one that bred carries an egg home and lays
      one to four on sand, which crack (nearly always at night) and hatch; babies
      drop a scute when grown. Axolotls come in four colours (blue one in 1200 by
      breeding), hunt fish, squid, drowned and guardians in water, dry out after five
      minutes on land, and breed on tropical fish buckets (the water bucket comes
      back). Water buckets catch cod, salmon, pufferfish, tropical fish, axolotls and
      tadpoles; emptying the filled bucket lets them out. Missing: what a bucket
      remembers of its mob (colour, health), foxes carrying items and eating berries,
      parrots on shoulders, dancing and mimicking, axolotls playing dead.
      Goats, frogs, llamas, dolphins and pandas each live in a module of their own
      behind `world/kin.rs`. Goats: screaming goats (own sounds, inherited), horns,
      ramming at things standing still 4 to 16 blocks off (horns snap into goat horns on
      `snaps_goat_horn` blocks), milking. Frogs: biome variants, eating small slimes
      and magma cubes (slime balls, froglights), frogspawn on water after breeding,
      hatching after 3600 to 12000 ticks into tadpoles, which grow into frogs of
      the biome they grow up in. Llamas spit at wolves and attackers and wear
      carpets. Dolphins dry out on land, must surface for air, give Dolphin's Grace,
      lead players fed fish to structures and fight back as a group. Pandas have
      main and hidden genes and the seven personalities, sneeze, eat bamboo and
      cake, and mate only near bamboo. Missing: llama spit removal when the llama
      despawns, frogspawn breaking at once when its water goes, a mob index for
      goats' target scans.
      Breezes, wardens, withers and creakings run their own AI (`kin::own_ai`).
      Breezes leap about, keep their distance, shoot wind charges whose bursts throw
      entities and flip doors, levers and buttons, and deflect projectiles. Wardens
      come from can_summon shriekers set off through sculk sensors (warning levels
      1 to 4, Darkness), track vibrations and smell, grow anger, roar, sniff, emerge
      and dig back down, hit for 30 and fire sonic booms, and spread Darkness.
      Withers are built from soul sand and three wither skeleton skulls, charge up
      invulnerable with a boss bar and blow up, fly and fire black and blue skulls,
      get armour against arrows below half health, break blocks when hurt and heal
      over time. Creaking hearts between pale oak logs wake at night, spawn a
      creaking bound to them that freezes while watched, cannot be hurt while its
      heart lives, dies with it and vanishes at dawn. Missing: sonic boom and wither
      skull death messages, warning levels saved with players, projectiles shown to
      players who join mid-flight, natural hearts noticed when their chunk loads.
      Armadillos roll up at threats (undead, sprinting or riding players, attackers)
      and take less damage rolled, shed scutes and give them to brushes; spiders keep
      away from them. Camels dash when their rider jumps, sit down now and then and
      take a second rider; camel husks are their undead kin. Sniffers sniff, dig
      seeds from `sniffer_diggable_block` (gameplay/sniffer_digging) and lay eggs
      that hatch faster on moss. Allays fetch the item they were given to their
      player or a note block they heard, dance by jukeboxes and duplicate on
      amethyst. Bats hang from ceilings, flutter about and spawn in dark caves.
      Copper golems are built from copper blocks and a pumpkin, oxidize into
      statues, are scraped and waxed, and sort items from copper chests into chests.
      Dried ghasts hydrate in water into ghastlings that grow into happy ghasts;
      harnessed ones carry four players, steered by the first. Missing: camel husk
      jockeys, leads, the riding input beyond what Move Vehicle and riding jumps
      carry, copper weathering of other blocks.
- [x] Pathfinding and AI goals. A* over walkable blocks (`world/pathfinding.rs`):
      walking, one-block jumps, drops of up to three blocks, with the wiki's per-mob
      penalties (lava, fire, magma, cactus, berry bushes, powder snow, water, fences);
      monsters (except climbing spiders) follow paths to their target. Wandering and
      panicking mobs (`world/ai.rs`) walk paths to reachable harmless spots, fleeing
      away from what hurt them; non-swimmers in water head for land. Villagers open
      and close wooden and copper doors; some zombies break them on Hard
      (`CanBreakDoors`); closed doors are walls to everyone else. Missing: spiders
      climbing, boat/door-aware long-range village navigation.
- [ ] Passive mobs: cow, pig, sheep (shearing, dye), chicken, rabbit, horse family,
      wolf, cat, fox, parrot, turtle, fish, squid, axolotl, frog, bee, goat, ...
      Every 26.3 mob now exists (`KINDS` in `world/mobs.rs`): base health, hitbox,
      speed, damage, sight and experience come from the data generator's attribute
      defaults (through Pumpkin's entity data, `levitas_registry::MobStats`); they
      spawn from the biome lists, wander, swim or fly, and monsters hunt and hit.
      Mobs keep the saved tags Levitas does not handle yet. Their own behaviours are
      listed below as they land.
      Horses and their kin (`world/horses.rs`): horses, donkeys, mules, skeleton and
      zombie horses, llamas and camels are born with vanilla's random speed, jump and
      health (and coats and markings, a llama's strength). Riding an untamed one tames
      it on a roll under its temper or bucks the rider off (temper +5); food heals and
      calms them. Tamed adults take a saddle (shears take it off) and their rider's
      client steers them, as with boats; sneaking gets off. Golden carrots and apples
      make tamed ones breed; a horse and a donkey make a mule; foals take after their
      parents. Saved as vanilla saves them (saddle in `equipment`, `attributes`).
      Missing: horse armour, donkey and llama chests, the horse inventory screen,
      leashes, llama caravans and spit, camel dashing and two seats.
      Piglins (`world/piglins.rs`): they leave players wearing gold armour alone
      unless angered (a hit on any piglin near, or opening or breaking blocks they
      guard within 16 blocks); half carry crossbows and shoot, the rest golden
      swords; brutes always attack. Gold ingots handed over or dropped near an adult
      are admired for six seconds and bartered from `gameplay/piglin_bartering`.
      Outside the Nether piglins and brutes turn into zombified piglins and hoglins
      into zoglins after 15 seconds. Missing: baby piglins, hunting hoglins, fear of
      soul fire, picking up other gold items.
      Done: cows (milking), pigs, sheep (natural wool colours, shearing, regrowing wool
      by eating grass), chickens (eggs, slow falling). They wander, panic when hit,
      swim, look at players and make their sounds. Missing: breeding, babies, temptation
      by food, dyeing, biome variants (all are temperate), and every other animal.
      Wolves (`world/wolves.rs`): wild ones are neutral until hit, then the pack bites
      the player for 20 to 39 seconds; a bone tames one in three tries (sits, 40
      health, red collar, dye changes it, meat mends it); tamed wolves sit or stand on
      right-click, follow and teleport to their owner, and join the owner's fights.
      Pups of tamed wolves are tamed. Missing: biome wolf variants and sound variants
      (all pale and classic), wolves hunting sheep, skeletons and rabbits, armour,
      shaking dry, wagging tails and health-based growls, cats, parrots and horses.
      Wolves wear their biome's coat (26.3's wolf variants). Cats: tamed with raw cod or
      salmon, sit, collars, random coats, follow and teleport, bred with fish; wild ones keep
      away from players; strays turn up next to villagers. Missing for cats: lying on beds
      and chests, scaring creepers and phantoms, morning gifts, ocelots.
      Rabbits: 3 health, bolt from players within 5 blocks, breed on carrots, golden
      carrots and dandelions, spawn from the biomes' creature lists, drop by their loot
      table. Missing: biome coat variants (all brown), hopping animation timing, the
      killer bunny, eating carrot crops.
      Squid, cod and salmon (`MobKind::swims`): they drift to goals inside the water, flee
      faster when hurt, dry out (1 damage a second) on land, and spawn in one freshly
      generated water chunk in three from the biome's water lists. Missing: pufferfish,
      tropical fish, glow squid, dolphins, turtles, schooling, fish buckets, ink clouds,
      and the bucket/from-bucket entity data (none is sent, so fish look default).
- [ ] Hostile mobs: zombie variants, skeleton variants, creeper, spider, enderman,
      slime, witch, phantom, drowned, pillager, silverfish, ...
      Variants share their base mob's behaviour (`MobKind::family`): husks (zombies
      the sun leaves alone, whose hits make players hungry for 7, 14 or 21 seconds on
      Easy, Normal and Hard; desert), strays (skeletons whose arrows slow for 30
      seconds; snowy biomes), cave spiders (12 health, small, poison 7 or 15 seconds on
      Normal and Hard; from mineshaft spawners) and slimes (three sizes by size squared
      health, size as damage and none for the smallest, split when they die, hop;
      swamps). Husks and strays spawn only under open sky. Missing: slime chunks
      underground (and moon phases in swamps), drowned, jockeys, baby husks,
      reinforcements, converting under water or in powder snow.
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
      follow players holding their food. Wolves can be tamed; name tags work. Leads
      work (`world/leads.rs`): leashing, pulling and snapping, leash knots on fences,
      saved as vanilla. Taming of cats, horses and parrots is missing.

## Phase 6: Farming, nature and time

- [x] Fishing (`world/fishing.rs`): a rod casts a bobber that floats in water; a fish bites
      after 5 to 30 seconds (5 less a level of Lure; sooner in rain over open water, later
      under a roof), with a bubble trail as it comes, and stays for 1 to 2 seconds; reeling
      in then rolls `gameplay/fishing` (fish, junk, treasure only in open water, Luck of the
      Sea weighing by quality), throws the catch to the angler, gives 1 to 6 experience,
      counts fish_caught and the fishing_rod_hooked advancements (Fishy Business). Casts hook
      mobs and dropped items, which reeling in pulls closer; the rod wears 1 for a catch, 2
      from the ground, 3 for an item and 5 for a mob. The bobber goes when the rod is put
      away, its owner dies or is 32 blocks off.
- [x] Day/night cycle, weather (rain, thunder), sleeping in beds (`world/time.rs`):
      time and weather saved in level.dat, `/time` and `/weather`, beds set the
      respawn point and skip the night once everyone has slept 100 ticks, and not with
      monsters nearby. Rain puts out fires. Weather acts on the world (`world/weather.rs`):
      each loaded chunk gets a precipitation tick 1 in 16 ticks on a random column, where in
      a cold biome (temperature below 0.15, falling 0.00125 a block above y 80) water
      freezes and snow settles up to `max_snow_accumulation_height`, and rain (5%) or snow
      (10%) fills cauldrons. Thunderstorms strike each chunk 1 in 100,000 ticks: the bolt
      goes to the nearest lightning rod within 128 blocks at the top of its column (which
      gives off redstone power for 8 ticks and loses its patina), or to a living thing in
      the open nearby; it sets fires on Normal and Hard, does 5 damage and 8 seconds of
      fire, burns up items, charges creepers (a blast twice as strong, saved as `powered`)
      and turns pigs into zombified piglins and villagers into witches.
- [x] Random ticks: crops grow, saplings become trees, grass and mycelium spread.
      Done (`world/nature.rs`): three random ticks per section per tick; wheat,
      carrots, potatoes and beetroots ripen by the wiki's growth formula; oak, birch,
      spruce, jungle, acacia, cherry and dark oak (2x2) saplings grow trees; grass and
      mycelium spread and die under cover; farmland dries out; sugar cane and cacti
      grow. And (`world/farming.rs`): melon and pumpkin stems ripen by the crop formula
      and put fruit on a free side with fitting ground (`supports_stem_fruit`), bending
      to it and straightening when it is taken; cocoa and sweet berry bushes grow a
      stage a fifth of the time; kelp grows up through still water to age 25; bamboo
      shoots become stalks that grow to 16 blocks; vines grow down (`spread_vines`);
      mushrooms spread where it is dark or onto mycelium and podzol, up to five in a
      9x3x9 area. Vines spread as the wiki describes: down into air or onto the vine
      below, up and sideways along and round walls and under ceilings while fewer than
      four others are within the 9x3x9 area.
- [x] Bone meal (crops, saplings, grass), hoes tilling farmland, seeds and crops that
      plant under another name. Bone meal also grows stems, cocoa, sweet berries, kelp
      and bamboo. Sweet berries are picked by hand (bushes grown two stages and up).
      Composters take what 26.3's `compostable` component says, with its chances (the
      first layer always), are full at 7 and ready a second later, and give bone meal.
      Falls onto farmland trample it with the wiki's chance (height less half a block).
      Cocoa sits on any jungle log or wood. Mobs big enough (width x width x height over
      0.512) trample farmland too, when mob griefing is on. Bone meal grows a small
      mushroom into a huge one 40% of the time (`world/huge_mushroom.rs`, shapes ported
      from Pumpkin). A hopper above a composter feeds it, one below takes the bone meal,
      and comparators read composter and cauldron levels.
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
- [x] Rails: powered, detector, activator. Powered and activator rails take power
      from redstone and pass it along up to 8 rails of their kind; detector rails
      give off power (strong into the block below) while a minecart is on them
      (`world/minecarts.rs`).
- [x] Doors, trapdoors, lamps, TNT, note blocks and bells powered by redstone. Doors
      (iron ones too), trapdoors, fence gates, lamps and TNT answer power. Note blocks
      (`world/devices.rs`) take their instrument from the block below, are tuned by
      using them and play when hit or powered; bells ring when used or powered.
- [ ] Update order and timing faithful enough that common farms and machines work

## Phase 8: Progression

- [x] Experience: orbs, levels, sources. Orbs (`world/experience.rs`) drift to players
      within 8 blocks; mobs killed by players (1-3 for animals, 5 for monsters) and ores
      mined without silk touch give experience; levels follow the wiki's costs, are
      saved, and dying drops 7 per level (at most 100). `/xp add|query`. Smelting
      (the recipe's experience, paid when the result is taken), breeding, fishing and
      trading give experience too.
- [x] Enchanting table and bookshelves, enchantment effects. The table
      (`world/enchanting_table.rs`, `enchanting.rs`) counts up to 15 bookshelves,
      offers three slots by the wiki's costs, hints at what each gives, and picks
      enchantments from the data generator's definitions (weights, cost ranges,
      supported items, exclusive sets); books become enchanted books. Enchantments are
      sent to clients and saved. Working effects: sharpness, smite, bane of
      arthropods, knockback, fire aspect, sweeping edge, efficiency, unbreaking,
      fortune, silk touch, looting, the protection family, feather falling,
      respiration, power, flame, infinity, mending, thorns (15% a level, 1 to 4 back
      at melee attackers), punch, frost walker (freezing still water into frosted ice
      that cracks and melts), curse of binding and curse of vanishing. Enchantments that
      act through attributes (efficiency, aqua affinity, depth strider, respiration,
      swift sneak, blast and fire protection, sweeping edge) are sent to each player as
      attribute modifiers read from their definitions (`world/attributes.rs`).
- [x] Anvil (repair, rename, combine), grindstone. Anvils (`repair.rs`) repair with
      materials or a second item, merge enchantments from items and books by the
      wiki's rules, charge levels with the prior work penalty, refuse 40 levels or
      more in survival, and wear out. Renaming (`names.rs`) costs a level more, or
      one level plus the penalty alone (never too expensive), names whole stacks,
      and an empty name takes a custom name off. Name tags name mobs, which then
      never despawn. Grindstones strip all but curses, merge two items and give
      experience back.
- [x] Brewing stand, potions, splash and lingering potions, tipped arrows. Potions
      (`potions.rs`, effects and recipes from the wiki's tables) are carried as the
      `minecraft:potion_contents` component. Glass bottles fill at water; drinking
      gives the effects. Brewing stands (`world/brewing.rs`) take blaze powder for 20
      brews and brew in 20 seconds: base potions, effects, redstone and glowstone,
      fermented spider eyes, gunpowder (splash) and dragon's breath (lingering), fed
      by hoppers as in vanilla. Splash potions give players and mobs within 4 blocks their
      effects, weaker with distance, with accurate particle colors (`potions::color`).
      Lingering potions and dragon's breath spawn Area Effect Clouds (`world/clouds.rs`,
      `entity_type::AREA_EFFECT_CLOUD`) that linger, shrink per application, apply
      quarter-duration effects or magic damage, and dragon's breath can be scooped into
      bottles (`minecraft:item.bottle.fill_dragonbreath`). Tipped arrows, shot from bows
      and dispensers, give players and mobs their potion for an eighth of its time and
      are picked up as tipped arrows; Infinity does not spare them. Undead mobs invert
      healing and harming; undead and spiders ignore poison; resistance and fire resistance
      protect mobs.
- [x] Beacons, conduits. Beacons (`world/beacon.rs`) calculate pyramid layers (1 to 4) of
      mineral blocks (`minecraft:beacon_base_blocks`), verify unobstructed sky above
      (`light_filter < 15`), provide container (`menu::BEACON`) with payment slot for
      netherite/emerald/diamond/gold/iron ingots (`click::Layout::Beacon`), support
      `serverbound::SET_BEACON` to configure primary and secondary powers, broadcast
      `BLOCK_ENTITY_DATA` to render the beam, and pulse status effects (Speed, Haste, Resistance,
      Jump Boost, Strength, Regeneration) every 80 ticks to players in range. Conduits
      (`world/conduit.rs`) activate underwater inside a 3x3x3 water cage and 16 to 42
      prismarine/sea lantern frame blocks, grant Conduit Power (underwater breathing, night vision,
      mining speed) to nearby players in water or rain, and attack hostile mobs with 4 magic damage
      every 40 ticks when fully framed (42 blocks).
- [x] Maps, compasses, recovery compasses, clocks, spyglass. `inventory.rs` serializes and saves
      `minecraft:lodestone_tracker`, `minecraft:map_id` and `minecraft:map_decorations` item
      components. Right-clicking a lodestone with a compass binds it (`item.lodestone_compass.lock`)
      to that position and dimension. Dying records `LastDeathLocation: {dimension, pos}` in player
      NBT and sends it in login and respawn packets so recovery compasses point to the player's
      last death. Maps (`world/maps.rs`): using an empty map starts a filled map of the grid square
      the player stands in; while held it draws the terrain within 128 blocks (64 in the Nether)
      from the highest block maps can see, shaded by height against the pixel to the north and
      water by depth, mixing the colours of a zoomed-out pixel's area, a share of the columns each
      tick, and sends the changed pixels with the markers: holders, item frames holding the map,
      banners marked by using the map on them, and an explorer map's destination. Maps in frames
      are sent to players within 64 blocks. Maps are saved as `data/minecraft/maps/<id>.dat` with
      `maps/last_id.dat`, as 26.3 does; their IDs are shared by all dimensions. Cartography tables
      clone maps (any in `#clonable_maps`), and locking (glass pane) or zooming out (paper) makes a
      new map, as vanilla does (`minecraft:map_post_processing` marks the result until it is
      taken). Explorer maps (26.3's own items: `ocean_monument_map`, `woodland_mansion_map`,
      `buried_trial_chambers_map`, the jungle, swamp and five village maps) are sold by
      cartographers from the data-generated trade lists, by villager kind; the nearest structure
      within 100 chunks is found with Pumpkin's structure placement, the map (scale 2) is started
      there and marked, and a trade with no structure in reach is dropped. Using a spyglass raises
      living flags and plays `item.spyglass.use`; releasing, switching slots, or dying stops usage.
      Clocks and compasses are drawn by the client. Not done: explorer maps' biome preview (the map
      fills in once explored).

## Phase 9: Dimensions

- [ ] Multiple worlds: Nether and End. Each dimension runs as its own world task
      (`world/dimension.rs`, `world/travel.rs`); players are handed over whole, their
      connection switching to the new task, and keep their dimension across logins.
      The Nether is generated by Pumpkin and kept in `dimensions/minecraft/the_nether/`
      (chunks hold the overworld's 24 sections; only y = 0..255 is sent and saved).
      Chat and join/leave/death messages reach every dimension. Dying elsewhere
      respawns in the overworld. `/execute in <dimension> run tp` and `/tp` move
      players. In the Nether water boils away when poured, beds blow up (power 5,
      with fire), and lava runs as far as water, three times as often. The End is a
      third dimension (see below). The player list is one for the whole server
      (`world/tablist.rs`, shared by the dimensions): everyone is listed wherever they
      are, a player who traveled can be seen in the new dimension, leaving the game
      takes them off every list, game mode changes reach all, and `/msg` reaches
      players in other dimensions. Missing: selectors and other commands across
      dimensions.
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

- [x] Villagers: professions, workstations, trading, restocking, gossip. Villages
      generate with their villagers (and Pumpkin's structure entities come out with
      each chunk). Villagers (`world/villagers.rs`) wander, claim an unclaimed
      workstation within 16 blocks and take its profession, and trade from the
      wiki's tables for all 13 professions (enchanted gear and books included).
      Trading earns them experience up to master, each level adding two trades;
      they restock twice a day while they have a workstation; one that never traded
      loses its profession with its workstation. Picking a trade pulls the payment
      from the inventory. Saved in vanilla's format. Prices follow demand (it rises by
      the uses and falls by the uses left at each restock, adding base x demand x
      multiplier while positive) and reputation from gossip. Their day
      (`world/village_life.rs`) follows vanilla's schedules: idle, at work from 2000
      (walking to their workstation), meeting at the bell from 9000, idle from 11000 and
      asleep in their claimed bed from 12000 (lying in it, the bed occupied, waking at
      dawn or when hurt); babies play instead of working. They pick up food and crops into
      an 8-slot inventory (saved as `Inventory`, home and bell as Brain memories);
      farmers harvest and replant ripe crops around them; one with plenty of food throws
      some to a hungry neighbour at the bell. Two adults with 12 food points each and a
      free bed about make a baby, eating the food. Iron golems and cats are below and in
      Phase 5.
- [x] Wandering traders, iron golems, zombie villagers and curing. Iron golems
      (`world/golems.rs`) come with villages and are built from four iron blocks
      and a carved pumpkin; they go after monsters (not creepers) within 16 blocks
      and fling them up, turn on players who hit them unless a player built them,
      are mended by iron ingots, take no fall damage or knockback and drop iron and
      poppies. Wild iron golems spawn when 3+ panicking villagers gather within 32
      blocks. Monsters (zombies, raiders) actively hunt villagers and iron golems.
      When a zombie kills a villager, it converts into a zombie villager retaining
      its profession and trades. Zombie villagers also spawn naturally (5% chance).
      Weakened zombie villagers fed a golden apple begin conversion (3600 ticks,
      Strength I effect, conversion shakes), completing into a cured villager with
      permanent trade discounts. Wandering traders spawn periodically near players
      with 1-2 trader llamas (which follow the trader and spit at hostiles), offering
      5 regular trades and 1 special trade, drinking invisibility potions at night
      and milk at dawn, and despawning after 48,000 ticks.
- [x] Raids and bad omen (`world/raids.rs`). Drinking an ominous bottle
      (`minecraft:ominous_bottle`, `world/survival.rs`) gives Bad Omen for 100 minutes
      (`mob_effect::BAD_OMEN`). Approaching within 64 blocks of a villager converts Bad Omen
      into Raid Omen (`mob_effect::RAID_OMEN`, 30 seconds) with trial spawner activation sounds
      (`minecraft:block.trial_spawner.ominous_activate`, `minecraft:event.mob_effect.raid_omen`).
      When Raid Omen expires, the raid begins at the village center, sounding the raid war horn
      (`minecraft:event.raid.horn`, volume 64.0) and displaying the raid boss bar (`clientbound::BOSS_EVENT`).
      Waves bring raider mobs (`world/mobs.rs`): pillagers (shooting crossbows, wave 1 leader wearing an
      ominous banner and dropping ominous bottles), vindicators (iron axes dealing 9.0 melee damage),
      ravagers (100 health, dealing 12.0 damage with 1.5 knockback, dropping saddles), witches (throwing
      splash potions of slowness, poison, weakness, harming, or drinking healing/fire resistance), and
      evokers (conjuring snapping Evoker Fangs in a line dealing 6.0 magic damage, dropping totems of
      undying). Raiders target players, villagers, and iron golems; iron golems actively fight raiders.
      Clearing all waves achieves Victory, turning the boss bar green, playing `minecraft:ui.toast.challenge_complete`,
      celebrating villagers, and awarding Hero of the Village (`mob_effect::HERO_OF_THE_VILLAGE`, 40 minutes),
      which grants substantial trade discounts (`special_price` in `MERCHANT_OFFERS`, `world/villagers.rs`).
      If all villagers die, Defeat is declared and raiders celebrate.
      Raids are saved in the overworld's `levitas_raids.dat` (state, wave, omen, boss bar and the
      UUIDs of the living raiders); raiders whose chunk unloads are away, not dead, and join their
      raid again when it loads, after a restart too. Not in vanilla's `raids.dat` format yet.

## Phase 11: Everything else

- [ ] Advancements and statistics. Advancements (`world/advancements.rs`): the 126 of
      26.3's data (the recipe-unlocking ones wait for a recipe book), laid out as trees,
      shown as vanilla shows them (a tab once its root is done, then two levels ahead),
      with toasts, chat announcements (`show_advancement_messages`) and experience
      rewards, saved in `players/advancements/<uuid>.json` in vanilla's format. Criteria
      met: inventory changes, location (biome, dimension), killing mobs, eating and
      drinking, breeding, changing dimension, enchanting, sleeping, trading, curing
      zombie villagers, Hero of the Village, building iron golems, filling buckets.
      Conditions are matched for items, tags, counts, entity types, biomes and
      dimensions; anything else never matches, so those criteria wait (structures such
      as fortresses, damage sources, block placement, taming, totems, and the rest of
      the 50 or so triggers). Statistics (`world/stats.rs`): blocks mined, items
      crafted (grid, stonecutter, smithing), used (blocks placed, food, bows, tools),
      broken, picked up and dropped, mobs and players killed and killed by, and the
      custom ones: play and world time, time since death and rest, sneaking time,
      distances walked, sprinted, crouched, swum, climbed, flown and fallen, jumps,
      deaths, kills, damage dealt, taken and absorbed, beds and straw beds, trades,
      enchanting, breeding, drops, leaving, and opening chests, barrels, ender chests,
      shulker boxes and every workstation. Sent when the statistics screen asks, saved
      in `players/stats/<uuid>.json` as vanilla saves them (both checked against a
      vanilla 26.3 server). Missing: recipe advancements, `/advancement`, the other
      advancement triggers, and riding, boating and fishing statistics.
- [ ] Brigadier command tree with suggestions; console commands. The built-in commands
      are declared to the client (`commands::command_tree`) for tab completion; there is
      no command framework or console input yet.
- [ ] Vanilla commands: /tp, /time, /weather, /gamerule, /give with components,
      /effect, /enchant, /summon, /setblock, /fill, /kill, /clear, /spawnpoint, ...
      Working: /gamemode, /give, /kill, /clear, /tp (coordinates, players, to players),
      /time, /weather, /gamerule, /difficulty, /effect, /xp, /summon, /enchant (the
      item in hand, with the level and compatibility rules), /setblock and /fill
      (`world/commands_extra.rs`; block states like `oak_stairs[facing=north]`;
      destroy, keep, replace, hollow and outline; limited by `max_block_modifications`
      and to loaded chunks), /msg, /tell, /w and /me (for everyone), /setworldspawn,
      /seed, /save-all, and the console commands (op, ban, whitelist, ...). Players are
      picked by name or `@s`, `@a`, `@p`, `@r`, and all of it is offered for
      tab-completion. Missing: other selectors (`@e`, arguments like `[distance=..5]`),
      targets in other dimensions (but /msg), colour and format in /tellraw and /title (text only) (/title works with plain text, quoted strings and {"text":..} only), /data, /execute
      (beyond `in <dimension> run tp`), /locate, /fillbiome, /clone, /scoreboard.
- [x] Game rules and difficulty. Every 26.3 game rule is kept and saved in
      `data/minecraft/game_rules.dat` as vanilla saves it (`rules.rs`), set with
      `/gamerule` in game or at the console (with or without `minecraft:`). Enforced:
      keep_inventory, advance_time, advance_weather, spawn_mobs, spawn_monsters,
      mob_drops, block_drops, fire_spread_radius_around_player (0 stops fire spreading),
      mob_griefing (creeper and fireball blasts), natural_health_regeneration,
      fall_damage, fire_damage, drowning_damage, show_death_messages,
      random_tick_speed, players_sleeping_percentage, immediate_respawn,
      reduced_debug_info and limited_crafting (told to clients).
      Difficulty comes from `server.properties` (`difficulty`, easy by default) and
      `/difficulty`: monsters hit softer on Easy and harder on Hard, starving stops
      at 5 hearts on Easy and kills on Hard, fire spreads more readily the harder
      the game, and Peaceful has no monsters and no hunger.
- [ ] Scoreboards, teams, boss bars, titles
- [ ] Item components on the network (enchantments, names, damage, ...). Damage,
      enchantments, stored enchantments, repair cost, custom names, armor trims and
      potion contents are kept, saved and sent; other components are dropped.
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
      Pumpkin targets 26.x, and every block and biome it places is a 26.3 one.
- [x] Features: trees, ores, flowers, lakes, ...
- [x] Structures: villages, strongholds, mineshafts, temples, monuments, ...
      Pumpkin places them along with their block entities, which come out with each
      generated chunk. Chests, barrels and dispensers naming a loot table are filled
      from vanilla's chest tables when their chunk arrives (`loot.rs`: random slots,
      split stacks, enchanted and worn gear, enchanted books); spawners keep the mob
      the generator gave them. Saved chests from vanilla worlds that were never opened
      are filled when loaded. Treasure and explorer maps in chests are drawn to the
      nearest of their structure (and left out when there is none, as vanilla does);
      loot sets potions, stew effects, ominous bottle levels, goat horn instruments and
      enchantments. Suspicious sand and gravel hold archaeology loot that brushing brings
      out (`world/archaeology.rs`); decorated pots keep their sherds and a stack, and
      trial chamber pots are filled from their table; mineshafts' chest minecarts come
      with their loot table, rolled when opened or broken. Known gap: abandoned camp
      maps (Pumpkin does not generate abandoned camps) and set_name's item names.
- [x] Nether and End generation, by Pumpkin like the overworld (see Phase 9).
- [x] Loading existing Leaf/vanilla worlds (pre-generated terrain). Chunks, entities,
      block entities, players, statistics and advancements saved by vanilla or Leaf
      load as they are; blocks and items renamed since (`chain` to `iron_chain`,
      `grass` to `short_grass`) take their new names (`levitas_registry::current_name`),
      where unknown ones would become air. Opening a world moves it into 26.3's layout,
      Paper and Leaf's `world_nether/DIM-1` and `world_the_end/DIM1` included (checked
      on a world made by a vanilla 1.21.8 server). Items saved before 1.20.5 (`Count`
      and `tag`) are read too: damage, enchantments, stored enchantments, repair cost,
      names, dye colours, potions, maps, trims and lodestone targets. Known gaps:
      worlds from before 1.13 (numeric IDs) are not read; Pumpkin's terrain at the edge of
      the old world may not line up exactly with what vanilla would have generated;
      Leaf's own plugin data is not read.

## Phase 13: Plugins and economy

- [ ] Plugin API: events, commands, permissions, scheduler, storage, GUIs
- [ ] Native Rust and WASM plugins
- [ ] Economy service; first plugins: permissions, economy, homes/warps/tpa, shops, claims

## Infrastructure

- [x] CI: fmt, clippy and tests on every push (`.github/workflows/ci.yml` runs the data
      generator with Java 25, caching `generated/`, then fmt, clippy and the tests)
- [ ] Region-based parallel ticking (see ARCHITECTURE.md)
- [ ] Benchmarks against Leaf: memory per player, tick time, chunk throughput
