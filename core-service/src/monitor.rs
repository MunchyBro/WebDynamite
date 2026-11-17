use serde_json::json;
use sysinfo::{Disks, Networks, System};
use std::time::Duration;

/// Collects a short CPU sample and a point-in-time snapshot of host resources.
/// Memory and disk values are reported in bytes; network counters are cumulative
/// since the operating system booted or the interface was reset.
pub fn snapshot() -> serde_json::Value {
    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(250));
    system.refresh_cpu_usage();

    let disks = Disks::new_with_refreshed_list();
    let disk_total: u64 = disks.iter().map(|disk| disk.total_space()).sum();
    let disk_available: u64 = disks.iter().map(|disk| disk.available_space()).sum();
    let disk_used = disk_total.saturating_sub(disk_available);

    let networks = Networks::new_with_refreshed_list();
    let received: u64 = networks.iter().map(|(_, data)| data.total_received()).sum();
    let transmitted: u64 = networks.iter().map(|(_, data)| data.total_transmitted()).sum();
    let interfaces: Vec<serde_json::Value> = networks.iter().map(|(name, data)| {
        json!({
            "name": name,
            "received_bytes": data.total_received(),
            "transmitted_bytes": data.total_transmitted()
        })
    }).collect();

    let memory_total = system.total_memory();
    let memory_used = system.used_memory();
    json!({
        "cpu_usage_percent": system.global_cpu_usage(),
        "memory_total_bytes": memory_total,
        "memory_used_bytes": memory_used,
        "memory_usage_percent": if memory_total > 0 { memory_used as f64 * 100.0 / memory_total as f64 } else { 0.0 },
        "disk_total_bytes": disk_total,
        "disk_used_bytes": disk_used,
        "disk_available_bytes": disk_available,
        "disk_usage_percent": if disk_total > 0 { disk_used as f64 * 100.0 / disk_total as f64 } else { 0.0 },
        "network_received_bytes": received,
        "network_transmitted_bytes": transmitted,
        "network_interfaces": interfaces,
        "host_name": System::host_name(),
        "os_name": System::name(),
        "os_version": System::os_version(),
        "uptime_seconds": System::uptime()
    })
}
