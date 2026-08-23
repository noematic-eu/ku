use sysinfo::System;

#[derive(Debug, Clone, Default)]
pub struct CpuSnapshot {
    pub global: f32,
    pub brand: String,
    pub cores: Vec<CoreSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub struct CoreSnapshot {
    pub name: String,
    pub usage: f32,
    pub frequency_mhz: u64,
}

pub fn collect(sys: &System) -> CpuSnapshot {
    let cores = sys
        .cpus()
        .iter()
        .map(|cpu| CoreSnapshot {
            name: cpu.name().to_string(),
            usage: cpu.cpu_usage(),
            frequency_mhz: cpu.frequency(),
        })
        .collect();
    let brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_default();
    CpuSnapshot {
        global: sys.global_cpu_usage(),
        brand,
        cores,
    }
}
