//! Host resource metrics from `/proc` (CPU, memory, load, net, disk I/O).
//!
//! Counters follow Prometheus / node_exporter conventions so Grafana can use
//! `rate()`. Linux-only collection; parsers are OS-agnostic for tests.

use std::fmt::Write as _;

const CPU_MODES: [&str; 8] = [
    "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuCounters {
    /// `total` or `0`, `1`, …
    pub cpu: String,
    /// Seconds in each of [`CPU_MODES`].
    pub seconds: [f64; 8],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryBytes {
    pub total: u64,
    pub available: u64,
    pub free: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetCounters {
    pub device: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskCounters {
    pub device: String,
    pub reads_completed: u64,
    pub writes_completed: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FsUsage {
    pub label: String,
    pub mountpoint: String,
    pub size_bytes: u64,
    pub avail_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostSnapshot {
    pub hostname: String,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub uptime_seconds: f64,
    pub memory: MemoryBytes,
    pub cpus: Vec<CpuCounters>,
    pub nets: Vec<NetCounters>,
    pub disks: Vec<DiskCounters>,
    pub filesystems: Vec<FsUsage>,
}

impl HostSnapshot {
    pub fn collect() -> Self {
        #[cfg(not(target_os = "linux"))]
        {
            Self::default()
        }
        #[cfg(target_os = "linux")]
        {
            collect_linux()
        }
    }

    pub fn render_prometheus(&self, out: &mut String) {
        if self.cpus.is_empty()
            && self.memory.total == 0
            && self.nets.is_empty()
            && self.disks.is_empty()
        {
            return;
        }

        let host = sanitize_label(&self.hostname);

        if !host.is_empty() {
            let _ = writeln!(
                out,
                "# HELP pertisk_host_info Host identity\n\
                 # TYPE pertisk_host_info gauge\n\
                 pertisk_host_info{{hostname=\"{host}\"}} 1"
            );
        }

        let _ = writeln!(
            out,
            "# HELP pertisk_load1 1-minute load average\n\
             # TYPE pertisk_load1 gauge\n\
             pertisk_load1 {}\n\
             # HELP pertisk_load5 5-minute load average\n\
             # TYPE pertisk_load5 gauge\n\
             pertisk_load5 {}\n\
             # HELP pertisk_load15 15-minute load average\n\
             # TYPE pertisk_load15 gauge\n\
             pertisk_load15 {}\n\
             # HELP pertisk_uptime_seconds Seconds since boot\n\
             # TYPE pertisk_uptime_seconds gauge\n\
             pertisk_uptime_seconds {}",
            self.load1, self.load5, self.load15, self.uptime_seconds
        );

        let _ = writeln!(
            out,
            "# HELP pertisk_memory_total_bytes Total physical memory\n\
             # TYPE pertisk_memory_total_bytes gauge\n\
             pertisk_memory_total_bytes {}\n\
             # HELP pertisk_memory_available_bytes Estimate of memory available for new workloads\n\
             # TYPE pertisk_memory_available_bytes gauge\n\
             pertisk_memory_available_bytes {}\n\
             # HELP pertisk_memory_free_bytes Unused memory (MemFree)\n\
             # TYPE pertisk_memory_free_bytes gauge\n\
             pertisk_memory_free_bytes {}",
            self.memory.total, self.memory.available, self.memory.free
        );

        if !self.cpus.is_empty() {
            out.push_str(
                "# HELP pertisk_cpu_seconds_total Seconds CPU spent in each mode\n\
                 # TYPE pertisk_cpu_seconds_total counter\n",
            );
            for cpu in &self.cpus {
                let id = sanitize_label(&cpu.cpu);
                for (i, mode) in CPU_MODES.iter().enumerate() {
                    let _ = writeln!(
                        out,
                        "pertisk_cpu_seconds_total{{cpu=\"{id}\",mode=\"{mode}\"}} {}",
                        cpu.seconds[i]
                    );
                }
            }
        }

        if !self.nets.is_empty() {
            out.push_str(
                "# HELP pertisk_network_receive_bytes_total Network bytes received\n\
                 # TYPE pertisk_network_receive_bytes_total counter\n",
            );
            for n in &self.nets {
                let d = sanitize_label(&n.device);
                let _ = writeln!(
                    out,
                    "pertisk_network_receive_bytes_total{{device=\"{d}\"}} {}",
                    n.rx_bytes
                );
            }
            out.push_str(
                "# HELP pertisk_network_transmit_bytes_total Network bytes transmitted\n\
                 # TYPE pertisk_network_transmit_bytes_total counter\n",
            );
            for n in &self.nets {
                let d = sanitize_label(&n.device);
                let _ = writeln!(
                    out,
                    "pertisk_network_transmit_bytes_total{{device=\"{d}\"}} {}",
                    n.tx_bytes
                );
            }
            out.push_str(
                "# HELP pertisk_network_receive_packets_total Network packets received\n\
                 # TYPE pertisk_network_receive_packets_total counter\n",
            );
            for n in &self.nets {
                let d = sanitize_label(&n.device);
                let _ = writeln!(
                    out,
                    "pertisk_network_receive_packets_total{{device=\"{d}\"}} {}",
                    n.rx_packets
                );
            }
            out.push_str(
                "# HELP pertisk_network_transmit_packets_total Network packets transmitted\n\
                 # TYPE pertisk_network_transmit_packets_total counter\n",
            );
            for n in &self.nets {
                let d = sanitize_label(&n.device);
                let _ = writeln!(
                    out,
                    "pertisk_network_transmit_packets_total{{device=\"{d}\"}} {}",
                    n.tx_packets
                );
            }
        }

        if !self.disks.is_empty() {
            out.push_str(
                "# HELP pertisk_disk_read_bytes_total Bytes read from disk\n\
                 # TYPE pertisk_disk_read_bytes_total counter\n",
            );
            for d in &self.disks {
                let name = sanitize_label(&d.device);
                let _ = writeln!(
                    out,
                    "pertisk_disk_read_bytes_total{{device=\"{name}\"}} {}",
                    d.read_bytes
                );
            }
            out.push_str(
                "# HELP pertisk_disk_written_bytes_total Bytes written to disk\n\
                 # TYPE pertisk_disk_written_bytes_total counter\n",
            );
            for d in &self.disks {
                let name = sanitize_label(&d.device);
                let _ = writeln!(
                    out,
                    "pertisk_disk_written_bytes_total{{device=\"{name}\"}} {}",
                    d.written_bytes
                );
            }
            out.push_str(
                "# HELP pertisk_disk_reads_completed_total Completed disk reads\n\
                 # TYPE pertisk_disk_reads_completed_total counter\n",
            );
            for d in &self.disks {
                let name = sanitize_label(&d.device);
                let _ = writeln!(
                    out,
                    "pertisk_disk_reads_completed_total{{device=\"{name}\"}} {}",
                    d.reads_completed
                );
            }
            out.push_str(
                "# HELP pertisk_disk_writes_completed_total Completed disk writes\n\
                 # TYPE pertisk_disk_writes_completed_total counter\n",
            );
            for d in &self.disks {
                let name = sanitize_label(&d.device);
                let _ = writeln!(
                    out,
                    "pertisk_disk_writes_completed_total{{device=\"{name}\"}} {}",
                    d.writes_completed
                );
            }
        }

        if !self.filesystems.is_empty() {
            out.push_str(
                "# HELP pertisk_filesystem_size_bytes Filesystem capacity\n\
                 # TYPE pertisk_filesystem_size_bytes gauge\n",
            );
            for fs in &self.filesystems {
                let label = sanitize_label(&fs.label);
                let mp = sanitize_label(&fs.mountpoint);
                let _ = writeln!(
                    out,
                    "pertisk_filesystem_size_bytes{{label=\"{label}\",mountpoint=\"{mp}\"}} {}",
                    fs.size_bytes
                );
            }
            out.push_str(
                "# HELP pertisk_filesystem_avail_bytes Filesystem space available to non-root\n\
                 # TYPE pertisk_filesystem_avail_bytes gauge\n",
            );
            for fs in &self.filesystems {
                let label = sanitize_label(&fs.label);
                let mp = sanitize_label(&fs.mountpoint);
                let _ = writeln!(
                    out,
                    "pertisk_filesystem_avail_bytes{{label=\"{label}\",mountpoint=\"{mp}\"}} {}",
                    fs.avail_bytes
                );
            }
        }
    }
}

fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' | '\\' | '\n' => '_',
            c => c,
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn collect_linux() -> HostSnapshot {
    let ticks = clock_ticks();
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let (load1, load5, load15) = parse_loadavg(&read("/proc/loadavg"));
    let uptime_seconds = parse_uptime(&read("/proc/uptime"));
    let memory = parse_meminfo(&read("/proc/meminfo"));
    let cpus = parse_stat(&read("/proc/stat"), ticks);
    let nets = parse_netdev(&read("/proc/net/dev"));
    let disks = parse_diskstats(&read("/proc/diskstats"));
    let filesystems = filesystems_from_inspect();
    HostSnapshot {
        hostname,
        load1,
        load5,
        load15,
        uptime_seconds,
        memory,
        cpus,
        nets,
        disks,
        filesystems,
    }
}

#[cfg(target_os = "linux")]
fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn clock_ticks() -> f64 {
    // SAFETY: sysconf(_SC_CLK_TCK) has no pointer args.
    let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if t > 0 {
        t as f64
    } else {
        100.0
    }
}

#[cfg(target_os = "linux")]
fn filesystems_from_inspect() -> Vec<FsUsage> {
    crate::disk_inspect::inspect_disks()
        .volumes
        .into_iter()
        .filter(|v| v.mounted && v.total_bytes > 0)
        .map(|v| FsUsage {
            label: v.label,
            mountpoint: v.mountpoint,
            size_bytes: v.total_bytes,
            avail_bytes: v.total_bytes.saturating_sub(v.used_bytes),
        })
        .collect()
}

#[allow(dead_code)]
pub fn parse_meminfo(text: &str) -> MemoryBytes {
    let mut total = 0u64;
    let mut available = 0u64;
    let mut free = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let kib: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let bytes = kib.saturating_mul(1024);
        match key {
            "MemTotal:" => total = bytes,
            "MemAvailable:" => available = bytes,
            "MemFree:" => free = bytes,
            "Buffers:" => buffers = bytes,
            "Cached:" => cached = bytes,
            _ => {}
        }
    }
    if available == 0 {
        available = free.saturating_add(buffers).saturating_add(cached);
    }
    MemoryBytes {
        total,
        available,
        free,
    }
}

/// `/proc/stat` CPU lines → seconds per mode (`ticks` is USER_HZ, usually 100).
#[allow(dead_code)]
pub fn parse_stat(text: &str, ticks: f64) -> Vec<CpuCounters> {
    let hz = if ticks > 0.0 { ticks } else { 100.0 };
    let mut out = Vec::new();
    for line in text.lines() {
        let rest = if let Some(r) = line.strip_prefix("cpu ") {
            ("total", r)
        } else if let Some(r) = line.strip_prefix("cpu") {
            let mut it = r.splitn(2, |c: char| c.is_whitespace());
            let id = it.next().unwrap_or("");
            let vals = it.next().unwrap_or("");
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            (id, vals)
        } else {
            continue;
        };
        let mut vals = rest
            .1
            .split_whitespace()
            .filter_map(|v| v.parse::<u64>().ok());
        let mut seconds = [0.0; 8];
        for slot in seconds.iter_mut() {
            *slot = vals.next().unwrap_or(0) as f64 / hz;
        }
        out.push(CpuCounters {
            cpu: rest.0.to_string(),
            seconds,
        });
    }
    out
}

#[allow(dead_code)]
pub fn parse_netdev(text: &str) -> Vec<NetCounters> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let device = name.trim();
        if device.is_empty() || device == "lo" {
            continue;
        }
        let mut nums = rest
            .split_whitespace()
            .filter_map(|v| v.parse::<u64>().ok());
        let rx_bytes = nums.next().unwrap_or(0);
        let rx_packets = nums.next().unwrap_or(0);
        for _ in 0..6 {
            let _ = nums.next();
        }
        let tx_bytes = nums.next().unwrap_or(0);
        let tx_packets = nums.next().unwrap_or(0);
        out.push(NetCounters {
            device: device.to_string(),
            rx_bytes,
            rx_packets,
            tx_bytes,
            tx_packets,
        });
    }
    out.sort_by(|a, b| a.device.cmp(&b.device));
    out
}

#[allow(dead_code)]
pub fn parse_diskstats(text: &str) -> Vec<DiskCounters> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _major = parts.next();
        let _minor = parts.next();
        let Some(name) = parts.next() else {
            continue;
        };
        if skip_disk(name) {
            continue;
        }
        let reads_completed = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let _merged_r = parts.next();
        let sectors_read: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let _ms_r = parts.next();
        let writes_completed = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let _merged_w = parts.next();
        let sectors_written: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        out.push(DiskCounters {
            device: name.to_string(),
            reads_completed,
            writes_completed,
            read_bytes: sectors_read.saturating_mul(512),
            written_bytes: sectors_written.saturating_mul(512),
        });
    }
    out.sort_by(|a, b| a.device.cmp(&b.device));
    out
}

