//! Light levels of a chunk: sky light and block light for every 16x16x16 section, one
//! nibble per block. Light covers one extra section below and one above the world.
//!
//! Sections where every block has the same level store just that level, so the usual
//! case (full daylight above ground, darkness deep below) costs almost nothing.

use levitas_protocol::varint;

use crate::chunk::SECTIONS;

/// Light sections: the world's sections plus one below and one above.
pub const LIGHT_SECTIONS: usize = SECTIONS + 2;
const NIBBLE_BYTES: usize = 2048;

/// One section's light levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightLayer {
    Uniform(u8),
    Nibbles(Box<[u8; NIBBLE_BYTES]>),
}

impl LightLayer {
    fn index(x: usize, y: usize, z: usize) -> usize {
        (y * 16 + z) * 16 + x
    }

    /// A layer from the protocol's or Anvil's 2048-byte nibble array.
    pub fn from_nibbles(bytes: &[u8]) -> Self {
        let Ok(array) = <[u8; NIBBLE_BYTES]>::try_from(bytes) else {
            return LightLayer::Uniform(0);
        };
        let first = array[0];
        if first & 0x0F == first >> 4 && array.iter().all(|&b| b == first) {
            LightLayer::Uniform(first & 0x0F)
        } else {
            LightLayer::Nibbles(Box::new(array))
        }
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        match self {
            LightLayer::Uniform(level) => *level,
            LightLayer::Nibbles(bytes) => {
                let i = Self::index(x, y, z);
                (bytes[i >> 1] >> ((i & 1) * 4)) & 0x0F
            }
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, level: u8) {
        let level = level.min(15);
        if let LightLayer::Uniform(current) = *self {
            if current == level {
                return;
            }
            *self = LightLayer::Nibbles(Box::new([current * 0x11; NIBBLE_BYTES]));
        }
        if let LightLayer::Nibbles(bytes) = self {
            let i = Self::index(x, y, z);
            let shift = (i & 1) * 4;
            bytes[i >> 1] = (bytes[i >> 1] & !(0x0F << shift)) | (level << shift);
        }
    }

    fn write_nibbles(&self, out: &mut Vec<u8>) {
        match self {
            LightLayer::Uniform(level) => out.resize(out.len() + NIBBLE_BYTES, level * 0x11),
            LightLayer::Nibbles(bytes) => out.extend_from_slice(&bytes[..]),
        }
    }
}

/// The light of a whole chunk column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLight {
    pub sky: Vec<LightLayer>,
    pub block: Vec<LightLayer>,
}

impl ChunkLight {
    /// Full daylight everywhere and no block light: what a chunk without computed light
    /// gets.
    pub fn full_sky() -> Self {
        ChunkLight {
            sky: vec![LightLayer::Uniform(15); LIGHT_SECTIONS],
            block: vec![LightLayer::Uniform(0); LIGHT_SECTIONS],
        }
    }

    /// The light part of a chunk packet: which sections have light, which are dark, then
    /// the nibble arrays of the lit ones, for sky light and block light.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let masks = |layers: &[LightLayer]| {
            let mut lit = 0u64;
            let mut dark = 0u64;
            for (i, layer) in layers.iter().enumerate() {
                if *layer == LightLayer::Uniform(0) {
                    dark |= 1 << i;
                } else {
                    lit |= 1 << i;
                }
            }
            (lit, dark)
        };
        let bitset = |out: &mut Vec<u8>, bits: u64| {
            if bits == 0 {
                varint::write(out, 0);
            } else {
                varint::write(out, 1);
                out.extend_from_slice(&bits.to_be_bytes());
            }
        };
        let (sky_lit, sky_dark) = masks(&self.sky);
        let (block_lit, block_dark) = masks(&self.block);
        bitset(out, sky_lit);
        bitset(out, block_lit);
        bitset(out, sky_dark);
        bitset(out, block_dark);
        for layers in [&self.sky, &self.block] {
            let lit: Vec<&LightLayer> = layers
                .iter()
                .filter(|l| **l != LightLayer::Uniform(0))
                .collect();
            varint::write(out, lit.len() as i32);
            for layer in lit {
                varint::write(out, NIBBLE_BYTES as i32);
                layer.write_nibbles(out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layers_stay_small_until_they_vary() {
        let mut layer = LightLayer::Uniform(15);
        layer.set(1, 2, 3, 15);
        assert_eq!(layer, LightLayer::Uniform(15));
        layer.set(1, 2, 3, 7);
        assert_eq!(layer.get(1, 2, 3), 7);
        assert_eq!(layer.get(0, 2, 3), 15);
        assert_eq!(layer.get(2, 2, 3), 15);
        let mut bytes = Vec::new();
        layer.write_nibbles(&mut bytes);
        assert_eq!(LightLayer::from_nibbles(&bytes), layer);
        assert_eq!(
            LightLayer::from_nibbles(&[0x99; 2048]),
            LightLayer::Uniform(9)
        );
    }

    #[test]
    fn full_sky_encoding_matches_the_old_constant() {
        let mut out = Vec::new();
        ChunkLight::full_sky().encode(&mut out);
        // Sky mask (1 long), empty block mask, empty sky-dark mask, block-dark mask (1 long),
        // 26 sky arrays, no block arrays.
        let header = 1 + 8 + 1 + 1 + 1 + 8;
        let arrays = 1 + LIGHT_SECTIONS * (2 + NIBBLE_BYTES) + 1;
        assert_eq!(out.len(), header + arrays);
        assert_eq!(out[header + 3], 0xFF);
    }
}
