# Fix: Outbound delivery infinite retry loop + strict-TLS delivery failures

**Status:** Open — ready for a developer to implement
**Severity:** High (self-inflicted mail-server abuse pattern; misleading success logs; every transient failure loops forever)
**Area:** `src/outbound/` (MX delivery + retry queue)
**Prereqs to read:** this document is self-contained; no prior context required.

---

## 1. TL;DR

A single legitimate outbound message got stuck retrying **forever, every ~30 seconds**, while the log claimed each attempt "successfully delivered". Two independent defects combine to cause this:

1. **Retry-loop defect (critical, delivery-agnostic):** On a transient failure, `OutboundService::send_raw` enqueues a **brand-new** queue job (new UUID, `attempts = 0`) and returns `Ok(())`. The queue worker calls `send_raw`, sees `Ok`, logs *"successfully delivered!"*, and deletes the job it was processing. Net effect on every retry: the old job is deleted and an identical fresh job is created, so `attempts` never increments, `max_attempts` is never reached, exponential backoff is never applied, and the message is retried on every queue tick until the end of time.

2. **TLS-policy defect (why it failed in the first place):** MX delivery on port 25 does **strict WebPKI certificate verification**. Many legitimate mail servers (including `*.mail.protection.outlook.com`) present a leaf certificate without a complete chain, so rustls rejects it as `UnknownIssuer`. Opportunistic STARTTLS for MX delivery is supposed to encrypt without enforcing certificate validity (RFC 7435 "opportunistic security"), which is what standard MTAs (Postfix et al.) do by default.

Either defect alone is a bug. Together they produced an infinite, silent, IP-reputation-damaging loop.

A third, contributing issue: **there is no test covering a retry that fails.** Existing queue tests only cover (a) first send fails → enqueue, and (b) retry succeeds → cleanup. The failing-retry path — the one that loops — is untested, which is why this shipped.

---

## 2. Incident evidence

### 2.1 Server logs (repeating every ~30s, one block per queue tick)

```
INFO  maileroo::outbound::queue: Queue job <UUID-A> delivery attempt 1/10...
INFO  maileroo::outbound: Looking up MX records for domain: empiric.com
INFO  maileroo::outbound: Found best MX for empiric.com: empiric-com.mail.protection.outlook.com (pref: 0)
INFO  maileroo::outbound: Connecting via IPv4 to 52.101.68.16:25 ...
INFO  maileroo::outbound: STARTTLS detected, initiating upgrade...
WARN  maileroo::outbound: Transient delivery failure, automatically queuing for retry: TLS connection failed: invalid peer certificate: UnknownIssuer
INFO  maileroo::outbound::queue: Queue job <UUID-A> successfully delivered!     <-- FALSE
INFO  maileroo::outbound::queue: Queue job <UUID-B> delivery attempt 1/10...     <-- new UUID, attempt resets to 1
...
```

Tells:
- The UUID changes every tick — a new job is created each time.
- It is always `attempt 1/10` — `attempts` never advances.
- `WARN ... queuing for retry` and `successfully delivered!` appear for the *same* tick — the failure is being reported as success.

### 2.2 Database (live, production Postgres)

```
 to_recipient             | from_envelope         | attempts | status  | created_at
 azaan.iqbal@empiric.com  | hello@protocol4.trade |    0     | pending | 2026-07-13 17:08:...
```

Exactly **one** row at any moment (the loop deletes-and-recreates), `attempts = 0` always, and `from_envelope` is our own domain (`EMAIL_DOMAINS=protocol4.trade`). **This was our own message, not external abuse.**

### 2.3 TLS root cause reproduced from the mail host

```
$ openssl s_client -starttls smtp -connect empiric-com.mail.protection.outlook.com:25 \
      -servername empiric-com.mail.protection.outlook.com
subject= CN = mail.protection.outlook.com
issuer=  DigiCert Cloud Services CA-1
verify error:num=20: unable to get local issuer certificate
Verify return code: 20 (unable to get local issuer certificate)
```

