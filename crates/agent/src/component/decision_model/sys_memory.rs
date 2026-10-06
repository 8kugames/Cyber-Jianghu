// ============================================================================
// 系统可用内存探测（低配自动降档判据）
// ============================================================================
//
// 仅用于决策模型量化档位的自动降级：Linux 精确读取 MemAvailable（并叠加
// cgroup v2 限额），macOS 以物理内存为上界估计（不会误触发降档），其余平台
// 返回 None（不降档）。模型权重 2B q5_k_m 约 2.5GB，任何常规桌面机均可承载，
// 降档主要面向低配 Linux 容器/小内存主机。

/// 系统可用内存（MB）；无法探测时返回 None
pub fn available_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        return linux_available_mb();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_total_mb();
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "linux")]
fn linux_available_mb() -> Option<u64> {
    // 非常规内核：无法解析时按 0 处理会误触发降档，保持 None 维持原档
    let proc_avail = parse_proc_meminfo();
    let cgroup_avail = cgroup_available_mb();
    match (proc_avail, cgroup_avail) {
        (Some(a), Some(c)) => Some(a.min(c)),
        (Some(a), None) => Some(a),
        (None, Some(c)) => Some(c),
        (None, None) => None,
    }
}

/// /proc/meminfo 的 MemAvailable（kB → MB）
#[cfg(target_os = "linux")]
fn parse_proc_meminfo() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest.trim().split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

/// cgroup v2 内存限额可用量（limit - current，MB）；无限额或非 cgroup 返回 None
#[cfg(target_os = "linux")]
fn cgroup_available_mb() -> Option<u64> {
    const LIMITS: [&str; 2] = [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ];
    const CURRENTS: [&str; 2] = [
        "/sys/fs/cgroup/memory.current",
        "/sys/fs/cgroup/memory/memory.usage_in_bytes",
    ];
    let limit = read_cgroup_bytes(&LIMITS)?;
    if limit >= u64::MAX / 2 {
        return None; // "max" 无限额
    }
    let current = read_cgroup_bytes(&CURRENTS).unwrap_or(0);
    Some(limit.saturating_sub(current) / (1024 * 1024))
}

#[cfg(target_os = "linux")]
fn read_cgroup_bytes(paths: &[&str]) -> Option<u64> {
    for p in paths {
        if let Ok(content) = std::fs::read_to_string(p) {
            let v = content.trim();
            if let Ok(bytes) = v.parse::<u64>() {
                return Some(bytes);
            }
        }
    }
    None
}

/// macOS：物理内存总量（MB）——作为"可用量"的上界估计，低配 Mac 同样不会
/// 误触发降档（q5_k_m 2B 需约 2.5GB，任何现售 Mac 皆满足）。
#[cfg(target_os = "macos")]
fn macos_total_mb() -> Option<u64> {
    let mut size: libc::size_t = std::mem::size_of::<u64>();
    let mut memsize: u64 = 0;
    let name = b"hw.memsize\0";
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const std::ffi::c_char,
            (&mut memsize as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    Some(memsize / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_returns_sane_value_on_this_host() {
        if let Some(mb) = available_memory_mb() {
            assert!(mb > 0, "可用内存应大于 0：{mb}MB");
            assert!(mb < 1 << 20, "可用内存应小于 1TB：{mb}MB");
        }
        // None 也是合法结果（非 Linux/macOS 平台）
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_meminfo_parses_when_present() {
        if std::path::Path::new("/proc/meminfo").exists() {
            assert!(
                parse_proc_meminfo().is_some(),
                "Linux 上应能解析 MemAvailable"
            );
        }
    }
}
