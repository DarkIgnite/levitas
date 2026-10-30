//! Network protocol for Java Edition 26.3.

mod codec;
mod types;
pub mod varint;

pub use codec::{DecodeError, Reader, Writer};
pub use types::{angle, BlockPos};

/// Protocol version number sent by 26.3 clients in the handshake.
pub const PROTOCOL_VERSION: i32 = 777;

/// Game version this protocol implementation targets.
pub const GAME_VERSION: &str = "26.3";

/// Largest packet the protocol allows: the length prefix is at most a 3-byte VarInt.
pub const MAX_PACKET_LEN: usize = 2_097_151;