Even `openssl` with the system CA bundle cannot build the chain — the server does not send the intermediate. Strict verification is the wrong policy for opportunistic MX delivery.

### 2.4 Immediate remediation already performed

The stuck job was drained from production to stop the loop hitting Outlook:

```sql
DELETE FROM outbound_queue WHERE to_recipient = 'azaan.iqbal@empiric.com';
```
plus removal of the orphaned `storage/emails/outbound/*.eml` files. Queue confirmed empty and stable across multiple ticks afterward. **This did not fix the code** — the next transient failure to any recipient will reproduce the loop. That is what this document is for.

---

## 3. Root cause analysis (code)

### 3.1 Defect 1 — transient failure re-enqueues inside the delivery call

`src/outbound/mod.rs`, `send_raw` (the wrapper around `send_raw_inner`):

```rust
// src/outbound/mod.rs  (approx lines 196-237)
pub(crate) async fn send_raw(&self, to, from_envelope, body) -> anyhow::Result<()> {
    match self.send_raw_inner(to, from_envelope, body).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let err_str = e.to_string();
            let is_permanent = err_str.contains("550") || err_str.contains("554")
                || err_str.contains("552") || err_str.contains("501")
                || err_str.contains("Invalid recipient")
                || err_str.contains("Command contains invalid characters");

            if !is_permanent {
                tracing::warn!("Transient delivery failure, automatically queuing for retry: {}", err_str);
                // >>> creates a NEW job (new UUID, attempts = 0) <<<
                crate::outbound::queue::enqueue_job(&self.db, &self.storage_dir, from_envelope, to, body).await?;
                Ok(())              // <<< returns Ok even though nothing was delivered
            } else {
                tracing::error!("Permanent delivery failure, skipping queue: {}", err_str);
                Err(e)
            }
        }
    }
}
```

`src/outbound/queue.rs`, `process_queue_tick` calls this same `send_raw`:

```rust
// src/outbound/queue.rs  (approx lines 118-159)
match outbound.send_raw(&job.to_recipient, &job.from_envelope, &body_bytes).await {
    Ok(_) => {
        tracing::info!("Queue job {} successfully delivered!", job_id);   // reached on transient failure!
        crate::db::queue::delete_job(pool, job_id).await;
        tokio::fs::remove_file(&file_path).await;
    }
    Err(e) => {
        // attempts+1, calculate_next_retry(), max_attempts handling...
        // THIS BRANCH IS DEAD CODE for transient failures, because send_raw
        // swallowed the Err into an Ok after enqueuing a fresh job.
    }
}
```

**Why it loops forever, precisely:**

- `enqueue_job` → `insert_job` sets `next_retry_at = now` (`src/db/queue.rs:36`), so the fresh job is eligible on the very next tick.
- `send_raw` returns `Ok`, so the worker takes the success branch, deletes the *current* job, and logs delivery.
- The freshly enqueued job has `attempts = 0`, so `next_attempt >= max_attempts` is never true.
- `calculate_next_retry` (correct exponential backoff, `src/outbound/queue.rs:46`) only runs in the worker's `Err` branch, which never executes. Backoff is therefore never applied — it retries at the raw tick interval (30s, `src/server.rs:161-166`).

Result: deleted-and-recreated forever, `attempts` pinned at 0, no backoff, no give-up, "success" logged each time.

> Note: the auto-enqueue behaviour is **correct and desired for the *first* send** (from web compose / inbound forward / reply), where the caller wants "accept and queue on transient failure, return Ok". The bug is that the **queue worker reuses the same entry point**, so retries re-enter the enqueue logic instead of feeding the worker's own accounting.

### 3.2 Defect 2 — strict TLS verification on opportunistic MX STARTTLS

`src/outbound/mod.rs`:

