pub use taboom_core::hardware::{
    count_cpu_list, device_memory_gb, host_architecture, physical_memory_mb as mem_total_mb,
};

/// CPUs this process may run on, which is what Chrome reports as `hardwareConcurrency`
/// (Compose `cpuset` narrows it; `cpus:` quotas do not).
pub fn visible_cpus() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Cpus_allowed_list:").map(|v| count_cpu_list(v.trim()))))
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get() as u32))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists_count_and_probe() {
        assert_eq!(count_cpu_list("0-3,6,8-9"), 7);
        assert_eq!(count_cpu_list("0"), 1);
        assert!(visible_cpus() >= 1);
    }

}
