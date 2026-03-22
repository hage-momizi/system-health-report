use anyhow::Result;
use serde::{Deserialize, Serialize};
use sysinfo::{Disks, System};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::config::SystemMetrics as SystemMetricsConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub average_usage: f32,
    pub interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_mb: u64,
    pub used_mb: u64,
    pub available_mb: u64,
    pub usage_percent: f32,
    pub swap_total_mb: u64,
    pub swap_used_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadAverageMetrics {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub mount_point: String,
    pub total_gb: f64,
    pub used_gb: f64,
    pub available_gb: f64,
    pub usage_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu: Option<CpuMetrics>,
    pub memory: Option<MemoryMetrics>,
    pub load_average: Option<LoadAverageMetrics>,
    pub disks: Vec<DiskMetrics>,
    pub collected_at: String,
}

pub async fn collect_metrics(config: &SystemMetricsConfig) -> Result<SystemMetrics> {
    info!("Starting system metrics collection");

    let cpu = if config.cpu.enabled {
        Some(collect_cpu(config.cpu.interval_seconds).await)
    } else {
        None
    };

    let mut sys = System::new_all();
    sys.refresh_all();

    let memory = if config.memory.enabled {
        Some(collect_memory(&sys))
    } else {
        None
    };

    let load_average = if config.load_average.enabled {
        Some(collect_load_average())
    } else {
        None
    };

    let disks = if config.disk_status.enabled {
        collect_disks(&config.disk_status.mount_points)
    } else {
        vec![]
    };

    let collected_at = chrono::Utc::now().to_rfc3339();

    Ok(SystemMetrics {
        cpu,
        memory,
        load_average,
        disks,
        collected_at,
    })
}

async fn collect_cpu(interval_seconds: u64) -> CpuMetrics {
    info!("Collecting CPU metrics over {} seconds", interval_seconds);
    let mut sys = System::new_all();
    let mut samples: Vec<f32> = Vec::new();

    let sample_count = interval_seconds.max(1);

    for _ in 0..sample_count {
        sys.refresh_cpu_usage();
        let usage = sys.global_cpu_usage();
        samples.push(usage);
        sleep(Duration::from_secs(1)).await;
    }

    let average = if samples.is_empty() {
        0.0
    } else {
        samples.iter().sum::<f32>() / samples.len() as f32
    };

    CpuMetrics {
        average_usage: average,
        interval_seconds,
    }
}

fn collect_memory(sys: &System) -> MemoryMetrics {
    let total = sys.total_memory();
    let used = sys.used_memory();
    let available = sys.available_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();

    let usage_percent = if total > 0 {
        (used as f32 / total as f32) * 100.0
    } else {
        0.0
    };

    MemoryMetrics {
        total_mb: total / 1024 / 1024,
        used_mb: used / 1024 / 1024,
        available_mb: available / 1024 / 1024,
        usage_percent,
        swap_total_mb: swap_total / 1024 / 1024,
        swap_used_mb: swap_used / 1024 / 1024,
    }
}

fn collect_load_average() -> LoadAverageMetrics {
    let load = System::load_average();
    LoadAverageMetrics {
        one: load.one,
        five: load.five,
        fifteen: load.fifteen,
    }
}

fn collect_disks(mount_points: &[String]) -> Vec<DiskMetrics> {
    let disks = Disks::new_with_refreshed_list();
    let mut result = Vec::new();

    for mount_point in mount_points {
        let mut found = false;
        for disk in disks.list() {
            let disk_mount = disk.mount_point().to_string_lossy().to_string();
            if disk_mount == *mount_point {
                let total = disk.total_space();
                let available = disk.available_space();
                let used = total.saturating_sub(available);
                let usage_percent = if total > 0 {
                    (used as f32 / total as f32) * 100.0
                } else {
                    0.0
                };

                result.push(DiskMetrics {
                    mount_point: mount_point.clone(),
                    total_gb: total as f64 / 1024.0 / 1024.0 / 1024.0,
                    used_gb: used as f64 / 1024.0 / 1024.0 / 1024.0,
                    available_gb: available as f64 / 1024.0 / 1024.0 / 1024.0,
                    usage_percent,
                });
                found = true;
                break;
            }
        }

        if !found {
            warn!("Mount point {} not found", mount_point);
        }
    }

    result
}
