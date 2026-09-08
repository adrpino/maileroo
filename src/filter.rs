use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use dashmap::{DashMap, DashSet};
use mail_parser::MessageParser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::db::DbPool;
use crate::db::labels::{
    batch_assign_label_to_emails, get_filters_for_engine, get_unlabeled_emails_for_user,
};

/// Maximum bytes of email body text to scan for keyword filtering (64 KB).
pub const MAX_FILTER_SCAN_BYTES: usize = 64 * 1024;

/// Strips basic HTML tags and normalizes spaces for search matching.
pub fn strip_html_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut inside_tag = false;
    for c in html.chars() {
        match c {
            '<' => inside_tag = true,
            '>' => {
                inside_tag = false;
                output.push(' ');
            }
            _ if !inside_tag => output.push(c),
            _ => {}
        }
    }
    output
}

/// Extracts up to `max_bytes` of searchable body text from a raw email.
/// Completely skips and ignores all binary attachments and non-text parts.
pub fn extract_searchable_body(raw_eml: &[u8], max_bytes: usize) -> String {
    let message = match MessageParser::default().parse(raw_eml) {
        Some(msg) => msg,
        None => return String::new(),
    };

    let body = if let Some(text) = message.body_text(0) {
        text.to_string()
    } else if let Some(html) = message.body_html(0) {
        strip_html_tags(&html)
    } else {
        String::new()
    };

    if body.len() > max_bytes {
        let mut end = max_bytes;
        while !body.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        body[..end].to_string()
    } else {
        body
    }
}

/// Compiled multi-pattern Aho-Corasick automaton for a single user's active filter rules.
pub struct CompiledFilterSet {
    automaton: Option<AhoCorasick>,
    label_ids: Vec<Uuid>,
}

impl CompiledFilterSet {
    pub fn new(rules: &[(String, Uuid)]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if rules.is_empty() {
            return Ok(Self {
                automaton: None,
                label_ids: Vec::new(),
            });
        }

        let mut patterns = Vec::with_capacity(rules.len());
        let mut label_ids = Vec::with_capacity(rules.len());

        for (kw, lid) in rules {
            let trimmed = kw.trim();
            if !trimmed.is_empty() {
                patterns.push(trimmed.to_string());
                label_ids.push(*lid);
            }
        }

        if patterns.is_empty() {
            return Ok(Self {
                automaton: None,
                label_ids: Vec::new(),
            });
        }

        let ac = AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::Standard)
            .build(&patterns)?;

        Ok(Self {
            automaton: Some(ac),
            label_ids,
        })
    }

    /// Single-pass O(N) multi-pattern match over the body text.
    /// Returns deduplicated label IDs.
    pub fn matches(&self, text: &str) -> Vec<Uuid> {
        let Some(ref ac) = self.automaton else {
            return Vec::new();
        };

        let mut matched = HashSet::new();
        for mat in ac.find_iter(text) {
            let pattern_id = mat.pattern().as_usize();
            if let Some(&label_id) = self.label_ids.get(pattern_id) {
                matched.insert(label_id);
            }
        }

        matched.into_iter().collect()
    }
}

/// Thread-safe in-memory cache of compiled filter rules per user.
#[derive(Clone, Default)]
pub struct FilterEngine {
    cache: Arc<DashMap<Uuid, Arc<CompiledFilterSet>>>,
}

impl FilterEngine {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Invalidates cached rules for a user when they add, update, or remove rules.
    pub fn invalidate(&self, user_id: &Uuid) {
        self.cache.remove(user_id);
    }

