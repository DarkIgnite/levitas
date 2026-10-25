//! Network protocol for Java Edition 1.21.8.

mod codec;
pub mod varint;

pub use codec::{DecodeError, Reader, Writer};

/// Protocol version number sent by 1.21.7 and 1.21.8 clients in the handshake.
pub const PROTOCOL_VERSION: i32 = 772;

/// Game version this protocol implementation targets.
pub const GAME_VERSION: &str = "1.21.8";

/// Largest packet the protocol allows: the length prefix is at most a 3-byte VarInt.
pub const MAX_PACKET_LEN: usize = 2_097_151;
