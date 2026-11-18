use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    Wired,
    Wireless,
    Other,
}

impl Medium {
    pub fn label(self) -> &'static str {
        match self {
            Medium::Wired => "wired",
            Medium::Wireless => "wireless",
            Medium::Other => "other",
        }
    }
}

/// Detect the medium of the interface carrying the default route.
///
/// Linux sysfs is enough:
/// - `/sys/class/net/<iface>/wireless` exists for Wi-Fi
/// - `/sys/class/net/<iface>/type` is 1 for Ethernet
pub fn detect_medium() -> Medium {
    let Some(iface) = default_iface() else {
        return Medium::Other;
    };
    medium_of(&format!("/sys/class/net/{iface}"))
}

fn medium_of(base: &str) -> Medium {
    let base = Path::new(base);
    if base.join("wireless").exists() {
        return Medium::Wireless;
    }
    match fs::read_to_string(base.join("type"))
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
    {
        Some(1) => Medium::Wired,
        _ => Medium::Other,
    }
}

fn default_iface() -> Option<String> {
    let raw = fs::read_to_string("/proc/net/route").ok()?;
    parse_default_iface(&raw)
}

fn parse_default_iface(raw: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for line in raw.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 8 {
            continue;
        }
        let iface = cols[0];
        let destination = cols[1];
        let metric = cols[6];
        let mask = cols[7];
        if destination != "00000000" || mask != "00000000" {
            continue;
        }
        let Ok(metric) = metric.parse::<u32>() else {
            continue;
        };
        if best
            .as_ref()
            .map_or(true, |(best_metric, _)| metric < *best_metric)
        {
            best = Some((metric, iface.to_string()));
        }
    }
    best.map(|(_, iface)| iface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_route_by_metric() {
        let raw = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
wlan0\t00000000\t0101A8C0\t0003\t0\t0\t200\t00000000\t0\t0\t0
eth0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0
";
        assert_eq!(parse_default_iface(raw), Some("eth0".to_string()));
    }

    #[test]
    fn ignores_non_default_routes() {
        let raw = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
eth0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0
";
        assert_eq!(parse_default_iface(raw), None);
    }
}
