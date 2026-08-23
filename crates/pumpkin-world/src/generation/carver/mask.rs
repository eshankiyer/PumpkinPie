use pumpkin_util::math::position::BlockPos;

type AdditionalMaskPredicate = Box<dyn Fn(i32, i32, i32) -> bool + Send + Sync>;

/// A per-chunk bit mask of carved block positions, mirroring vanilla's
/// `CarvingMask` (`net/minecraft/world/level/chunk/CarvingMask.java`).
pub struct CarvingMask {
    min_y: i32,
    mask: Vec<u64>,
    /// Vanilla's `additionalMask` field is an arbitrary `(x, y, z) -> bool` predicate
    /// (`CarvingMask.Mask`, `CarvingMask.java:11,18,50-52`), defaulting to `(x, y, z) -> false`.
    /// Its one real caller (`Blender.addAroundOldChunksCarvingMaskFilter`,
    /// `Blender.java:337-343`) feeds a noise-based distance-to-old-chunk-edge closure, not
    /// another `CarvingMask`, so this is stored as a closure rather than a second bitset.
    additional_mask: Option<AdditionalMaskPredicate>,
}

impl CarvingMask {
    #[must_use]
    pub fn new(height: i32, min_y: i32) -> Self {
        // Each u64 stores 64 bits. Total bits needed: 16 * 16 * height.
        let total_bits = 16 * 16 * height;
        let num_u64s = (total_bits as usize).div_ceil(64);
        Self {
            min_y,
            mask: vec![0; num_u64s],
            additional_mask: None,
        }
    }

    const fn get_bit_index(&self, x: i32, y: i32, z: i32) -> usize {
        // x: 0..16, z: 0..16, y: min_y..min_y+height
        let rel_x = x & 15;
        let rel_z = z & 15;
        let rel_y = y - self.min_y;
        (rel_x | (rel_z << 4) | (rel_y << 8)) as usize
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32) {
        let bit_index = self.get_bit_index(x, y, z);
        let u64_idx = bit_index / 64;
        let bit_in_u64 = bit_index % 64;
        self.mask[u64_idx] |= 1 << bit_in_u64;
    }

    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> bool {
        let bit_index = self.get_bit_index(x, y, z);
        let u64_idx = bit_index / 64;
        let bit_in_u64 = bit_index % 64;
        // Mirrors vanilla `CarvingMask.get`: `additionalMask.test(x, y, z) || mask.get(...)`
        // (`net/minecraft/world/level/chunk/CarvingMask.java:35-37`).
        if let Some(additional) = &self.additional_mask
            && additional(x, y, z)
        {
            return true;
        }
        (self.mask[u64_idx] >> bit_in_u64) & 1 != 0
    }

    /// Sets the additional-mask predicate, mirroring vanilla
    /// `CarvingMask.setAdditionalMask` (`net/minecraft/world/level/chunk/CarvingMask.java:18`).
    pub fn set_additional_mask(
        &mut self,
        predicate: impl Fn(i32, i32, i32) -> bool + Send + Sync + 'static,
    ) {
        self.additional_mask = Some(Box::new(predicate));
    }

    /// Returns the raw primary mask bits, mirroring vanilla `CarvingMask.toArray`
    /// (`net/minecraft/world/level/chunk/CarvingMask.java:48`).
    #[must_use]
    pub fn to_array(&self) -> Vec<u64> {
        self.mask.clone()
    }

    /// Returns every carved position in this chunk as a `BlockPos` list, mirroring
    /// vanilla `CarvingMask.stream` (`net/minecraft/world/level/chunk/CarvingMask.java:39`).
    #[must_use]
    pub fn stream(&self, chunk_x: i32, chunk_z: i32) -> Vec<BlockPos> {
        let mut positions = Vec::new();
        for (word_idx, &word) in self.mask.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let i = word_idx * 64 + bit;
                let x = (i & 15) as i32;
                let z = ((i >> 4) & 15) as i32;
                let y = (i >> 8) as i32 + self.min_y;
                positions.push(BlockPos::new(chunk_x * 16 + x, y, chunk_z * 16 + z));
            }
        }
        positions
    }
}
