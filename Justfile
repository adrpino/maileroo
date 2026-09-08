default: help

# Show available commands
help:
	@just --list

# Initialize the local environment (.env file)
init-env:
	@if [ ! -f .env ]; then cp .env.example .env; echo "Created .env from .env.example"; fi

# Start the local database (Postgres)
db-up:
	docker-compose -f docker-compose.dev.yml up -d
	@echo "Waiting for Postgres to be ready..."
	@sleep 3

# Stop the local database
db-down:
	docker-compose -f docker-compose.dev.yml down

# Run the setup script to migrate and seed the database
seed: init-env db-up
	cargo run --bin setup_db

# Run the development server
dev: seed
	@echo "======================================================"
	@echo "⚠️ IMPORTANT: Access the server at http://localhost:3000"
	@echo "Do NOT use http://0.0.0.0:3000 or login cookies will fail."
	@echo "======================================================"
	cargo run

# Run tests
test:
	cargo test

# Clean the environment (stops DB and removes .env)
clean: db-down
	rm -f .env

# Run the app wired to MailHog for local E2E tests.
# Uses a dedicated SQLite DB + storage dir so it never touches dev data.
e2e-run:
	./scripts/e2e-run.sh

# Clean E2E storage and temporary files
e2e-clean:
	rm -rf ./storage/e2e ./e2e/.auth ./e2e/playwright-report

# Run the complete E2E test suite automatically (handles dependencies, server lifecycle, and tests)
e2e-test:
	./scripts/e2e-test.sh