- The single `client_config` is built with full root-store verification (`OutboundService::new`, approx lines 94-103): `ClientConfig::builder().with_root_certificates(root_store).with_no_client_auth()`.
- The MX STARTTLS path uses that same config (`send_raw_inner`, approx lines 379-408): `TlsConnector::from(self.client_config.clone())` → `connector.connect(server_name, stream)` → on any verification error returns `Err("TLS connection failed: ...")`.

For MX-to-MX delivery on port 25, strict verification is contrary to standard practice. The connection should be **encrypted but not certificate-validated** (opportunistic security). The relay path (`src/outbound/relay.rs`, authenticated submission to a known relay host) should **keep** strict verification.

### 3.3 Defect 3 — test gap

`tests/queue_tests.rs` has two tests:
- `test_outbound_queue_retry_lifecycle_e2e`: first send fails (drop-connection mock) → enqueued; then retry against a **healthy** mock → delivered → cleaned up.
- `test_outbound_queue_daemon_periodic_delivery_e2e`: pre-seeded job → healthy mock → delivered.

Neither drives a **retry that fails**. That single missing scenario is exactly the loop. Section 5 adds it.

---

## 4. The fix

Implement all of 4.1 and 4.2. 4.1 stops the loop for any transient failure to any recipient; 4.2 makes delivery to strict/incomplete-chain servers (Outlook and many others) actually succeed. They are independent — do both.

Follow `AGENTS.md`: top-level `use` imports, English-only comments, keep modules cohesive, unit-test pure functions, and run integration tests on both SQLite and Postgres via `common::run_on_all_dbs`.

### 4.1 Stop the retry loop

**Goal:** the queue worker must observe the *real* delivery result and own all retry accounting. First-hand sends keep their "enqueue on transient, return Ok" behaviour.

**Step 1 — expose a non-enqueuing delivery entry point.**
`send_raw_inner` already performs delivery and returns the true `Result`. Expose it to the queue worker without the auto-enqueue wrapper. Minimal change: add a thin method on `OutboundService`:

```rust
/// Attempts a single direct delivery and returns the real result.
/// Unlike `send_raw`, this NEVER auto-enqueues on failure. The outbound
/// queue worker owns retry scheduling, so it must call this, not `send_raw`.
pub(crate) async fn deliver_once(&self, to: &str, from_envelope: &str, body: &[u8]) -> anyhow::Result<()> {
    self.send_raw_inner(to, from_envelope, body).await
}
```

**Step 2 — extract error classification into a shared pure function** (unit-testable per `AGENTS.md`). Put it in `src/outbound/mod.rs` (or a small `outbound/classify.rs`):

```rust
/// Classifies an SMTP/delivery error string as permanent (do not retry) or transient (retry).
/// Permanent = hard 5xx and malformed-input conditions; everything else is transient
/// (connection resets, timeouts, TLS errors, 4xx greylisting, DNS, etc.).
pub fn is_permanent_delivery_error(err: &str) -> bool {
    err.contains("550") || err.contains("554") || err.contains("552") || err.contains("501")
        || err.contains("Invalid recipient")
        || err.contains("Command contains invalid characters")
}
```

Use it in **both** `send_raw` (first-hand path, unchanged behaviour) and the queue worker.

**Step 3 — rewrite `process_queue_tick`'s delivery match** (`src/outbound/queue.rs`) to call `deliver_once` and do all accounting itself:

