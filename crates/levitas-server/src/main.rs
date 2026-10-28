mod blocks;
mod click;
mod connection;
mod enchanting;
mod flat;
mod inventory;
mod loot;
mod play;
mod recipes;
mod rng;
mod terrain;
#[cfg(test)]
mod tests;
mod world;

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use levitas_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

/// Localhost only for now, so testing does not open the machine to the network.
const DEFAULT_ADDR: &str = "127.0.0.1:25565";
/// The world folder, unless `LEVITAS_WORLD` names another.
const DEFAULT_WORLD: &str = "world";

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let addr = std::env::args().nth(1);
    let addr: SocketAddr = addr
        .as_deref()
        .unwrap_or(DEFAULT_ADDR)
        .parse()
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid listen address: {e}"),
            )
        })?;
    let world_dir = std::env::var_os("LEVITAS_WORLD")
        .map_or_else(|| PathBuf::from(DEFAULT_WORLD), PathBuf::from);
    // New worlds get generated terrain unless LEVITAS_WORLD_TYPE=flat asks for superflat.
    let kind = match std::env::var("LEVITAS_WORLD_TYPE").as_deref() {
        Ok("flat") => world::WorldKind::Flat,
        _ => world::WorldKind::Noise,
    };
    let seed = std::env::var("LEVITAS_SEED")
        .ok()
        .and_then(|s| s.parse().ok());
    let world = world::spawn(world_dir, world::NewWorld { kind, seed });
    let listener = TcpListener::bind(addr).await?;
    info!(
        "Levitas {} for Minecraft {GAME_VERSION} (protocol {PROTOCOL_VERSION}) listening on {addr}",
        env!("CARGO_PKG_VERSION")
    );

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
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
