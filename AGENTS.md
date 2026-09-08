# Agent Instructions & Architectural Conventions

This document specifies foundational conventions and guidelines that all LLM agents (e.g., Gemini CLI) must adhere to when contributing code to this workspace.

---

## 1. Import Style and Code Cleanliness

- **No Absolute Path Calls Mid-Code**: Never invoke absolute or mid-file paths (like `crate::db::sent_emails::get_sent_email_by_id_and_user(...)` or `crate::mod::submodule::function(...)`) inside method or function bodies.
- **Top-Level `use` Imports**: Declare all your imports strictly at the top of files using grouped or targeted `use` declarations. Keep the code clean, concise, and standard.

## 2. Test Structure and the Multi-Database Approach

- **Integration Tests Dual-Database Execution**: All integration and touchpoint database tests must run against **both** SQLite (in-memory) and PostgreSQL engines.
- **`run_on_all_dbs` Pattern**: Structure and wrap all integration test bodies using the standard `common::run_on_all_dbs(|db| async move { ... })` closure block. This ensures reference parity and catches database-specific queries or typing issues early.

## 3. End-to-End (E2E) Testing Guidelines

- **Playwright and SQLite:** The E2E test suite (`./e2e`) runs against a local instance of the application using Playwright. By default, E2E tests are configured to use an isolated SQLite database (`./storage/e2e`) to ensure they never mutate or collide with developer Postgres data. 
- **Execution Script:** Always use the dedicated test runner (`./scripts/e2e-test.sh` or `just e2e-test`) which handles installing Node dependencies, standing up the background server, and running the Playwright tests, before cleaning up after itself.
- **MailHog Dependency:** The E2E tests rely on MailHog for testing outbound and inbound email pipelines. Before running the E2E suite locally, ensure MailHog is running via the dev compose stack (`docker-compose -f docker-compose.dev.yml up -d mailhog` or `just db-up`).

## 4. Pure Functions and Unit Testing

- **Prioritize Pure Functions**: Structure code logic to isolate computations, formatting, or encoding into standalone pure functions (functions with zero side effects).
- **Comprehensive Unit Testing**: Cover these pure functions with fast, direct unit tests in the same module. This makes the codebase significantly easier to debug and test in isolation.

## 5. Module Sizing and Encapsulation

- **Keep Modules Self-Contained**: Do not let files/modules grow too large. If a module's responsibilities expand, decompose it into smaller, logically isolated files.
- **Strict Encapsulation**: Keep internal implementation details hidden and interface structures public and well-defined.

## 6. Comments, Commits, and Documentation

- **English Only**: All inline comments, code documentation, and git commit messages must be written strictly in professional English.
- **No Emojis/No Clutter**: Commit messages must be written in a professional manner, excluding emojis or conversational filler.

## 7. Frontend Internationalization (i18n)

- **Mandatory Translation**: All user-facing frontend UI strings (labels, buttons, modal titles, tooltips, error messages, notifications) must be integrated into the existing internationalization system in `src/web/i18n.rs`.
- **Four Locales Supported**: Every message key added to the `Messages` trait must be translated across all four supported locales: English (`Locale::En`), Spanish (`Locale::Es`), French (`Locale::Fr`), and Portuguese (`Locale::Pt`).
- **No Hardcoded English in Templates**: Never hardcode user-facing text strings directly in Askama templates (`templates/*.html`). Always reference them via `{{ locale.your_message_key() }}`.

## 8. Pre-Commit Verification Workflow

Before committing any changeset or submitting code, you must run the following checks in order, matching the project CI pipeline (`.github/workflows/ci.yml`):

1. **Formatting Check**:
   ```bash
   cargo fmt --all -- --check
   ```
   If formatting fails, run `cargo fmt --all` to automatically resolve formatting issues.

2. **Clippy Linter**:
   ```bash
   cargo clippy --all-targets
   ```
   Ensure no regressions or compiler warnings are introduced.

3. **Multi-Database Test Suite**:
   Start the local dev database (`just db-up`) and run the test suite with `TEST_DATABASE_URL` configured so tests execute against both SQLite and PostgreSQL:
   ```bash
   TEST_DATABASE_URL="postgres://postgres:mysecret@localhost:54322/maily" cargo test
   ```

4. **End-to-End Test Suite**:
   When routes, templates, or email lifecycles are modified, run the automated E2E test runner:
   ```bash
   just e2e-test
   ```


