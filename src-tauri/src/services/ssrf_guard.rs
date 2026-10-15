//! SSRF guard for user-supplied outbound URLs (mirrors origin #1300/#1294).
//!
//! Desktop exposure is narrow — the only place we fetch a user-configured URL
//! is the OpenAPI transport's spec fetch (`mcp/openapi_transport.rs`). Server
//! config is admin-managed here (skipAuth default = single admin), so unlike
//! origin's multi-user deployments the guard is defense-in-depth, not a trust
//! boundary. Kept as a hard failure (fail closed) on any unparseable config.
//!
//! Semantics mirrored from origin `src/utils/ssrf.ts`:
//! - blocked IPv4 ranges (unspecified/RFC1918/CGNAT/loopback/link-local/…)
//! - blocked IPv6 forms (::, ::1, fe80::/10, fec0::/10, ff00::/8, 6to4,
//!   Teredo, NAT64 well-known + local-use, ULA fc00::/7, v4-mapped)
//! - `system_config.network.allowedCidrs` carve-outs, validated per request
//!   (removing a range affects the next fetch immediately — no caching)
//! - DNS results validated BEFORE connect, and the validated IPs are pinned
//!   into the reqwest client (`resolve()`) so a rebinding DNS answer between
//!   validation and connect cannot bypass the check.

use std::net::IpAddr;

/// Blocked IPv4 ranges as [start, end] inclusive u32. Ported verbatim from
/// origin (comment preserved: CGNAT range corrected in #1294 so public
/// 100.x/8 addresses outside 100.64/10 are NOT blocked).
const IPV4_BLOCKED_RANGES: [(u32, u32); 9] = [
    (0x0000_0000, 0x00ff_ffff), // 0.0.0.0/8 unspecified
    (0x0a00_0000, 0x0aff_ffff), // 10.0.0.0/8 RFC1918
    (0x6440_0000, 0x647f_ffff), // 100.64.0.0/10 CGNAT
    (0x7f00_0000, 0x7fff_ffff), // 127.0.0.0/8 loopback
    (0xa9fe_0000, 0xa9fe_ffff), // 169.254.0.0/16 link-local (IMDS)
    (0xac10_0000, 0xac1f_ffff), // 172.16.0.0/12 RFC1918
    (0xc000_0000, 0xc000_00ff), // 192.0.0.0/24 IETF protocol assignments
    (0xc0a8_0000, 0xc0a8_ffff), // 192.168.0.0/16 RFC1918
    (0xc612_0000, 0xc613_ffff), // 198.18.0.0/15 benchmarking
];

fn ipv4_to_u32(ip: std::net::Ipv4Addr) -> u32 {
    u32::from(ip)
}

fn is_blocked_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let n = ipv4_to_u32(ip);
    IPV4_BLOCKED_RANGES.iter().any(|(s, e)| n >= *s && n <= *e)
}

fn is_blocked_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let big = u128::from(ip);
    if big == 0 || big == 1 {
        return true; // :: and ::1
    }
    if big >> 118 == 0x3fa {
        return true; // fe80::/10 link-local
    }
    if big >> 118 == 0x3fb {
        return true; // fec0::/10 site-local (deprecated)
    }
    if big >> 120 == 0xff {
        return true; // ff00::/8 multicast — never a valid dial target
    }
    if big >> 112 == 0x2002 {
        return true; // 2002::/16 6to4 transition (deprecated, RFC 7526)
    }
    if big >> 80 == 0x64ff9b1 {
        return true; // 64:ff9b:1::/48 local-use NAT64
    }
    if big >> 96 == 0x2001_0000 {
        return true; // 2001:0::/32 Teredo transition
    }
    if big >> 96 == 0x64ff9b {
        return true; // 64:ff9b::/96 NAT64 well-known prefix
    }
    if big >> 121 == 0x7e {
        return true; // fc00::/7 unique-local
    }
    if big >> 32 == 0xffff {
        return is_blocked_ipv4(u32::try_from(big & 0xffff_ffff).unwrap_or(0).into()); // ::ffff:a.b.c.d
    }
    if big < 0x1_0000_0000 {
        return is_blocked_ipv4(u32::try_from(big).unwrap_or(0).into()); // ::a.b.c.d
    }
    false
}

/// Validate a CIDR allow-list entry: `a.b.c.d/p` with p <= 32 (v4) or
/// `x::y/p` with p <= 128 (v6). Mirrors origin `validateAllowedCidrs`.
pub fn is_valid_cidr(cidr: &str) -> bool {
    let Some((addr, prefix)) = cidr.split_once('/') else {
        return false;
    };
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    // No leading zeros (origin: /^(0|[1-9]\d*)$/).
    if prefix.len() > 1 && prefix.starts_with('0') {
        return false;
    }
    let Ok(p): Result<u32, _> = prefix.parse() else {
        return false;
    };
    match addr.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => p <= 32,
        Ok(IpAddr::V6(_)) => p <= 128,
        Err(_) => false,
    }
}

pub fn validate_allowed_cidrs(value: &[String]) -> bool {
    value.iter().all(|c| is_valid_cidr(c))
}

