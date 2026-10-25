mod connection;
mod flat;
mod inventory;
mod loot;
mod play;
mod recipes;
mod rng;
#[cfg(test)]
mod tests;
mod world;

use std::io;
use std::net::SocketAddr;

use levitas_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

/// Localhost only for now, so testing does not open the machine to the network.
const DEFAULT_ADDR: &str = "127.0.0.1:25565";

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
    let world = world::spawn();
    let listener = TcpListener::bind(addr).await?;
    info!(
        "Levitas {} for Minecraft {GAME_VERSION} (protocol {PROTOCOL_VERSION}) listening on {addr}",
        env!("CARGO_PKG_VERSION")
    );

    loop {
        let (stream, peer) = match listener.accept().await {
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