#[allow(dead_code)]
fn skip_disk(name: &str) -> bool {
    name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("fd")
        || name.starts_with("sr")
        || name.starts_with("dm-")
        || name.starts_with("zram")
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "\
cpu  100 20 30 400 10 2 3 1 0 0
cpu0 50 10 15 200 5 1 1 0 0 0
cpu1 50 10 15 200 5 1 2 1 0 0
intr 1
";

    const MEM: &str = "\
MemTotal:        2048000 kB
MemFree:          512000 kB
MemAvailable:    1024000 kB
Buffers:          128000 kB
Cached:           256000 kB
";

    const NET: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000    10    0    0    0     0          0         0 1000    10    0    0    0     0       0          0
  eth0: 8000    40    0    0    0     0          0         0 4000    20    0    0    0     0       0          0
";

    const DISK: &str = "\
   8       0 sda 100 0 200 0 50 0 80 0 0 0 0
   7       0 loop0 1 0 2 0 0 0 0 0 0 0 0
 253       0 vda 10 0 40 0 5 0 16 0 0 0 0
";

    #[test]
    fn uptime_first_field() {
        assert!((parse_uptime("123.45 456.0") - 123.45).abs() < 1e-9);
        assert_eq!(parse_uptime(""), 0.0);
    }

    #[test]
    fn loadavg_three_fields() {
        assert_eq!(parse_loadavg("0.42 0.35 0.28 1/234 99"), (0.42, 0.35, 0.28));
        assert_eq!(parse_loadavg(""), (0.0, 0.0, 0.0));
    }

    #[test]
    fn meminfo_kib_to_bytes() {
        let m = parse_meminfo(MEM);
        assert_eq!(m.total, 2048000 * 1024);
        assert_eq!(m.available, 1024000 * 1024);
        assert_eq!(m.free, 512000 * 1024);
    }

    #[test]
    fn stat_seconds_from_ticks() {
        let cpus = parse_stat(STAT, 100.0);
        assert_eq!(cpus.len(), 3);
        assert_eq!(cpus[0].cpu, "total");
        assert!((cpus[0].seconds[0] - 1.0).abs() < f64::EPSILON);
        assert!((cpus[0].seconds[3] - 4.0).abs() < f64::EPSILON);
        assert_eq!(cpus[1].cpu, "0");
        assert_eq!(cpus[2].cpu, "1");
    }

    #[test]
    fn netdev_skips_loopback() {
        let nets = parse_netdev(NET);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].device, "eth0");
        assert_eq!(nets[0].rx_bytes, 8000);
        assert_eq!(nets[0].tx_bytes, 4000);
        assert_eq!(nets[0].rx_packets, 40);
        assert_eq!(nets[0].tx_packets, 20);
    }

    #[test]
    fn diskstats_skips_loop_and_uses_512b_sectors() {
        let disks = parse_diskstats(DISK);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].device, "sda");
        assert_eq!(disks[0].read_bytes, 200 * 512);
        assert_eq!(disks[0].written_bytes, 80 * 512);
        assert_eq!(disks[1].device, "vda");
        assert_eq!(disks[1].reads_completed, 10);
    }

    #[test]
    fn prometheus_text_includes_host_series() {
        let snap = HostSnapshot {
            hostname: "node-1".into(),
            load1: 0.5,
            load5: 0.4,
            load15: 0.3,
            uptime_seconds: 12.0,
            memory: MemoryBytes {
                total: 1024,
                available: 512,
                free: 256,
            },
            cpus: parse_stat(STAT, 100.0),
            nets: parse_netdev(NET),
            disks: parse_diskstats(DISK),
            filesystems: vec![FsUsage {
                label: "STATE".into(),
                mountpoint: "/system/state".into(),
                size_bytes: 1000,
                avail_bytes: 400,
            }],
        };
        let mut body = String::new();
        snap.render_prometheus(&mut body);
        assert!(body.contains("pertisk_cpu_seconds_total{cpu=\"total\",mode=\"idle\"}"));
        assert!(body.contains("pertisk_memory_total_bytes 1024"));
        assert!(body.contains("pertisk_network_receive_bytes_total{device=\"eth0\"} 8000"));
        assert!(body.contains("pertisk_disk_read_bytes_total{device=\"sda\"}"));
        assert!(body.contains(
            "pertisk_filesystem_size_bytes{label=\"STATE\",mountpoint=\"/system/state\"} 1000"
        ));
        assert!(body.contains("pertisk_load1 0.5"));
        assert!(body.contains("pertisk_host_info{hostname=\"node-1\"} 1"));
    }
}
