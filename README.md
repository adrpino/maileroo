# Maileroo

Maileroo is a programmable self-hosted email forwarding gateway, private alias manager, and SMTP server built in Rust. It allows you to run your own private email forwarding service (similar to SimpleLogin or Firefox Relay) using your own domain names.

## How It Works

Maileroo acts as a secure intermediary between the public internet and your personal inbox (such as Gmail or ProtonMail).

### 1. Inbound Forwarding with SRS
When an email is sent to an alias on your registered domain (e.g., shopping@yourdomain.com):
1. Maileroo accepts the connection on Port 25, parses the email, and verifies that the alias exists in the database.
2. It rewrites the sender envelope address using the Sender Rewriting Scheme (SRS) (e.g., rewriting sender@gmail.com to SRS0+hash=timestamp=gmail.com=sender@yourdomain.com). This ensures that forwarding the email does not break SPF and DMARC checks.
3. It forwards the email to your configured real personal inbox.

### 2. Two-Way Anonymous Replies
If you click reply from your personal inbox:
1. Your mail client sends the reply back to the rewritten SRS address at Maileroo.
2. Maileroo validates the cryptographic HMAC signature of the SRS address to prevent spam or spoofing.
3. It decodes the original sender address, removes your personal address from the headers, and sends the reply to the original sender. Your real personal inbox remains completely hidden.

### 3. REST API and Admin Console
Maileroo includes an integrated web server that serves a lightweight administrative dashboard:
* Create and manage email domains and secure aliases.
* Manage API keys for programmatic integration.
* Rotate DKIM selectors and keys dynamically with zero downtime.
* Monitor live email delivery queues and retry histories.

## Core Features

* **Inbound SMTP Daemon**: Handles high-concurrency connections on Port 25 with integrated tarpitting, slowloris read timeouts, and disk-buffered OOM protection.
* **Unified Database Layer**: Supports both zero-dependency local SQLite databases and multi-node PostgreSQL clusters out of the box, with automated schema migrations on startup.
* **Native Auto-TLS**: Performs Let's Encrypt HTTP-01 and TLS-ALPN-01 ACME challenge negotiation natively on Port 80 and 443 to secure both web traffic and SMTP STARTTLS dynamically.
* **Outbound Relay Fallback**: Safely routes outgoing mail through trusted upstream relays (such as Amazon SES, Postmark, or SendGrid) to bypass Port 25 blocks on cloud VPS providers.
* **DKIM Signing**: Dynamically signs all outgoing mail (firsthand sends and forwarded replies) using secure, modern Ed25519 (Elliptic Curve) signatures.

## Technology Stack

* **Web Framework**: Axum and Tower-Sessions.
* **Frontend rendering**: HTMX and Askama Templates.
* **Async Runtime**: Tokio.
* **Crypto and Network**: Tokio-Rustls, Rustls-ACME, and Hickory-Resolver.

## Development Setup

### 1. Setup Local Environment
Copy the sample environment configuration file:
```bash
cp .env.example .env
```

### 2. Run Local Dependencies
Start the local development database and a Step-CA instance for offline TLS certificate validation:
```bash
docker compose -f docker-compose.dev.yml up -d
```

### 3. Generate Development Certificates
Generate self-signed STARTTLS certificates for local development:
```bash
mkdir -p certs
openssl req -x509 -newkey rsa:4096 -keyout certs/smtp_key.pem -out certs/smtp_cert.pem -sha256 -days 365 -nodes -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost"
```

### 4. Run Schema Migrations
Initialize the database and tables:
```bash
cargo run --bin setup_db
```

### 5. Running the Application
```bash
# Start the Maileroo Monolith
cargo run

# Run the local SMTP testing client in a separate terminal
cargo run --bin smtp-test
```

## Project Structure

* `src/inbound/`: Inbound SMTP server, connection rate-limiting, blocklisting, and protocol session state.
* `src/outbound/`: Outbound mail sending, SRS encoding, DKIM signing, and delivery retry queue daemon.
* `src/web/`: REST API endpoints, session authentication, HTMX dashboard views, and the native Auto-TLS certificate server.
* `src/db/`: Unified database pool dispatcher supporting dynamic SQLite/PostgreSQL execution.
