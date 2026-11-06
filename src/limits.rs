//! Building blocks for abuse limits shared by the SMTP server and the web app: who a caller
//! is for counting purposes ([`network_of`]) and a counter per key over fixed windows
//! ([`Windows`]). Counts live in memory only; a restart forgets them.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Keys a counter holds before those whose window has ended are dropped.
const KEYS_KEPT: usize = 100_000;

/// The address a caller is counted under: an IPv4 address as it is, an IPv6 address as its
/// /64 network. A single IPv6 host usually controls a whole /64, so counting full addresses
/// would let it dodge every limit by changing the low bits. IPv4-mapped IPv6 addresses count
/// as the IPv4 address they carry.
pub fn network_of(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            let mut segments = v6.segments();
            segments[4..].fill(0);
            IpAddr::V6(Ipv6Addr::from(segments))
        }
    }
}

/// Counts events per key in fixed windows of `length`, starting at a key's first event.
pub struct Windows<K> {
    length: Duration,
    counts: Mutex<HashMap<K, (Instant, u32)>>,
}

impl<K: Eq + Hash> Windows<K> {
    pub fn new(length: Duration) -> Self {
        Self {
            length,
            counts: Mutex::default(),
        }
    }

    /// Counts one more event for `key` and returns the key's count in its current window,
    /// this event included.
    pub fn count(&self, key: K, now: Instant) -> u32 {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if counts.len() >= KEYS_KEPT {
            let length = self.length;
            counts.retain(|_, (start, _)| now.duration_since(*start) < length);
        }
        let (start, count) = counts.entry(key).or_insert((now, 0));
        if now.duration_since(*start) >= self.length {
            *start = now;
            *count = 0;
        }
        *count += 1;
        *count
    }

    /// The count for `key` in its current window, without counting anything.
    pub fn current(&self, key: &K, now: Instant) -> u32 {
        let counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        match counts.get(key) {
            Some((start, count)) if now.duration_since(*start) < self.length => *count,
            _ => 0,
        }
    }

    /// Drops the keys whose window has ended.
    pub fn forget_ended(&self, now: Instant) {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let length = self.length;
        counts.retain(|_, (start, _)| now.duration_since(*start) < length);
    }

    /// How many keys are held.
    pub fn len(&self) -> usize {
        self.counts.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_addresses_count_as_themselves() {
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(network_of(ip), ip);
    }

    #[test]
    fn ipv6_addresses_count_as_their_64_network() {
        let a: IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(network_of(a), "2001:db8:1:2::".parse::<IpAddr>().unwrap());
        assert_eq!(network_of(a), network_of(b));
        assert_ne!(network_of(a), network_of(other));
    }

    #[test]
    fn ipv4_mapped_ipv6_addresses_count_as_ipv4() {
        let mapped: IpAddr = "::ffff:203.0.113.7".parse().unwrap();
        assert_eq!(network_of(mapped), "203.0.113.7".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn a_key_counts_up_within_its_window_and_starts_again_after() {
        let windows = Windows::new(Duration::from_secs(60));
        let start = Instant::now();
        assert_eq!(windows.count("a", start), 1);
        assert_eq!(windows.count("a", start + Duration::from_secs(30)), 2);
        assert_eq!(windows.current(&"a", start + Duration::from_secs(59)), 2);
        assert_eq!(windows.current(&"a", start + Duration::from_secs(60)), 0);
        assert_eq!(windows.count("a", start + Duration::from_secs(60)), 1);
    }

    #[test]
    fn keys_are_counted_apart() {
        let windows = Windows::new(Duration::from_secs(60));
        let now = Instant::now();
        windows.count("a", now);
        windows.count("a", now);
        assert_eq!(windows.count("b", now), 1);
        assert_eq!(windows.current(&"a", now), 2);
    }

    #[test]
    fn forget_ended_drops_only_ended_windows() {
        let windows = Windows::new(Duration::from_secs(60));
        let start = Instant::now();
        windows.count("old", start);
        windows.count("new", start + Duration::from_secs(50));
        windows.forget_ended(start + Duration::from_secs(70));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows.current(&"new", start + Duration::from_secs(70)), 1);
    }
}
