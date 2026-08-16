//! Network pure logic — IPv4 parsing, CIDR membership, SSID and signal
//! validation.
//!
//! Host-testable by design: pure string/integer logic, no syscalls, so the
//! `#[cfg(test)]` module runs under host `cargo test` (the same
//! cfg(not(test)) treatment as libsarga's errno/net/semver).

/// Parse a dotted-quad IPv4 address ("192.168.1.10").
/// Rejects empty octets, non-digit octets, values > 255, and > 4 octets.
pub fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = s.split('.');
    for slot in octets.iter_mut() {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let v: u32 = part.parse().ok()?;
        if v > 255 {
            return None;
        }
        *slot = v as u8;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(octets)
}

/// Whether `ip` is inside the CIDR block `base/prefix` (prefix 0..=32).
pub fn cidr_contains(ip: [u8; 4], base: [u8; 4], prefix: u8) -> bool {
    let prefix = prefix.min(32);
    if prefix == 0 {
        return true;
    }
    let ip_u = u32::from_be_bytes(ip);
    let base_u = u32::from_be_bytes(base);
    let mask = if prefix == 32 {
        u32::MAX
    } else {
        !((1u32 << (32 - prefix)) - 1)
    };
    (ip_u & mask) == (base_u & mask)
}

/// SSID validation: 1..=32 printable ASCII bytes (IEEE 802.11 limit).
pub fn ssid_valid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| (0x20..=0x7E).contains(&b))
}

/// Normalize a Wi-Fi RSSI (dBm, typically -100..=-40) to a 0..=100 percent.
/// Values outside the range are clamped.
pub fn rssi_to_percent(rssi: i8) -> u8 {
    let clamped = rssi.clamp(-100, -40);
    ((clamped + 100) as i16 * 100 / 60) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_addresses() {
        assert_eq!(parse_ipv4("0.0.0.0"), Some([0, 0, 0, 0]));
        assert_eq!(parse_ipv4("192.168.1.10"), Some([192, 168, 1, 10]));
        assert_eq!(parse_ipv4("255.255.255.255"), Some([255, 255, 255, 255]));
        assert_eq!(parse_ipv4("10.0.0.1"), Some([10, 0, 0, 1]));
    }

    #[test]
    fn parse_rejects_malformed() {
        assert_eq!(parse_ipv4(""), None);
        assert_eq!(parse_ipv4("1.2.3"), None);
        assert_eq!(parse_ipv4("1.2.3.4.5"), None);
        assert_eq!(parse_ipv4("256.1.1.1"), None);
        assert_eq!(parse_ipv4("1.2.3.4."), None);
        assert_eq!(parse_ipv4(".1.2.3"), None);
        assert_eq!(parse_ipv4("1..3.4"), None);
        assert_eq!(parse_ipv4("a.b.c.d"), None);
        assert_eq!(parse_ipv4("1.2.3.4 "), None);
    }

    #[test]
    fn cidr_membership() {
        let net = parse_ipv4("192.168.1.0").unwrap();
        let host = parse_ipv4("192.168.1.200").unwrap();
        let other = parse_ipv4("192.168.2.1").unwrap();
        assert!(cidr_contains(host, net, 24));
        assert!(!cidr_contains(other, net, 24));
        assert!(cidr_contains(host, net, 0)); // /0 contains everything
        assert!(cidr_contains(other, net, 0));
        assert!(!cidr_contains(parse_ipv4("192.168.1.1").unwrap(), net, 32));
        assert!(cidr_contains(parse_ipv4("192.168.1.0").unwrap(), net, 32));
        // Prefix > 32 is clamped to 32, not a panic.
        assert!(!cidr_contains(parse_ipv4("10.0.0.1").unwrap(), net, 200));
    }

    #[test]
    fn cidr_masks_are_exact() {
        let base = [255, 255, 255, 0];
        assert!(cidr_contains([255, 255, 255, 1], base, 24));
        assert!(cidr_contains([255, 255, 255, 254], base, 24));
        assert!(!cidr_contains([255, 255, 254, 1], base, 24));
    }

    #[test]
    fn ssid_rules() {
        assert!(ssid_valid("MyNetwork"));
        assert!(ssid_valid(" ")); // single printable space
        assert!(ssid_valid("a".repeat(32).as_str()));
        assert!(!ssid_valid(""));
        assert!(!ssid_valid("a".repeat(33).as_str()));
        assert!(!ssid_valid("tab\there"));
        assert!(!ssid_valid("ünïcode"));
    }

    #[test]
    fn rssi_normalization() {
        assert_eq!(rssi_to_percent(-40), 100);
        assert_eq!(rssi_to_percent(-70), 50); // midpoint
        assert_eq!(rssi_to_percent(-100), 0);
        assert_eq!(rssi_to_percent(-90), 16); // 10/60 -> 16.6 -> 16
        assert_eq!(rssi_to_percent(-30), 100); // clamped
        assert_eq!(rssi_to_percent(-110), 0); // clamped
    }
}
