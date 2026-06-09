use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Subnet {
    pub network: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Subnet {
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let ip_u32 = u32::from_be_bytes(ip.octets());
        let net_u32 = u32::from_be_bytes(self.network.octets());
        if self.prefix == 0 {
            true
        } else if self.prefix >= 32 {
            ip_u32 == net_u32
        } else {
            let mask = u32::MAX << (32 - self.prefix);
            (ip_u32 & mask) == (net_u32 & mask)
        }
    }
}

pub struct BlocklistInner {
    pub ips: HashSet<IpAddr>,
    pub subnets: Vec<Ipv4Subnet>,
}

pub struct Blocklist {
    inner: RwLock<BlocklistInner>,
    file_path: std::path::PathBuf,
}

impl Blocklist {
    pub fn new(file_path: std::path::PathBuf) -> Self {
        let mut ips = HashSet::new();
        let mut subnets = Vec::new();

        if let Ok(content) = std::fs::read_to_string(&file_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("deny ") {
                    let clean = line["deny ".len()..].trim_end_matches(';').trim();
                    if clean.contains('/') {
                        if let Some((ip_str, prefix_str)) = clean.split_once('/') {
                            if let (Ok(ip), Ok(prefix)) = (ip_str.parse::<Ipv4Addr>(), prefix_str.parse::<u8>()) {
                                subnets.push(Ipv4Subnet { network: ip, prefix });
                            }
                        }
                    } else if let Ok(ip) = clean.parse::<IpAddr>() {
                        ips.insert(ip);
                    }
                }
            }
        }

        Self {
            inner: RwLock::new(BlocklistInner { ips, subnets }),
            file_path,
        }
    }

    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        let guard = self.inner.read().unwrap();
        if guard.ips.contains(&ip) {
            return true;
        }

        if let IpAddr::V4(ipv4) = ip {
            for subnet in &guard.subnets {
                if subnet.contains(ipv4) {
                    return true;
                }
            }
        }

        false
    }

    pub fn add_ip(&self, ip: IpAddr) -> std::io::Result<()> {
        // 1. Check memory first with a write-lock and insert
        let is_new = {
            let mut guard = self.inner.write().unwrap();
            guard.ips.insert(ip)
        };

        if is_new {
            // 2. Persist to file
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&self.file_path)?;
            writeln!(file, "deny {};", ip)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_ipv4_subnet_contains() {
        let subnet = Ipv4Subnet {
            network: "178.207.10.0".parse().unwrap(),
            prefix: 24,
        };
        assert!(subnet.contains("178.207.10.91".parse().unwrap()));
        assert!(subnet.contains("178.207.10.1".parse().unwrap()));
        assert!(!subnet.contains("178.207.11.91".parse().unwrap()));

        let subnet_host = Ipv4Subnet {
            network: "192.168.1.5".parse().unwrap(),
            prefix: 32,
        };
        assert!(subnet_host.contains("192.168.1.5".parse().unwrap()));
        assert!(!subnet_host.contains("192.168.1.6".parse().unwrap()));
    }

    #[test]
    fn test_blocklist_new_and_is_blocked() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "deny 1.2.3.4;").unwrap();
        writeln!(temp_file, "deny 5.6.7.0/24;").unwrap();
        writeln!(temp_file, "# some comment").unwrap();
        writeln!(temp_file, "   deny 8.8.8.8; ").unwrap();

        let blocklist = Blocklist::new(temp_file.path().to_path_buf());

        assert!(blocklist.is_blocked("1.2.3.4".parse().unwrap()));
        assert!(blocklist.is_blocked("5.6.7.8".parse().unwrap()));
        assert!(blocklist.is_blocked("8.8.8.8".parse().unwrap()));
        assert!(!blocklist.is_blocked("1.2.3.5".parse().unwrap()));
        assert!(!blocklist.is_blocked("5.6.8.1".parse().unwrap()));
    }

    #[test]
    fn test_blocklist_add_ip() {
        let temp_file = NamedTempFile::new().unwrap();
        let blocklist = Blocklist::new(temp_file.path().to_path_buf());

        let test_ip = "10.20.30.40".parse::<IpAddr>().unwrap();
        assert!(!blocklist.is_blocked(test_ip));

        blocklist.add_ip(test_ip).unwrap();
        assert!(blocklist.is_blocked(test_ip));

        // Verify it was persisted to file
        let content = std::fs::read_to_string(temp_file.path()).unwrap();
        assert!(content.contains("deny 10.20.30.40;"));

        // Adding again should not duplicate in file
        let file_len_before = content.len();
        blocklist.add_ip(test_ip).unwrap();
        let content_after = std::fs::read_to_string(temp_file.path()).unwrap();
        assert_eq!(content_after.len(), file_len_before);
    }
}