fn cidr_matches(ip: IpAddr, cidr: &str) -> bool {
    let Some((addr, prefix)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(p) = prefix.parse::<u32>() else {
        return false;
    };
    match (ip, addr.parse::<IpAddr>()) {
        (IpAddr::V4(i), Ok(IpAddr::V4(a))) => {
            let pi = u32::from(i);
            let pa = u32::from(a);
            if p == 0 {
                return true;
            }
            if p > 32 {
                return false;
            }
            let mask = if p == 32 { u32::MAX } else { u32::MAX << (32 - p) };
            pi & mask == pa & mask
        }
        (IpAddr::V6(i), Ok(IpAddr::V6(a))) => {
            let pi = u128::from(i);
            let pa = u128::from(a);
            if p == 0 {
                return true;
            }
            if p > 128 {
                return false;
            }
            let mask = if p == 128 { u128::MAX } else { u128::MAX << (128 - p) };
            pi & mask == pa & mask
        }
        _ => false,
    }
}

/// Is `ip` disallowed, given the per-request CIDR allow-list? Reads config at
/// each call so config edits apply to live clients.
///
/// Desktop semantics (user decision 2026-10-13): the allow-list is the ONLY
/// gate — no identity/admin distinction at call time. An EMPTY list means
/// "allow all" (default single-user posture: local tool, admin is the only
/// editor of this config), so the default deployment never blocks. A
/// NON-EMPTY list opts into enforcement: internal ranges are blocked unless
/// carved out; a malformed list still fails closed.
pub fn is_disallowed_ip(ip: IpAddr, allowed_cidrs: &[String]) -> bool {
    if allowed_cidrs.is_empty() {
        return false;
    }
    let blocked = match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    };
    if !blocked {
        return false;
    }
    if !validate_allowed_cidrs(allowed_cidrs) {
        return true; // malformed allow-list → fail closed
    }
    !allowed_cidrs.iter().any(|c| cidr_matches(ip, c))
}

/// Resolve + validate a URL host, returning the validated addresses for
/// connection pinning. `Err` message mirrors origin's UnsafeUrlError classes.
pub async fn assert_safe_url(
    raw_url: &str,
    allowed_cidrs: &[String],
) -> Result<Vec<std::net::SocketAddr>, String> {
    let parsed: url::Url = url::Url::parse(raw_url).map_err(|e| format!("Invalid URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("Disallowed URL scheme: {scheme}"));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| "URL has no port".to_string())?;
    let host = parsed.host_str().ok_or_else(|| "URL has no host".to_string())?;
    let host = host.trim_start_matches('[').trim_end_matches(']');

    // IP literal: no DNS involved.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_disallowed_ip(ip, allowed_cidrs) {
            return Err(format!("Blocked target IP: {host}"));
        }
        return Ok(vec![std::net::SocketAddr::new(ip, port)]);
    }

    // Hostname: resolve, validate EVERY answer, pin for connect.
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| format!("Unable to resolve host: {host}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(format!("No addresses resolved for host: {host}"));
    }
    for a in &addrs {
        if is_disallowed_ip(a.ip(), allowed_cidrs) {
            return Err(format!("Blocked resolved IP: {}", a.ip()));
        }
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_ranges_match_origin() {
        // #1294: public 100.x outside CGNAT must NOT be blocked.
        assert!(is_blocked_ipv4("100.64.0.1".parse().unwrap()));
        assert!(is_blocked_ipv4("100.100.1.1".parse().unwrap()));
        assert!(!is_blocked_ipv4("100.200.1.1".parse().unwrap()));
        assert!(is_blocked_ipv4("127.0.0.1".parse().unwrap()));
        assert!(is_blocked_ipv4("10.1.2.3".parse().unwrap()));
        assert!(is_blocked_ipv4("169.254.169.254".parse().unwrap()));
        assert!(is_blocked_ipv4("192.168.1.1".parse().unwrap()));
        assert!(!is_blocked_ipv4("8.8.8.8".parse().unwrap()));
        assert!(!is_blocked_ipv4("121.230.19.1".parse().unwrap()));
    }

    #[test]
    fn blocked_ipv6_forms() {
        assert!(is_blocked_ipv6("::1".parse().unwrap()));
        assert!(is_blocked_ipv6("fe80::1".parse().unwrap()));
        assert!(is_blocked_ipv6("::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_blocked_ipv6("fc00::1".parse().unwrap()));
        assert!(!is_blocked_ipv6("2606:4700::1".parse().unwrap()));
    }

    #[test]
    fn cidr_validation_and_matching() {
        assert!(is_valid_cidr("10.0.0.0/8"));
        assert!(is_valid_cidr("0.0.0.0/0"));
        assert!(!is_valid_cidr("10.0.0.0/33"));
        assert!(!is_valid_cidr("10.0.0.0"));
        assert!(!is_valid_cidr("10.0.0.0/08"));
        assert!(!is_valid_cidr("host/8"));
        assert!(cidr_matches("10.1.2.3".parse().unwrap(), "10.0.0.0/8"));
        assert!(!cidr_matches("11.1.2.3".parse().unwrap(), "10.0.0.0/8"));
    }

    #[tokio::test]
    async fn assert_safe_url_blocks_internal_and_allows_exceptions() {
        // Desktop semantics: EMPTY allow-list = allow all (default posture).
        assert!(assert_safe_url("http://127.0.0.1:1/x", &[]).await.is_ok());
        assert!(assert_safe_url("http://169.254.169.254/latest", &[]).await.is_ok());
        assert!(assert_safe_url("http://10.0.0.5/x", &[]).await.is_ok());
        // Non-empty list opts into enforcement: internal blocked unless carved out.
        assert!(assert_safe_url("http://10.0.0.5/x", &["192.168.0.0/16".to_string()])
            .await
            .is_err());
        // Carve-out via allowedCidrs.
        assert!(assert_safe_url("http://10.0.0.5/x", &["10.0.0.0/8".to_string()])
            .await
            .is_ok());
        // Scheme fail-closed regardless of list.
        assert!(assert_safe_url("file:///etc/passwd", &[]).await.is_err());
    }
}
