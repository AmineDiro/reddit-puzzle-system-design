// 52,631 bits / 8 = ~6.5 KB. We'll use 823 u64s which gives 52672 bits.
pub const COOLDOWN_ARRAY_LEN: usize = 1024;

#[derive(Clone)]
pub struct CooldownArray {
    pub bits: [u64; COOLDOWN_ARRAY_LEN],
}

impl CooldownArray {
    pub fn new() -> Self {
        Self {
            bits: [0; COOLDOWN_ARRAY_LEN],
        }
    }

    #[inline(always)]
    pub fn is_on_cooldown(&self, local_id: u32) -> bool {
        let chunk_idx = (local_id >> 6) as usize;
        let bit_mask = 1 << (local_id & 63);
        (self.bits[chunk_idx] & bit_mask) != 0
    }

    #[inline(always)]
    pub fn set_cooldown(&mut self, local_id: u32) {
        let chunk_idx = (local_id >> 6) as usize;
        let bit_mask = 1 << (local_id & 63);
        self.bits[chunk_idx] |= bit_mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cooldown_array() {
        let mut arr = CooldownArray::new();
        assert!(!arr.is_on_cooldown(10));
        assert!(!arr.is_on_cooldown(52000));

        arr.set_cooldown(10);
        arr.set_cooldown(52000);

        assert!(arr.is_on_cooldown(10));
        assert!(arr.is_on_cooldown(52000));
        assert!(!arr.is_on_cooldown(11));
        assert!(!arr.is_on_cooldown(52001));
    }
}
