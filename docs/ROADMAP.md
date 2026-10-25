# Roadmap

Each milestone ends with something that can be tested in a real 1.21.8 client.

## Milestone 0: Foundation

- [x] Project name, license (GPL-3.0), clean-room policy
- [x] Cargo workspace and crate skeletons
- [x] Rust toolchain installed, workspace builds
- [ ] CI (fmt, clippy, test)
- [x] Data-generator pipeline for `levitas-registry`

## Milestone 1: Join the server

- [x] Handshake, Status (server list ping with MOTD and player count)
- [x] Login, offline mode
- [ ] Online mode (encryption, Mojang session check) and compression
- [x] Configuration phase: registries, tags, known packs
- [x] Flat test world, chunks streamed around the player as they move
- [ ] Load an existing Anvil world
- [x] Movement, keep-alive, own tab-list entry, chat echo
- [x] Shared world: players see each other move and swing, chat broadcast

## Milestone 2: Interaction

- [x] Break and place blocks (creative), block updates to other players
- [ ] Placement orientation (stairs, logs, slabs, doors, ...) instead of default states
- [ ] Inventory, creative and survival item handling, item drops
- [ ] Commands (Brigadier command tree), console
- [ ] Saving chunks and player data back to Anvil

## Milestone 3: Basic survival

- [x] Survival mode: mining with loot-table drops and correct-tool checks
- [x] Dropped items: physics, pickup, Q / click-outside throwing, death drops
- [x] Health, fall/void/starvation damage, hunger, regeneration, eating
- [x] Death screen and respawn
- [x] Server-side player inventory (container clicks trusted, not yet simulated)
- [x] Commands: /gamemode, /give, /kill
- [ ] Mining speed validation (needs block hardness data)
- [x] Crafting in the 2x2 grid and crafting tables (shaped, mirrored, shapeless)
- [x] Chests and barrels: shared contents, lid animation, contents drop when broken
- [x] Furnaces: fuel, smelting recipes, progress bars, lit block state
- [x] Click validation: items cannot appear except a crafting result whose ingredients are used up
- [ ] Server-side click simulation (shift-click crafting several at once, drag splitting)
- [ ] Special recipes (dyeing, fireworks, map cloning), stonecutter, smithing, blast furnace, smoker
- [ ] Double chests and block orientation when placing
- [ ] Day/night cycle, weather

## Milestone 4: Entities and mobs

- [ ] ECS, entity tracking, physics and collisions
- [ ] Mob spawning, basic AI, combat, drops

## Milestone 5: Plugin API

- [ ] Events, commands, permissions, scheduler, storage, GUIs
- [ ] Economy service
- [ ] First plugins: permissions, economy, homes/warps/tpa, shops, claims

## Milestone 6: The rest of vanilla

- [ ] Redstone, fluids, lighting updates
- [ ] World generation (overworld, nether, end)
- [ ] Villagers, enchanting, brewing, structures
