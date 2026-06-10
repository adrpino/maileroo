use dashmap::DashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct InboundLimits {
    pub tarpit_threshold: usize,
    pub block_threshold: usize,
    pub tarpit_duration_secs: u64,
}

impl InboundLimits {
    pub fn from_env() -> Self {
        let tarpit_threshold = crate::config::get_config("SMTP_TARPIT_THRESHOLD", "2")
            .parse()
            .unwrap_or(2);
        let block_threshold = crate::config::get_config("SMTP_BLOCK_THRESHOLD", "5")
            .parse()
            .unwrap_or(5);
        let tarpit_duration_secs = crate::config::get_config("SMTP_TARPIT_DURATION_SECS", "30")
            .parse()
            .unwrap_or(30);

        Self {
            tarpit_threshold,
            block_threshold,
            tarpit_duration_secs,
        }
    }
}

struct RateLimitEntry {
    failures: usize,
    last_seen: Instant,
}

pub struct RateLimiter {
    map: DashMap<IpAddr, RateLimitEntry>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Records a failure and returns the total number of recent failures for this IP.
    pub fn record_failure(&self, ip: IpAddr) -> usize {
        let mut entry = self.map.entry(ip).or_insert(RateLimitEntry {
            failures: 0,
            last_seen: Instant::now(),
        });

        entry.failures += 1;
        entry.last_seen = Instant::now();

        entry.failures
    }

    /// Removes entries that haven't been seen within the given max_age.
    pub fn cleanup(&self, max_age: Duration) {
        let now = Instant::now();
        self.map
            .retain(|_, entry| now.duration_since(entry.last_seen) < max_age);
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

        assert_eq!(limiter.record_failure(ip), 1);
        assert_eq!(limiter.record_failure(ip), 2);
        assert_eq!(limiter.record_failure(ip), 3);
    }

    #[test]
    fn test_multiple_ips_are_isolated() {
        let limiter = RateLimiter::new();
        let ip1 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

        assert_eq!(limiter.record_failure(ip1), 1);
        assert_eq!(limiter.record_failure(ip2), 1);
        assert_eq!(limiter.record_failure(ip1), 2);
    }

    #[test]
    fn test_cleanup_removes_old_entries() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        // Directly insert an old entry to simulate time passing
        limiter.map.insert(
            ip,
            RateLimitEntry {
                failures: 5,
                last_seen: Instant::now() - Duration::from_secs(4000), // Older than 1 hour (3600s)
            },
        );

        assert_eq!(limiter.map.len(), 1);

        limiter.cleanup(Duration::from_secs(3600));

        assert_eq!(limiter.map.len(), 0);
    }
}
