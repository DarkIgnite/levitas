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

- [ ] Health, damage, hunger, death and respawn
- [ ] Crafting, furnace, chests
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
