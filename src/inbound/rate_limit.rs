//! Slows down and briefly blocks callers that probe for addresses: each `RCPT TO` for an
//! address that doesn't exist is a miss, counted per network ([`network_of`]) over an hour.
//! Past [`InboundLimits::tarpit_threshold`] misses each further miss is answered slowly;
//! past [`InboundLimits::block_threshold`] the network is refused for
//! [`InboundLimits::block_duration_secs`]. Blocks end on their own, so a legitimate server
//! that sent a few messages to a deleted alias is not shut out for good. Simultaneous
//! connections per network are capped too, so slowed connections can't pile up.

use crate::limits::{Windows, network_of};
use dashmap::DashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long misses are counted for before a network's count starts again.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy)]
pub struct InboundLimits {
    /// Misses in an hour after which each further miss is answered slowly.
    pub tarpit_threshold: usize,
    /// Misses in an hour after which the network is blocked.
    pub block_threshold: usize,
    /// How long a slowed answer waits.
    pub tarpit_duration_secs: u64,
    /// How long a block lasts.
    pub block_duration_secs: u64,
    /// Connections one network may hold open at once.
    pub max_connections_per_network: usize,
}

impl Default for InboundLimits {
    fn default() -> Self {
        Self {
            tarpit_threshold: 3,
            block_threshold: 10,
            tarpit_duration_secs: 30,
            block_duration_secs: 3600,
            max_connections_per_network: 10,
        }
    }
}

impl InboundLimits {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            tarpit_threshold: env_or("SMTP_TARPIT_THRESHOLD", defaults.tarpit_threshold),
            block_threshold: env_or("SMTP_BLOCK_THRESHOLD", defaults.block_threshold),
            tarpit_duration_secs: env_or(
                "SMTP_TARPIT_DURATION_SECS",
                defaults.tarpit_duration_secs,
            ),
            block_duration_secs: env_or("SMTP_BLOCK_DURATION_SECS", defaults.block_duration_secs),
            max_connections_per_network: env_or(
                "SMTP_MAX_CONNECTIONS_PER_NETWORK",
                defaults.max_connections_per_network,
            ),
        }
    }

    /// What to do about a caller's `misses`-th miss this hour.
    pub fn verdict(&self, misses: usize) -> MissVerdict {
        if misses >= self.block_threshold {
            MissVerdict::Block
        } else if misses >= self.tarpit_threshold {
            MissVerdict::Tarpit
        } else {
            MissVerdict::Reject
        }
    }
}

fn env_or<T: std::str::FromStr + ToString>(key: &str, default: T) -> T {
    crate::config::get_config(key, &default.to_string())
        .parse()
        .unwrap_or(default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissVerdict {
    /// Answer "no such user" straight away.
    Reject,
    /// Wait, then answer "no such user".
    Tarpit,
    /// Block the network and close the connection.
    Block,
}

pub struct RateLimiter {
    misses: Windows<IpAddr>,
    blocked_until: DashMap<IpAddr, Instant>,
    open: Arc<DashMap<IpAddr, usize>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            misses: Windows::new(FAILURE_WINDOW),
            blocked_until: DashMap::new(),
            open: Arc::default(),
        }
    }

    /// Records a miss from `ip` and returns its network's misses this hour.
    pub fn record_failure(&self, ip: IpAddr, now: Instant) -> usize {
        self.misses.count(network_of(ip), now) as usize
    }

    /// Refuses `ip`'s network until `until`.
    pub fn block(&self, ip: IpAddr, until: Instant) {
        self.blocked_until.insert(network_of(ip), until);
    }

    /// Whether `ip`'s network is blocked at `now`.
    pub fn is_blocked(&self, ip: IpAddr, now: Instant) -> bool {
        self.blocked_until
            .get(&network_of(ip))
            .is_some_and(|until| now < *until)
    }

    /// Takes one of `max` connection slots for `ip`'s network, or `None` if all are taken.
    /// The slot is given back when the returned value is dropped.
    pub fn open_connection(&self, ip: IpAddr, max: usize) -> Option<OpenConnection> {
        let network = network_of(ip);
        let mut open = self.open.entry(network).or_insert(0);
        if *open >= max {
            return None;
        }
        *open += 1;
        Some(OpenConnection {
            open: self.open.clone(),
            network,
        })
    }

    /// Forgets counts whose hour has ended and blocks that have run out.
    pub fn cleanup(&self, now: Instant) {
        self.misses.forget_ended(now);
        self.blocked_until.retain(|_, until| now < *until);
    }
}

