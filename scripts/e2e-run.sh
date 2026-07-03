#!/usr/bin/env bash
set -euo pipefail

# Run the app wired to MailHog for local E2E tests.
# Uses a dedicated SQLite DB + storage dir so it never touches dev data.

export STORAGE_DIR=./storage/e2e
export DATABASE_URL="sqlite://./storage/e2e/maileroo.db?mode=rwc"
export SMTP_ADDR=127.0.0.1:2526
export WEB_ADDR=127.0.0.1:3001
export SRS_SECRET=e2e-srs-secret
export PROD_DOMAIN=localhost
export SECURE_COOKIES=false
export COOKIE_DOMAIN=""
export SMTP_RELAY_TOKEN=mailhog
export SMTP_RELAY_HOST=127.0.0.1
export SMTP_RELAY_PORT=1025
export SMTP_RELAY_USER=""
export ADMIN_EMAIL=admin@e2e.test
export ADMIN_PASSWORD=e2e-admin-password

mkdir -p "$STORAGE_DIR"

echo "Setting up E2E database..."
cargo run --bin setup_db

echo "Starting Maileroo in E2E mode..."
exec cargo run