    /// Retrieves or compiles user filter rules from the database into RAM.
    pub async fn get_or_compile(
        &self,
        pool: &DbPool,
        user_id: Uuid,
    ) -> Result<Arc<CompiledFilterSet>, sqlx::Error> {
        if let Some(cached) = self.cache.get(&user_id) {
            return Ok(cached.clone());
        }

        let filter_pairs = get_filters_for_engine(pool, user_id).await?;
        let compiled =
            CompiledFilterSet::new(&filter_pairs).unwrap_or_else(|_| CompiledFilterSet {
                automaton: None,
                label_ids: Vec::new(),
            });

        let arc = Arc::new(compiled);
        self.cache.insert(user_id, arc.clone());
        Ok(arc)
    }

    /// Evaluates an email body against the user's active filter rules.
    /// Zero database queries are executed if rules are already cached in RAM.
    pub async fn evaluate(
        &self,
        pool: &DbPool,
        user_id: Uuid,
        raw_eml: &[u8],
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let compiled = self.get_or_compile(pool, user_id).await?;
        if compiled.automaton.is_none() {
            return Ok(Vec::new());
        }

        let text = extract_searchable_body(raw_eml, MAX_FILTER_SCAN_BYTES);
        if text.is_empty() {
            return Ok(Vec::new());
        }

        Ok(compiled.matches(&text))
    }
}

/// A background backfill task for a single user, keyword, and target label.
#[derive(Debug, Clone)]
pub struct BackfillJob {
    pub user_id: Uuid,
    pub keyword: String,
    pub label_id: Uuid,
}

/// Safe multi-tenant background backfill engine.
/// Ensures that at most one backfill job is active per tenant,
/// strictly serializes execution to bound file descriptors and DB connections,
/// and streams only the first 64 KB of each email.
#[derive(Clone)]
pub struct BackfillEngine {
    tx: mpsc::Sender<BackfillJob>,
    active_tenants: Arc<DashSet<Uuid>>,
}

impl Default for BackfillEngine {
    fn default() -> Self {
        let (tx, _) = mpsc::channel(100);
        Self {
            tx,
            active_tenants: Arc::new(DashSet::new()),
        }
    }
}

impl BackfillEngine {
    pub fn new() -> (Self, mpsc::Receiver<BackfillJob>) {
        let (tx, rx) = mpsc::channel(100);
        let active_tenants = Arc::new(DashSet::new());
        (Self { tx, active_tenants }, rx)
    }

    /// Submits a backfill job for a user if not already running.
    /// Returns true if successfully queued, false if a backfill job is already active for this user.
    pub fn submit(&self, job: BackfillJob) -> bool {
        if self.active_tenants.insert(job.user_id) {
            let _ = self.tx.try_send(job);
            true
        } else {
            false
        }
    }

