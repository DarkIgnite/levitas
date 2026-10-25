# Architecture

This document describes the intended design. It is updated as the code lands; where it
disagrees with the code, the code wins and this file should be fixed.

## Design principles

1. **Data-oriented, not object-oriented.** Vanilla's deep class hierarchies
   (`Entity → LivingEntity → Mob → Zombie`) are not reproduced. Entities are plain data in
   an ECS; behaviour lives in systems.
2. **No shared mutable graphs.** No `Arc<Mutex<_>>` webs between entities, chunks and the
   world. Ownership is by region; cross-region effects go through message queues.
3. **Allocate rarely.** Packet encoding writes into reused buffers; chunks use paletted
   storage; hot paths avoid per-tick allocation.
4. **Measure everything.** Benchmarks run against a Leaf 1.21.8 baseline on the same machine.

## Crate layout

```
crates/
  levitas-server     binary: config, startup, main loop, console
  levitas-protocol   packet types, VarInt/strings/positions, encode/decode, compression, encryption
  levitas-nbt        NBT reader/writer (network + file variants)
  levitas-registry   blocks, items, biomes, ... generated from data-generator JSON at build time
  levitas-world      chunks, paletted containers, Anvil (.mca) region I/O, lighting
```

Planned later: `levitas-entity` (ECS components and systems), `levitas-plugin-api`,
`levitas-plugin-wasm`, `levitas-bench`.

## Threading model

- **Network:** `tokio` runtime. One task per connection for reading/decoding; outgoing
  packets are queued per player and flushed once per tick in batches.
- **World task (today):** one task owns all chunks and players. Connection tasks send it
  events over a channel and receive ready-to-write packet frames (`Arc<[u8]>`, shared
  between recipients), so world state needs no locks. Splitting it into regions is the
  next step below.
- **Game ticks:** 20 TPS. The world is split into regions (groups of chunks). Each tick,
  independent regions are ticked in parallel on a worker pool (`rayon`). Regions that are
  close to each other are merged so that interactions inside one tick stay single-threaded.
- **Chunk I/O and generation:** off the tick threads, on a dedicated pool. Results are
  handed back to the owning region at the next tick boundary.

## Data generation

Block states, items, registries and packet IDs are not hand-written. `levitas-registry`
reads the JSON reports produced by the official server's data generator and emits Rust
code from a build script. Generated output is never committed (see CLEAN_ROOM.md).

## Plugins

Two kinds, sharing one API surface:

- **Native Rust** plugins compiled into the server or loaded as dynamic libraries. Fastest,
  but tied to the server version.
- **WASM** plugins run in `wasmtime`. Sandboxed, hot-reloadable, can be written in any
  language that targets WASM.

The API exposes events, commands, permissions, a scheduler, inventory GUIs, persistent
storage, and an economy service (the Vault equivalent).

## Benchmarks to track

| Metric | How |
|---|---|
| RSS at idle (spawn chunks loaded) | process memory after startup |
| RSS per connected player | bots connected via a load tester |
| MSPT at N players | tick time histogram |
| Chunk load throughput | chunks/s from Anvil, cold and warm |
