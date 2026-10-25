# Roadmap

Each milestone ends with something that can be tested in a real 1.21.8 client.

## Milestone 0: Foundation

- [x] Project name, license (GPL-3.0), clean-room policy
- [x] Cargo workspace and crate skeletons
- [x] Rust toolchain installed, workspace builds
- [ ] CI (fmt, clippy, test)
- [ ] Data-generator pipeline for `levitas-registry`

## Milestone 1: Join the server

- [x] Handshake, Status (server list ping with MOTD and player count)
- [ ] Login (offline mode first, then online mode with encryption)
- [ ] Configuration phase: registries, tags, known packs
- [ ] Load an existing Anvil world and send chunks around the player
- [ ] Movement, keep-alive, player list, chat

## Milestone 2: Interaction

- [ ] Break and place blocks, block updates to other players
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