```rust
match outbound.deliver_once(&job.to_recipient, &job.from_envelope, &body_bytes).await {
    Ok(_) => {
        tracing::info!("Queue job {} successfully delivered.", job_id);
        let _ = crate::db::queue::delete_job(pool, job_id).await;
        let _ = tokio::fs::remove_file(&file_path).await;
    }
    Err(e) => {
        let err_msg = e.to_string();
        let next_attempt = job.attempts + 1;

        // Permanent failures fail fast; do not consume the whole retry budget.
        if crate::outbound::is_permanent_delivery_error(&err_msg) {
            tracing::error!("Queue job {} permanently failed: {}", job_id, err_msg);
            let _ = crate::db::queue::update_job_status(
                pool, job_id, "failed", next_attempt, Some(&err_msg),
                OffsetDateTime::now_utc() + time::Duration::days(365),
            ).await;
        } else if next_attempt >= job.max_attempts {
            tracing::error!("Queue job {} exceeded max retries ({}). Marking failed.", job_id, job.max_attempts);
            let _ = crate::db::queue::update_job_status(
                pool, job_id, "failed", next_attempt, Some(&err_msg),
                OffsetDateTime::now_utc() + time::Duration::days(365),
            ).await;
        } else {
            let next_retry = calculate_next_retry(next_attempt);
            tracing::warn!("Queue job {} attempt {}/{} failed, retrying at {}: {}",
                job_id, next_attempt, job.max_attempts, next_retry, err_msg);
            let _ = crate::db::queue::update_job_status(
                pool, job_id, "pending", next_attempt, Some(&err_msg), next_retry,
            ).await;
        }
    }
}
```

**Step 4 — guard the boundary.** Add a comment on `send_raw` stating it must not be used by the queue worker, and confirm no other caller of `send_raw`/`send_firsthand`/`send_reply`/`send_forward*` runs *inside* the queue worker. (These first-hand entry points legitimately keep the enqueue-on-transient behaviour — that is how a message first enters the queue.)

**Post-conditions after this step (asserted by tests in §5):**
- A failing retry keeps the **same** job id, increments `attempts` by 1, sets `status='pending'`, and sets `next_retry_at` in the **future** (backoff).
- The `outbound_queue` row **count does not grow** on a failed retry (no duplicate UUID).
- A permanent (5xx) failure sets `status='failed'` immediately.
- Reaching `max_attempts` (default 10) sets `status='failed'` and stops.

### 4.2 Use opportunistic TLS for MX delivery

**Goal:** encrypt MX connections but do not reject on certificate-chain/issuer problems. Keep strict verification for the authenticated relay path.

**Step 1 — add a second, non-verifying client config** used only for MX STARTTLS. Store it on `OutboundService` (e.g. `mx_tls_config: Arc<ClientConfig>`) alongside the existing strict `client_config`.

**Step 2 — implement an accept-all verifier** (rustls 0.23 / `tokio_rustls`). Delegate signature checks to the installed crypto provider so the handshake still completes correctly; only the trust-anchor/hostname check is skipped:

```rust
use std::sync::Arc;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{DigitallySignedStruct, Error as RustlsError, SignatureScheme};

/// Opportunistic-TLS verifier for port-25 MX delivery: encrypts the channel but does
/// not validate the certificate chain or hostname (RFC 7435). Standard MTA behaviour.
/// MUST NOT be used for the authenticated relay path, which keeps strict verification.
#[derive(Debug)]
struct OpportunisticVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for OpportunisticVerifier {
    fn verify_server_cert(
        &self, _end_entity: &CertificateDer<'_>, _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>, _ocsp: &[u8], _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct)
        -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct)
        -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
```

Build the config in `OutboundService::new`:

```rust
let provider = Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider());
let mx_tls_config = Arc::new(
    ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(OpportunisticVerifier { provider: provider.clone() }))
        .with_no_client_auth(),
);
```

(The process already installs the `aws_lc_rs` provider; tests install it via `common::init_crypto_provider()`.)

**Step 3 — use `mx_tls_config` in the MX STARTTLS path only.** In `send_raw_inner`, change `TlsConnector::from(self.client_config.clone())` to `TlsConnector::from(self.mx_tls_config.clone())`. Leave `src/outbound/relay.rs` using the strict `client_config`.

