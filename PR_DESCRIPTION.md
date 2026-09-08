# Feature: High-Efficiency Multi-Label Email Filtering and Backfill Engine

## Summary
Implements a fast, multi-tenant email filtering and labeling system that categorizes incoming emails and safely backfills existing emails.

## Key Changes
- Multi-Pattern Matching Engine (src/filter.rs):
  - Uses Aho-Corasick automaton cached in memory per user in an ArcSwap/DashMap store to evaluate body keywords in a single O(N) pass.
  - Automatically ignores large binary attachments and caps body inspection to the first 64 KB.
- Safe Background Backfill Engine (src/filter.rs):
  - Multi-tenant serialized queue with an in-memory tenant guard (DashSet) ensuring at most one active backfill task per user.
  - Processes emails in chunks of 50 with cooperative yields (25ms pauses) to ensure zero IOPS starvation for live inbound SMTP writes.
  - Idempotent database tagging via ON CONFLICT DO NOTHING.
- Database Schema and Migrations:
  - Dual migrations for PostgreSQL and SQLite (migrations/postgres and migrations/sqlite) introducing labels, email_filters, and email_labels tables with ON DELETE CASCADE constraints.
  - Complete referential integrity guaranteed on manual email deletions and automated retention cleanups.
- User Interface and Anti-Overflow Protections:
  - Dashboard label filter strip and anti-overflow tag styling in email rows with flex wrapping and +N count indicators to prevent layout breaking.
  - Simple modal for creating rules with an unchecked default checkbox for Apply to existing emails accompanied by an informational tooltip.
- Full Internationalization (src/web/i18n.rs):
  - Added full translation keys across all four supported locales: English (En), Spanish (Es), French (Fr), and Portuguese (Pt).
- Configuration and Workflow:
  - Simplified environment initialization in Justfile and updated .env.example with localhost defaults.
  - Added pre-commit verification workflow instructions to AGENTS.md.
  - Bumped crate version to 0.2.5 in Cargo.toml.

## Testing and Verification
- Unit tests covering Aho-Corasick multi-matching, case insensitivity, 64 KB scan limits, HTML tag stripping, tenant guard locks, and anti-overflow label rendering.
- Multi-database integration tests covering label and filter CRUD, batch lookups, cascading deletions, auto-cleanup cascading, dashboard label filtering, live SMTP end-to-end multi-label tagging, and background backfilling.
- All 125 tests passing cleanly across both SQLite and PostgreSQL.
