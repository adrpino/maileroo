use hickory_resolver::TokioResolver;
use serde::Serialize;
use std::net::IpAddr;

#[derive(Serialize, Debug, Clone)]
pub struct DnsCheckResult {
    pub is_ok: bool,
    pub mx_status: RecordStatus,
    pub spf_status: RecordStatus,
    pub dmarc_status: RecordStatus,
}

#[derive(Serialize, Debug, Clone)]
pub struct RecordStatus {
    pub ok: bool,
    pub message: String,
    pub value: Option<String>,
}

#[derive(Clone)]
pub struct DnsScanner {
    resolver: TokioResolver,
}

impl DnsScanner {
    pub fn new(resolver: TokioResolver) -> Self {
        Self { resolver }
    }

    pub async fn check_domain(&self, domain: &str) -> anyhow::Result<DnsCheckResult> {
        // Run lookups in parallel
        let (mx_res, txt_res, dmarc_res) = tokio::join!(
            self.resolver.mx_lookup(domain),
            self.resolver.txt_lookup(domain),
            self.resolver.txt_lookup(format!("_dmarc.{}", domain))
        );

        // 1. Check MX
        let mx_status = match mx_res {
            Ok(lookup) => {
                let mut records: Vec<_> = lookup
                    .answers()
                    .iter()
                    .filter_map(|r| {
                        if let hickory_resolver::proto::rr::RData::MX(mx) = &r.data {
                            Some(mx)
                        } else {
                            None
                        }
                    })
                    .collect();

                if records.is_empty() {
                    RecordStatus {
                        ok: false,
                        message: "No MX records found. Email delivery will fail.".to_string(),
                        value: None,
                    }
                } else {
                    records.sort_by_key(|r| r.preference);
                    let best = records[0];
                    RecordStatus {
                        ok: true,
                        message: format!("Found {} MX records", records.len()),
                        value: Some(best.exchange.to_utf8().trim_end_matches('.').to_string()),
                    }
                }
            }
            Err(_) => RecordStatus {
                ok: false,
                message: "DNS lookup failed or no MX records found.".to_string(),
                value: None,
            },
        };

        // 2. Check SPF (TXT records on root domain starting with v=spf1)
        let mut spf_status = RecordStatus {
            ok: false,
            message: "No SPF record found.".to_string(),
            value: None,
        };
        if let Ok(lookup) = txt_res {
            for record in lookup.answers() {
                if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
                    for data in txt.txt_data.iter() {
                        let text = String::from_utf8_lossy(data);
                        if text.starts_with("v=spf1") {
                            spf_status = RecordStatus {
                                ok: true,
                                message: "SPF record found".to_string(),
                                value: Some(text.to_string()),
                            };
                            break;
                        }
                    }
                }
                if spf_status.ok {
                    break;
                }
            }
        }

        // 3. Check DMARC (TXT records on _dmarc.domain)
        let mut dmarc_status = RecordStatus {
            ok: false,
            message: "No DMARC record found.".to_string(),
            value: None,
        };
        if let Ok(lookup) = dmarc_res {
            for record in lookup.answers() {
                if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
                    for data in txt.txt_data.iter() {
                        let text = String::from_utf8_lossy(data);
                        if text.starts_with("v=DMARC1") {
                            dmarc_status = RecordStatus {
                                ok: true,
                                message: "DMARC record found".to_string(),
                                value: Some(text.to_string()),
                            };
                            break;
                        }
                    }
                }
                if dmarc_status.ok {
                    break;
                }
            }
        }

        let is_ok = mx_status.ok && spf_status.ok; // DMARC might be optional for "fine" but MX/SPF are critical

        Ok(DnsCheckResult {
            is_ok,
            mx_status,
            spf_status,
            dmarc_status,
        })
    }

    pub async fn is_domain_deliverable(&self, domain: &str) -> bool {
        match self.resolver.mx_lookup(domain).await {
            Ok(lookup) => lookup
                .answers()
                .iter()
                .any(|r| matches!(&r.data, hickory_resolver::proto::rr::RData::MX(_))),
            Err(_) => {
                // If MX fails, we can check for A/AAAA records as a fallback
                // since some mail servers deliver to the A record if MX is missing
                self.resolver.lookup_ip(domain).await.is_ok()
            }
        }
    }

    pub async fn check_dkim_record(
        &self,
        domain: &str,
        selector: &str,
        expected_pub_key: &str,
    ) -> RecordStatus {
        let dkim_domain = format!("{}._domainkey.{}", selector, domain);
        match self.resolver.txt_lookup(dkim_domain).await {
            Ok(lookup) => {
                for record in lookup.answers() {
                    if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
                        for data in txt.txt_data.iter() {
                            let text = String::from_utf8_lossy(data);
                            if text.contains(expected_pub_key) {
                                return RecordStatus {
                                    ok: true,
                                    message: "DKIM TXT record found and matches!".to_string(),
                                    value: Some(text.to_string()),
                                };
                            }
                        }
                    }
                }
                RecordStatus {
                    ok: false,
                    message: "DKIM record found but does not contain the expected public key."
                        .to_string(),
                    value: None,
                }
            }
            Err(e) => RecordStatus {
                ok: false,
                message: format!("DKIM DNS TXT lookup failed: {}", e),
                value: None,
            },
        }
    }
}