**Optional hardening (nice-to-have, not required):** if the TLS upgrade still fails for some server, fall back to plaintext delivery on port 25 rather than failing the whole attempt (true opportunistic behaviour). This needs a fresh connection since STARTTLS already consumed the greeting. Document as a follow-up if not implemented now.

**Trade-off to record in the PR:** opportunistic TLS accepts an active MITM on the SMTP path. This is the accepted industry norm for port-25 MX delivery and strictly better than the plaintext alternative. Stronger guarantees (DANE/TLSA, MTA-STS) are a separate future enhancement and out of scope here.

---

## 5. Tests (regression coverage so this cannot recur)

All integration tests go through `common::run_on_all_dbs(|db| async move { ... })` so they run on **both** SQLite (in-memory) and Postgres (when `TEST_DATABASE_URL` is set), per `AGENTS.md`. The existing `tests/queue_tests.rs` mock-SMTP pattern (drop-connection for transient failure; scripted handshake for success) is the template — reuse it.

### 5.1 Unit test — error classification (`src/outbound/…`, `#[cfg(test)]`)

```rust
#[test]
fn permanent_vs_transient_classification() {
    assert!(is_permanent_delivery_error("SMTP Error ...: 550 No such user"));
    assert!(is_permanent_delivery_error("554 rejected"));
    assert!(is_permanent_delivery_error("Command contains invalid characters"));

    assert!(!is_permanent_delivery_error("TLS connection failed: invalid peer certificate: UnknownIssuer"));
    assert!(!is_permanent_delivery_error("451 Try again later"));
    assert!(!is_permanent_delivery_error("Connection closed unexpectedly"));
    assert!(!is_permanent_delivery_error("Could not resolve IPv4 for MX ..."));
}
```

### 5.2 Integration test — **failing retry does not loop** (THE key regression, currently missing)

New test in `tests/queue_tests.rs`. Drive one `process_queue_tick` against a transient-failure mock and assert the job is *updated in place*, not deleted-and-recreated.

```rust
#[tokio::test]
async fn failing_retry_increments_attempts_and_does_not_duplicate() {
    common::run_on_all_dbs(|db| async move {
        let temp = tempfile::tempdir().unwrap();

        // Transient-failure relay: accept the TCP connection, then drop it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { if let Ok((s, _)) = listener.accept().await { drop(s); } });

        let resolver = hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap();
        let outbound = std::sync::Arc::new(
            maileroo::outbound::OutboundService::new(
                "srs".into(), resolver, "example.com".into(), db.clone(), temp.path().to_path_buf(),
            ).with_relay_override(maileroo::outbound::relay::RelayConfig {
                host: "127.0.0.1".into(), port, user: "api".into(), pass: "tok".into(),
            }),
        );

        // Seed a job (attempts = 0) eligible now.
        let id = uuid::Uuid::new_v4();
        let eml = maileroo::outbound::get_job_file_path(temp.path(), id);
        tokio::fs::create_dir_all(eml.parent().unwrap()).await.unwrap();
        tokio::fs::write(&eml, b"Subject: x\r\n\r\nbody").await.unwrap();
        maileroo::db::queue::insert_job(&db, id, "s@example.com", "r@ext.com").await.unwrap();

        maileroo::outbound::process_queue_tick(&db, temp.path(), outbound).await.unwrap();

        // Same row, attempts incremented, still pending, backoff in the future, file kept.
        let count = count_queue_rows(&db).await;                 // small SELECT COUNT(*) helper
        assert_eq!(count, 1, "must not create a duplicate job on failed retry");
        let jobs = maileroo::db::queue::fetch_all_jobs(&db).await; // or query directly; include non-eligible
        let job = jobs.iter().find(|j| j.id == id).expect("original job id must still exist");
        assert_eq!(job.attempts, 1, "attempts must increment");
        assert_eq!(job.status, "pending");
        assert!(job.next_retry_at > time::OffsetDateTime::now_utc(), "backoff must push next_retry into the future");
        assert!(eml.exists(), "EML must be retained for the next attempt");
    }).await;
}
```