    /// Pure async worker function that scans a user's existing emails in batches of 50.
    pub async fn process_job(pool: &DbPool, storage_dir: &Path, job: &BackfillJob) {
        let batch_size = 50;
        let mut offset = 0;

        let compiled = match CompiledFilterSet::new(&[(job.keyword.clone(), job.label_id)]) {
            Ok(c) => c,
            Err(_) => return,
        };

        loop {
            let candidates = match get_unlabeled_emails_for_user(
                pool,
                job.user_id,
                job.label_id,
                batch_size,
                offset,
            )
            .await
            {
                Ok(c) if c.is_empty() => break,
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Error fetching unlabeled emails during backfill: {}", e);
                    break;
                }
            };

            let count = candidates.len();
            let mut matched_ids = Vec::new();

            for (email_id, body_key) in &candidates {
                let eml_path = storage_dir.join(format!("{}.eml", body_key));
                if let Ok(file) = tokio::fs::File::open(&eml_path).await {
                    let mut reader =
                        tokio::io::BufReader::new(file).take(MAX_FILTER_SCAN_BYTES as u64);
                    let mut buffer = Vec::new();
                    if reader.read_to_end(&mut buffer).await.is_ok() {
                        let text = extract_searchable_body(&buffer, MAX_FILTER_SCAN_BYTES);
                        if !compiled.matches(&text).is_empty() {
                            matched_ids.push(*email_id);
                        }
                    }
                }
            }
            if !matched_ids.is_empty()
                && let Err(e) = batch_assign_label_to_emails(pool, &matched_ids, job.label_id).await
            {
                tracing::error!("Error assigning labels during backfill: {}", e);
            }

            let unmatched_count = count - matched_ids.len();

            if (count as i64) < batch_size {
                break;
            }

            offset += unmatched_count as i64;

            // Cooperative yield so live SMTP and web traffic always take priority
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Spawns the dedicated background worker loop that processes backfill jobs sequentially.
    pub fn start_worker(
        active_tenants: Arc<DashSet<Uuid>>,
        pool: DbPool,
        storage_dir: PathBuf,
        mut rx: mpsc::Receiver<BackfillJob>,
    ) {
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                Self::process_job(&pool, &storage_dir, &job).await;
                active_tenants.remove(&job.user_id);
            }
        });
    }

    pub fn active_tenants(&self) -> Arc<DashSet<Uuid>> {
        self.active_tenants.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compiled_filter_set_multi_match() {
        let label_billing = Uuid::new_v4();
        let label_receipts = Uuid::new_v4();
        let label_personal = Uuid::new_v4();

        let rules = vec![
            ("invoice".to_string(), label_billing),
            ("receipt".to_string(), label_receipts),
            ("personal".to_string(), label_personal),
        ];

        let filter_set = CompiledFilterSet::new(&rules).unwrap();

        let text = "Hello, here is your monthly invoice and a payment receipt attached.";
        let matches = filter_set.matches(text);

        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&label_billing));
        assert!(matches.contains(&label_receipts));
        assert!(!matches.contains(&label_personal));
    }

    #[test]
    fn test_case_insensitivity() {
        let label_id = Uuid::new_v4();
        let rules = vec![("UrGeNt".to_string(), label_id)];
        let filter_set = CompiledFilterSet::new(&rules).unwrap();

        assert_eq!(filter_set.matches("this is urgent!"), vec![label_id]);
        assert_eq!(filter_set.matches("THIS IS URGENT!"), vec![label_id]);
    }

    #[test]
    fn test_scan_ceiling_cap() {
        let long_text = format!("{} MATCH_KEYWORD", "a".repeat(70_000));
        let raw_eml = format!(
            "From: a@b.com\r\nSubject: Test\r\nContent-Type: text/plain\r\n\r\n{}",
            long_text
        );
        let extracted = extract_searchable_body(raw_eml.as_bytes(), 64 * 1024);
        assert_eq!(extracted.len(), 64 * 1024);
        assert!(!extracted.contains("MATCH_KEYWORD"));
    }

    #[test]
    fn test_html_tag_stripping() {
        let raw_html = "<div><p>Your <b>invoice</b> is ready</p></div>";
        let stripped = strip_html_tags(raw_html);
        assert!(stripped.contains("invoice"));
        assert!(!stripped.contains("<p>"));
    }

    #[test]
    fn test_backfill_engine_tenant_guard() {
        let (engine, mut rx) = BackfillEngine::new();
        let user_id = Uuid::new_v4();
        let label_id = Uuid::new_v4();

        let job1 = BackfillJob {
            user_id,
            keyword: "receipt".to_string(),
            label_id,
        };

        // First submit should succeed
        assert!(engine.submit(job1.clone()));
        assert!(engine.active_tenants().contains(&user_id));

        // Second submit while active should return false (guard drops duplicate)
        let job2 = BackfillJob {
            user_id,
            keyword: "invoice".to_string(),
            label_id,
        };
        assert!(!engine.submit(job2));

        // Drain the job from the receiver
        let received = rx.try_recv().unwrap();
        assert_eq!(received.user_id, user_id);

        // Clearing tenant unlocks them for future jobs
        engine.active_tenants().remove(&user_id);
        assert!(!engine.active_tenants().contains(&user_id));
        assert!(engine.submit(job1));
    }
}
