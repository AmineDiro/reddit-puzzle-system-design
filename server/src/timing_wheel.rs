use crate::const_settings::TIMING_WHEEL_TICKS;
use crate::cooldown::CooldownArray;

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
        // So they will expire TIMING_WHEEL_TICKS ticks from now.
        self.wheel[self.current_tick].set_cooldown(local_id);
    }
}

impl Default for TimingWheel {
    fn default() -> Self {
        Self::new()
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

        // ticking TIMING_WHEEL_TICKS-1 times shouldn't clear it
        for _ in 0..TIMING_WHEEL_TICKS - 1 {
            wheel.tick(&mut master);
            assert!(master.is_on_cooldown(55));
        }

        // Next tick should clear it
        wheel.tick(&mut master);
        assert!(!master.is_on_cooldown(55));
    }
}