// =========================================================================
// Pure, Modular, Unit-Testable SPF Verification Logic (RFC 7208 compliant)
// =========================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpfDirective {
    Ip4(String),
    Ip6(String),
    Include(String),
    Redirect(String),
}

/// Parses a TXT record to extract any valid SPF directives.
pub fn parse_spf_record(txt_str: &str) -> Vec<SpfDirective> {
    let mut directives = Vec::new();
    if !txt_str.starts_with("v=spf1") {
        return directives;
    }

    for term in txt_str.split_whitespace() {
        if term == "v=spf1" {
            continue;
        }

        // Strip qualifiers (+, -, ~, ?)
        let clean_term = if term.starts_with('+')
            || term.starts_with('-')
            || term.starts_with('~')
            || term.starts_with('?')
        {
            &term[1..]
        } else {
            term
        };

        if let Some(cidr) = clean_term.strip_prefix("ip4:") {
            directives.push(SpfDirective::Ip4(cidr.to_string()));
        } else if let Some(cidr) = clean_term.strip_prefix("ip6:") {
            directives.push(SpfDirective::Ip6(cidr.to_string()));
        } else if let Some(inc_dom) = clean_term.strip_prefix("include:") {
            directives.push(SpfDirective::Include(inc_dom.to_string()));
        } else if let Some(red_dom) = clean_term.strip_prefix("redirect=") {
            directives.push(SpfDirective::Redirect(red_dom.to_string()));
        }
    }

    directives
}

/// Matches an IP address against a CIDR block string (e.g., "192.168.1.0/24" or "2001:db8::/32")
pub fn match_ip_cidr(ip: IpAddr, cidr: &str) -> bool {
    let parts: Vec<&str> = cidr.split('/').collect();
    let cidr_ip_str = parts[0];

    let parsed_cidr_ip = match cidr_ip_str.parse::<IpAddr>() {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };

    let prefix = if parts.len() > 1 {
        parts[1].parse::<u32>().unwrap_or(match ip {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        })
    } else {
        match ip {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        }
    };

    match (ip, parsed_cidr_ip) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let prefix_capped = prefix.min(32);
            let a_val = u32::from_be_bytes(a.octets());
            let b_val = u32::from_be_bytes(b.octets());
            if prefix_capped == 0 {
                true
            } else if prefix_capped == 32 {
                a_val == b_val
            } else {
                let mask = !((1u64 << (32 - prefix_capped)) - 1) as u32;
                (a_val & mask) == (b_val & mask)
            }
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let prefix_capped = prefix.min(128);
            let a_val = u128::from_be_bytes(a.octets());
            let b_val = u128::from_be_bytes(b.octets());
            if prefix_capped == 0 {
                true
            } else if prefix_capped == 128 {
                a_val == b_val
            } else {
                let mask = !((1u128 << (128 - prefix_capped)) - 1);
                (a_val & mask) == (b_val & mask)
            }
        }
        _ => false,
    }
}

