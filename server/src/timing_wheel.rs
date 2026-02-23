use crate::cooldown::CooldownArray;

// 300 ticks for a 5-minute cooldown (1 tick per second)
pub const TIMING_WHEEL_TICKS: usize = 300;

pub struct TimingWheel {
    pub wheel: [CooldownArray; TIMING_WHEEL_TICKS],
    pub current_tick: usize,
}

impl TimingWheel {
    pub fn new() -> Self {
        Self {
            wheel: std::array::from_fn(|_| CooldownArray::new()),
            current_tick: 0,
        }
    }

    #[inline(always)]
    pub fn tick(&mut self, master: &mut CooldownArray) {
        self.current_tick = (self.current_tick + 1) % TIMING_WHEEL_TICKS;

        let expiring_users = &mut self.wheel[self.current_tick];

        // SIMD Vectorized mass eviction (AND NOT)
        for (master_chunk, expiring_chunk) in
            master.bits.iter_mut().zip(expiring_users.bits.iter_mut())
        {
            *master_chunk &= !*expiring_chunk;
            *expiring_chunk = 0; // Wipe bucket for future use in one pass
        }
    }

    #[inline(always)]
    pub fn add_cooldown(&mut self, local_id: u32) {
        // Find bucket that is basically just before current tick
        // So they will expire 300 ticks from now.
        self.wheel[self.current_tick].set_cooldown(local_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timing_wheel() {
        let mut master = CooldownArray::new();
        let mut wheel = TimingWheel::new();

        master.set_cooldown(55);
        wheel.add_cooldown(55);

        // ticking 299 times shouldn't clear it
        for _ in 0..299 {
            wheel.tick(&mut master);
            assert!(master.is_on_cooldown(55));
        }

        // 300th tick should clear it
        wheel.tick(&mut master);
        assert!(!master.is_on_cooldown(55));
    }
}
