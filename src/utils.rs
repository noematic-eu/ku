use unicode_width::UnicodeWidthStr;

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_bytes_signed(delta: i64) -> String {
    if delta == 0 {
        return "0 B".to_string();
    }
    let sign = if delta > 0 { "+" } else { "-" };
    format!("{sign}{}", format_bytes(delta.unsigned_abs()))
}

pub fn format_percent(pct: f64) -> String {
    if pct.is_nan() {
        return "—".to_string();
    }
    format!("{pct:5.1}%")
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

pub fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

pub fn parse_size(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (num, unit) = trimmed.split_at(split);
    let value: f64 = num.trim().parse().ok()?;
    if value < 0.0 {
        return None;
    }
    let mul = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "%" => return None,
        _ => return None,
    };
    Some((value * mul) as u64)
}

pub fn parse_duration_window(input: &str) -> Option<i64> {
    let trimmed = input.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (num, unit) = trimmed.split_at(split);
    let value: i64 = num.trim().parse().ok()?;
    let secs = match unit {
        "s" => value,
        "m" => value * 60,
        "h" => value * 3_600,
        "d" => value * 86_400,
        _ => return None,
    };
    Some(secs)
}

pub fn truncate_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for ch in s.chars() {
        let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_w + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_w;
    }
    out.push('…');
    out
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessFilter {
    pub text: String,
    pub cpu_min: Option<f32>,
    pub mem_min: Option<u64>,
    pub user: Option<String>,
}

impl ProcessFilter {
    pub fn parse(input: &str) -> Self {
        let mut filter = ProcessFilter::default();
        let mut text_parts = Vec::new();
        for token in input.split_whitespace() {
            let lower = token.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("cpu>")
                && let Ok(v) = rest.parse::<f32>()
            {
                filter.cpu_min = Some(v);
                continue;
            }
            if let Some(rest) = lower.strip_prefix("cpu>=")
                && let Ok(v) = rest.parse::<f32>()
            {
                filter.cpu_min = Some(v);
                continue;
            }
            if let Some(rest) = lower.strip_prefix("mem>")
                && let Some(v) = parse_mem_token(rest)
            {
                filter.mem_min = Some(v);
                continue;
            }
            if let Some(rest) = lower.strip_prefix("mem>=")
                && let Some(v) = parse_mem_token(rest)
            {
                filter.mem_min = Some(v);
                continue;
            }
            if let Some(rest) = token.strip_prefix("user:") {
                filter.user = Some(rest.to_string());
                continue;
            }
            if let Some(rest) = token.strip_prefix("user=") {
                filter.user = Some(rest.to_string());
                continue;
            }
            text_parts.push(token);
        }
        filter.text = text_parts.join(" ").to_ascii_lowercase();
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.cpu_min.is_none()
            && self.mem_min.is_none()
            && self.user.is_none()
    }

    pub fn matches(&self, pid: u32, name: &str, user: &str, cmd: &str, cpu: f32, mem: u64) -> bool {
        if let Some(min) = self.cpu_min
            && cpu < min
        {
            return false;
        }
        if let Some(min) = self.mem_min
            && mem < min
        {
            return false;
        }
        if let Some(ref want) = self.user
            && !user.eq_ignore_ascii_case(want)
        {
            return false;
        }
        if self.text.is_empty() {
            return true;
        }
        let hay = format!("{pid} {name} {user} {cmd}").to_ascii_lowercase();
        hay.contains(&self.text)
    }
}

fn parse_mem_token(token: &str) -> Option<u64> {
    if let Some(stripped) = token.strip_suffix('%') {
        let _ = stripped;
        return parse_size(token.trim_end_matches('%')).or_else(|| {
            token
                .trim_end_matches('%')
                .parse::<f64>()
                .ok()
                .map(|p| (p / 100.0 * 16.0 * 1024.0 * 1024.0 * 1024.0) as u64)
        });
    }
    parse_size(token)
}

#[cfg(unix)]
pub fn inode_usage(path: &std::path::Path) -> Option<(u64, u64)> {
    let cstr = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cstr.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let total = stat.f_files as u64;
    if total == 0 {
        return None;
    }
    let free = stat.f_ffree as u64;
    Some((total.saturating_sub(free), total))
}

#[cfg(not(unix))]
pub fn inode_usage(_path: &std::path::Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format_scales() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MiB");
        assert_eq!(format_bytes_signed(-2048), "-2.00 KiB");
        assert_eq!(format_bytes_signed(0), "0 B");
    }

    #[test]
    fn uptime_format() {
        assert_eq!(format_uptime(65), "1m");
        assert_eq!(format_uptime(3700), "1h 1m");
        assert_eq!(format_uptime(90_000), "1d 1h 0m");
    }

    #[test]
    fn parse_sizes() {
        assert_eq!(parse_size("10M"), Some(10 * 1024 * 1024));
        assert_eq!(
            parse_size("1.5G"),
            Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(parse_size("512"), Some(512));
        assert!(parse_size("nope").is_none());
    }

    #[test]
    fn parse_windows() {
        assert_eq!(parse_duration_window("1m"), Some(60));
        assert_eq!(parse_duration_window("5m"), Some(300));
        assert_eq!(parse_duration_window("1h"), Some(3600));
        assert_eq!(parse_duration_window("24h"), Some(86400));
        assert_eq!(parse_duration_window("7d"), Some(7 * 86400));
    }

    #[test]
    fn process_filter_tokens() {
        let f = ProcessFilter::parse("nginx cpu>2.5 mem>100M user:root");
        assert_eq!(f.text, "nginx");
        assert_eq!(f.cpu_min, Some(2.5));
        assert_eq!(f.mem_min, Some(100 * 1024 * 1024));
        assert_eq!(f.user.as_deref(), Some("root"));
        assert!(f.matches(
            42,
            "nginx",
            "root",
            "/usr/sbin/nginx",
            10.0,
            200 * 1024 * 1024
        ));
        assert!(!f.matches(
            42,
            "nginx",
            "www",
            "/usr/sbin/nginx",
            10.0,
            200 * 1024 * 1024
        ));
        assert!(!f.matches(42, "sshd", "root", "sshd", 10.0, 200 * 1024 * 1024));
    }

    #[test]
    fn truncate_keeps_width() {
        assert_eq!(truncate_ellipsis("hello", 10), "hello");
        let t = truncate_ellipsis("abcdefghij", 5);
        assert!(t.ends_with('…'));
        assert!(t.width() <= 5);
    }
}