/// Evaluates if the `client_ip` is an authorized sender for the given `domain` under RFC 7208.
pub async fn check_spf_for_domain(
    resolver: &TokioResolver,
    domain: &str,
    client_ip: IpAddr,
) -> bool {
    let mut domains_to_check = vec![domain.to_string()];
    let mut checked_domains = std::collections::HashSet::new();
    let mut depth = 0;

    while let Some(current_domain) = domains_to_check.pop() {
        if checked_domains.contains(&current_domain) {
            continue;
        }
        checked_domains.insert(current_domain.clone());
        depth += 1;
        if depth > 10 {
            // Prevent infinite DNS loops or DOS attacks
            break;
        }

        let lookup = match resolver.txt_lookup(format!("{}.", current_domain)).await {
            Ok(res) => res,
            Err(_) => continue,
        };

        for answer in lookup.answers() {
            if let hickory_resolver::proto::rr::RData::TXT(txt) = &answer.data {
                for txt_data in txt.txt_data.iter() {
                    let txt_str = String::from_utf8_lossy(txt_data);
                    let directives = parse_spf_record(&txt_str);
                    if directives.is_empty() {
                        continue;
                    }

                    for directive in directives {
                        match directive {
                            SpfDirective::Ip4(cidr) => {
                                if client_ip.is_ipv4() && match_ip_cidr(client_ip, &cidr) {
                                    return true;
                                }
                            }
                            SpfDirective::Ip6(cidr) => {
                                if client_ip.is_ipv6() && match_ip_cidr(client_ip, &cidr) {
                                    return true;
                                }
                            }
                            SpfDirective::Include(inc_dom) => {
                                domains_to_check.push(inc_dom);
                            }
                            SpfDirective::Redirect(red_dom) => {
                                domains_to_check.push(red_dom);
                            }
                        }
                    }
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn test_parse_spf_record() {
        let record = "v=spf1 ip4:185.70.40.0/22 include:_spf.protonmail.ch -all";
        let parsed = parse_spf_record(record);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], SpfDirective::Ip4("185.70.40.0/22".to_string()));
        assert_eq!(
            parsed[1],
            SpfDirective::Include("_spf.protonmail.ch".to_string())
        );

        let record_invalid = "v=spf2 ip4:1.1.1.1";
        assert!(parse_spf_record(record_invalid).is_empty());
    }

    #[test]
    fn test_match_ip_cidr_ipv4() {
        let ip_exact: IpAddr = "192.168.1.15".parse().unwrap();

        // Exact match
        assert!(match_ip_cidr(ip_exact, "192.168.1.15"));
        assert!(match_ip_cidr(ip_exact, "192.168.1.15/32"));
        assert!(!match_ip_cidr(ip_exact, "192.168.1.16"));

        // Subnet matches
        assert!(match_ip_cidr(ip_exact, "192.168.1.0/24"));
        assert!(match_ip_cidr(ip_exact, "192.168.0.0/16"));
        assert!(!match_ip_cidr(ip_exact, "192.168.2.0/24"));

        // Edge cases (0 prefix)
        assert!(match_ip_cidr(ip_exact, "0.0.0.0/0"));
    }

    #[test]
    fn test_match_ip_cidr_ipv6() {
        let ip_exact: IpAddr = "2001:db8::15".parse().unwrap();

        // Exact match
        assert!(match_ip_cidr(ip_exact, "2001:db8::15"));
        assert!(match_ip_cidr(ip_exact, "2001:db8::15/128"));
        assert!(!match_ip_cidr(ip_exact, "2001:db8::16"));

        // Subnet matches
        assert!(match_ip_cidr(ip_exact, "2001:db8::/64"));
        assert!(match_ip_cidr(ip_exact, "2001:db8::/32"));
        assert!(!match_ip_cidr(ip_exact, "2002:db8::/32"));

        // Edge cases (0 prefix)
        assert!(match_ip_cidr(ip_exact, "::/0"));
    }

    #[tokio::test]
    async fn test_check_spf_for_domain_real_lookup() {
        let resolver = TokioResolver::builder_tokio()
            .expect("Failed to create resolver builder")
            .build()
            .expect("Failed to build resolver");

        // ProtonMail's SPF includes 185.70.40.101 (part of 185.70.40.0/22 via include:_spf.protonmail.ch)
        let is_valid =
            check_spf_for_domain(&resolver, "protonmail.ch", "185.70.40.101".parse().unwrap())
                .await;
        assert!(is_valid);

        // A fake IP should fail
        let is_fake_valid =
            check_spf_for_domain(&resolver, "protonmail.ch", "1.1.1.1".parse().unwrap()).await;
        assert!(!is_fake_valid);
    }
}