> Implementation note: `fetch_next_retryable_jobs` filters on `next_retry_at <= now`, so after backoff the job is *not* returned by it. Assert existence with a direct `SELECT` (or add a test-only `fetch_all_jobs`) plus a `SELECT COUNT(*)` to prove no duplicate was created. The count-stays-1 assertion is the specific guard against the original "new UUID every tick" bug.

### 5.3 Integration test — permanent failure fails fast

Mock rejects `MAIL FROM` with `550`. After one tick: `status = "failed"`, `attempts = 1`, no retry scheduled, and count stays 1.

### 5.4 Integration test — exhausts `max_attempts` then stops

Seed a job with `attempts = 9` (default `max_attempts = 10`), tick against the transient-failure mock, assert `status = "failed"` and `attempts = 10`. A second tick must not resurrect it (still `failed`, count unchanged).

### 5.5 Integration test — TLS: delivery succeeds against a self-signed / incomplete-chain server

Prove Defect 2 is fixed. Stand up a mock SMTP server that advertises `STARTTLS` and completes the TLS handshake with a **self-signed** cert (reuse `common::generate_dummy_certs` / `rcgen`). Send through the **MX path** (not the relay override). Assert delivery succeeds (job cleaned up). Before the fix this fails with `UnknownIssuer`; after the fix it succeeds. Also add a negative test asserting the **relay** path still rejects an untrusted cert (strict verification preserved there).

### 5.6 E2E (Playwright, `e2e/`) — opportunistic TLS end-to-end

MailHog-based, matching `e2e/tests/outbound-attachments.spec.ts` and the helpers in `e2e/helpers/`. Configure the outbound delivery target to a STARTTLS endpoint using a self-signed certificate, compose+send via the UI, and assert the message is captured (`waitForMessage`). This locks in that real STARTTLS delivery to a non-strict server works through the full stack. Run via `./scripts/e2e-test.sh` (or `just e2e-test`); MailHog must be up (`just db-up`).

---

## 6. Verification checklist (before merge)

- [ ] `cargo test` green, including the new §5.1–§5.5 tests, on SQLite.
- [ ] `TEST_DATABASE_URL=<postgres> cargo test` green (dual-DB parity per `AGENTS.md`).
- [ ] `cargo fmt --check` and `cargo clippy` clean.
- [ ] Manual/e2e: trigger a transient failure and confirm in logs that a single job's `attempts` climbs `1 → 2 → 3…` with **widening** gaps (backoff), the UUID is **stable**, "successfully delivered" appears **only** on real delivery, and the job stops at `max_attempts` with `status='failed'`.
- [ ] Manual: deliver to a strict/incomplete-chain MX (Outlook is a good real target) and confirm success instead of `UnknownIssuer`.
- [ ] Confirm the authenticated relay path still enforces certificate verification.

## 7. Files touched (expected)

- `src/outbound/mod.rs` — `deliver_once`, `is_permanent_delivery_error`, `OpportunisticVerifier`, `mx_tls_config`, MX connector uses `mx_tls_config`.
- `src/outbound/queue.rs` — `process_queue_tick` calls `deliver_once` and owns retry accounting.
- `src/outbound/relay.rs` — unchanged (keeps strict `client_config`); verify only.
- `tests/queue_tests.rs` — add §5.2–§5.5.
- unit tests co-located with the classifier (§5.1).
- `e2e/tests/` — add §5.6.

## 8. Out of scope (follow-ups worth filing)

- DANE (TLSA) / MTA-STS enforcement for opportunistic-but-authenticated TLS.
- Surfacing permanently-failed queue jobs in the dashboard (currently they linger with `status='failed'` and a far-future `next_retry_at`); consider a bounce/notification path.
- Plaintext fallback when STARTTLS negotiation itself fails (§4.2 optional).
