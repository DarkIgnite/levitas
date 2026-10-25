//! Small value types shared by many packets.

/// A block position, sent as one long: x (26 bits), z (26 bits), y (12 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn from_packed(v: i64) -> Self {
        // Arithmetic shifts sign-extend each field.
        Self {
            x: (v >> 38) as i32,
            y: (v << 52 >> 52) as i32,
            z: (v << 26 >> 38) as i32,
        }
    }

    pub fn packed(self) -> i64 {
        ((i64::from(self.x) & 0x3FF_FFFF) << 38)
            | ((i64::from(self.z) & 0x3FF_FFFF) << 12)
            | (i64::from(self.y) & 0xFFF)
    }

    /// The neighbouring position on `face` (0 down, 1 up, 2 north, 3 south, 4 west, 5 east).
    pub fn offset(self, face: i32) -> Self {
        let (dx, dy, dz) = match face {
            0 => (0, -1, 0),
            1 => (0, 1, 0),
            2 => (0, 0, -1),
            3 => (0, 0, 1),
            4 => (-1, 0, 0),
            5 => (1, 0, 0),
            _ => (0, 0, 0),
        };
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    /// The chunk column containing this block.
    pub fn chunk(self) -> (i32, i32) {
        (self.x >> 4, self.z >> 4)
    }
}

/// A rotation in degrees as the protocol's one-byte angle (256 steps per turn).
pub fn angle(degrees: f32) -> u8 {
    (degrees.rem_euclid(360.0) / 360.0 * 256.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_round_trips_negative_coordinates() {
        for pos in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(-1, -64, -1),
            BlockPos::new(18_357_644, 831, -20_882_616),
            BlockPos::new(-33_554_432, -2048, 33_554_431),
        ] {
            assert_eq!(BlockPos::from_packed(pos.packed()), pos);
        }
    }

    #[test]
    fn chunk_of_negative_block() {
        assert_eq!(BlockPos::new(-1, 0, 16).chunk(), (-1, 1));
    }

    #[test]
    fn angles() {
        assert_eq!(angle(0.0), 0);
        assert_eq!(angle(90.0), 64);
        assert_eq!(angle(-90.0), 192);
    }
}
