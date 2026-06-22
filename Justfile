default: help

# Show available commands
help:
	@just --list

# Initialize and fix the local environment (.env file)
init-env:
	@if [ ! -f .env ]; then cp .env.example .env; echo "Created .env from .env.example"; fi
	@sed -i 's|^DATABASE_URL=.*|DATABASE_URL=postgres://postgres:mysecret@localhost:54322/maily|' .env
	@sed -i 's|^SECURE_COOKIES=.*|SECURE_COOKIES=false|' .env
	@sed -i 's|^COOKIE_DOMAIN=.*|COOKIE_DOMAIN=|' .env
	@echo "✅ Local .env configured for HTTP/localhost (no secure cookies, no domain restriction)."

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