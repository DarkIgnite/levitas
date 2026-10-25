//! Chunks and paletted block/biome storage. Anvil (.mca) region I/O comes later.

pub mod chunk;
pub mod paletted;

pub use chunk::{is_air, Chunk, MAX_Y, MIN_Y};
pub use paletted::{Kind, PalettedContainer};
