use sysinfo::System;

#[derive(Debug, Clone, Default)]
pub struct MemorySnapshot {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

impl MemorySnapshot {
    pub fn used_pct(&self) -> f64 {
        crate::utils::percent(self.used, self.total)
    }

    pub fn swap_pct(&self) -> f64 {
        crate::utils::percent(self.swap_used, self.swap_total)
    }
}

pub fn collect(sys: &System) -> MemorySnapshot {
    MemorySnapshot {
        total: sys.total_memory(),
        used: sys.used_memory(),
        available: sys.available_memory(),
        swap_total: sys.total_swap(),
        swap_used: sys.used_swap(),
    }
}
