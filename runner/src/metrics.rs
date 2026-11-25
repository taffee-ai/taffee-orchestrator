//! Process memory and CPU readings (Linux /proc). Elsewhere they read as zero.

use serde::Serialize;

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct MemorySample {
    pub rss_kb: u64,
    pub pss_kb: u64,
    pub private_dirty_kb: u64,
}

impl MemorySample {
    pub fn current() -> Self {
        let Ok(content) = std::fs::read_to_string("/proc/self/smaps_rollup") else {
            return Self::default();
        };
        let field = |name: &str| {
            content
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        Self { rss_kb: field("Rss:"), pss_kb: field("Pss:"), private_dirty_kb: field("Private_Dirty:") }
    }

    pub fn pss_mb(&self) -> f64 {
        self.pss_kb as f64 / 1024.0
    }
    pub fn rss_mb(&self) -> f64 {
        self.rss_kb as f64 / 1024.0
    }
    pub fn private_dirty_mb(&self) -> f64 {
        self.private_dirty_kb as f64 / 1024.0
    }
}

/// Process CPU time (user + system) in milliseconds, from /proc/self/stat.
pub fn process_cpu_ms() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // Fields after the parenthesised command name; utime and stime are 14 and 15 overall.
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
        return 0.0;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| fields.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    // utime = field 14 -> index 11 after the command; stime = index 12. USER_HZ is 100 on Linux.
    (ticks(11) + ticks(12)) as f64 * 10.0
}

/// MemAvailable in MB, from /proc/meminfo.
pub fn mem_available_mb() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = info.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb / 1024)
}