/// A connection slot held by a network; given back on drop.
pub struct OpenConnection {
    open: Arc<DashMap<IpAddr, usize>>,
    network: IpAddr,
}

impl Drop for OpenConnection {
    fn drop(&mut self) {
        if let Some(mut open) = self.open.get_mut(&self.network) {
            *open = open.saturating_sub(1);
        }
        self.open.remove_if(&self.network, |_, open| *open == 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_record_failure_increments() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let now = Instant::now();

        assert_eq!(limiter.record_failure(ip, now), 1);
        assert_eq!(limiter.record_failure(ip, now), 2);
        assert_eq!(limiter.record_failure(ip, now), 3);
    }

    #[test]
    fn test_multiple_ips_are_isolated() {
        let limiter = RateLimiter::new();
        let ip1 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
        let now = Instant::now();

        assert_eq!(limiter.record_failure(ip1, now), 1);
        assert_eq!(limiter.record_failure(ip2, now), 1);
        assert_eq!(limiter.record_failure(ip1, now), 2);
    }

    #[test]
    fn misses_from_one_ipv6_network_count_together() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let a: IpAddr = "2001:db8::1".parse().unwrap();
        let b: IpAddr = "2001:db8::ffff:2".parse().unwrap();

        assert_eq!(limiter.record_failure(a, now), 1);
        assert_eq!(limiter.record_failure(b, now), 2);
    }

    #[test]
    fn misses_are_forgotten_after_an_hour() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        let start = Instant::now();

        for _ in 0..5 {
            limiter.record_failure(ip, start);
        }
        assert_eq!(limiter.record_failure(ip, start + FAILURE_WINDOW), 1);
    }

    #[test]
    fn a_block_ends_on_its_own() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        let now = Instant::now();
        let until = now + Duration::from_secs(60);

        limiter.block(ip, until);
        assert!(limiter.is_blocked(ip, now));
        assert!(!limiter.is_blocked(ip, until));
        assert!(!limiter.is_blocked(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 2)), now));
    }

    #[test]
    fn a_block_covers_the_whole_ipv6_network() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        limiter.block(
            "2001:db8::1".parse().unwrap(),
            now + Duration::from_secs(60),
        );

        assert!(limiter.is_blocked("2001:db8::abcd".parse().unwrap(), now));
        assert!(!limiter.is_blocked("2001:db8:0:1::1".parse().unwrap(), now));
    }

    #[test]
    fn test_cleanup_removes_old_entries() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let start = Instant::now();

        limiter.record_failure(ip, start);
        limiter.block(ip, start + Duration::from_secs(60));
        assert_eq!(limiter.misses.len(), 1);

        limiter.cleanup(start + FAILURE_WINDOW);

        assert_eq!(limiter.misses.len(), 0);
        assert!(limiter.blocked_until.is_empty());
    }

    #[test]
    fn connections_are_capped_per_network_and_slots_come_back() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));

        let first = limiter.open_connection(ip, 2).unwrap();
        let _second = limiter.open_connection(ip, 2).unwrap();
        assert!(limiter.open_connection(ip, 2).is_none());
        assert!(
            limiter
                .open_connection(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 2)), 2)
                .is_some()
        );

        drop(first);
        assert!(limiter.open_connection(ip, 2).is_some());
    }

    #[test]
    fn the_verdict_follows_the_thresholds() {
        let limits = InboundLimits {
            tarpit_threshold: 3,
            block_threshold: 5,
            ..InboundLimits::default()
        };
        assert_eq!(limits.verdict(1), MissVerdict::Reject);
        assert_eq!(limits.verdict(2), MissVerdict::Reject);
        assert_eq!(limits.verdict(3), MissVerdict::Tarpit);
        assert_eq!(limits.verdict(4), MissVerdict::Tarpit);
        assert_eq!(limits.verdict(5), MissVerdict::Block);
    }
}
