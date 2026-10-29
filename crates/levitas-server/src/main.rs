mod access;
mod auth;
mod blocks;
mod click;
mod config;
mod connection;
mod enchanting;
mod flat;
mod inventory;
mod loot;
mod play;
mod potions;
mod recipes;
mod repair;
mod rng;
mod rules;
mod terrain;
#[cfg(test)]
mod tests;
mod world;

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use levitas_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    // Settings come from server.properties (written with defaults if missing); the
    // first argument and the LEVITAS_* variables override them.
    let config = config::load(Path::new("."));
    access::load(PathBuf::from("."), config.white_list);
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("{}:{}", config.server_ip, config.server_port));
    let addr: SocketAddr = addr.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid listen address {addr}: {e}"),
        )
    })?;
    let world_dir = std::env::var_os("LEVITAS_WORLD")
        .map_or_else(|| PathBuf::from(&config.level_name), PathBuf::from);
    let flat = match std::env::var("LEVITAS_WORLD_TYPE") {
        Ok(kind) => kind == "flat",
        Err(_) => matches!(config.level_type.as_str(), "minecraft:flat" | "flat"),
    };
    let kind = if flat {
        world::WorldKind::Flat
    } else {
        world::WorldKind::Noise
    };
    let seed = std::env::var("LEVITAS_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .or(config.level_seed);
    let world = world::spawn(world_dir, world::NewWorld { kind, seed });
    let listener = TcpListener::bind(addr).await?;
    info!(
        "Levitas {} for Minecraft {GAME_VERSION} (protocol {PROTOCOL_VERSION}) listening on {addr}",
        env!("CARGO_PKG_VERSION")
    );

    // Lines typed into the console are commands; `stop` stops the server.
    let (console_tx, mut console) = tokio::sync::mpsc::channel::<String>(16);
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if console_tx.blocking_send(line).is_err() {
                break;
            }
        }
    });

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            Some(line) = console.recv() => {
                if line.trim().trim_start_matches('/') != "stop" {
                    let _ = world.send(world::Event::Console { line }).await;
                    continue;
                }
                info!("stopping: saving the world");
                let (done, saved) = oneshot::channel();
                if world.send(world::Event::Shutdown { done }).await.is_ok() {
                    let _ = saved.await;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                info!("stopped");
                return Ok(());
            }
            _ = tokio::signal::ctrl_c() => {
                info!("stopping: saving the world");
                let (done, saved) = oneshot::channel();
                if world.send(world::Event::Shutdown { done }).await.is_ok() {
                    let _ = saved.await;
                }
                // Give connections a moment to deliver the disconnect message.
                tokio::time::sleep(Duration::from_millis(200)).await;
                info!("stopped");
                return Ok(());
            }
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(e) => {
                warn!("failed to accept connection: {e}");
                continue;
            }
        };
        let world = world.clone();
        tokio::spawn(async move {
            if let Err(e) = connection::handle(stream, world).await {
                debug!(%peer, "connection closed: {e}");
            }
        });
    }
}
