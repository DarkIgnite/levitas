# Levitas

*Latin: "lightness".*

Levitas is a lightweight, multithreaded game server for Minecraft: Java Edition **1.21.8**
(protocol 772), written from scratch in Rust.

The goal is a server that behaves like vanilla and runs on a fraction of the memory and
CPU that a JVM-based server (Paper, Purpur, Leaf, ...) needs.

> **Status:** early development. Not playable yet. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Goals

- **Light:** no JVM, no garbage collector, a small memory footprint at idle and per player.
- **Parallel:** region-based ticking so distant parts of the world use separate CPU cores.
- **Faithful:** vanilla-compatible gameplay, verified against a reference server.
- **Extensible:** a plugin API for native Rust and sandboxed WASM plugins.
- **Measured:** every performance claim backed by a reproducible benchmark.

## Building

Requires a stable Rust toolchain (install via [rustup](https://rustup.rs)).

```sh
cargo build --release
cargo run --release -p levitas-server
```

## Clean-room policy

Levitas does **not** contain code copied, translated, or decompiled from Minecraft, Paper,
or any of its forks. Behaviour is implemented from public protocol documentation, data
produced by the official server's data generator, and observation of a running server.
Contributors must follow [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md).

Levitas is not an official Minecraft product and is not approved by or associated with
Mojang or Microsoft.

## License

[GPL-3.0-only](LICENSE)
