#!/usr/bin/env bash
set -euo pipefail

# This script runs the entire E2E test suite automatically.
# It starts the E2E server in the background, runs Playwright tests,
# and guarantees the server is stopped on exit.

# Ensure we are in the workspace root
cd "$(dirname "$0")/.."

# 1. Prepare E2E state
echo "Cleaning old E2E state..."
rm -rf ./storage/e2e ./e2e/.auth ./e2e/playwright-report

# 2. Check and install E2E Node.js dependencies
echo "Installing E2E dependencies..."
cd e2e
npm install
npx playwright install chromium

# 3. Start E2E server in the background
echo "Starting E2E server in the background..."
cd ..
./scripts/e2e-run.sh > e2e-server.log 2>&1 &
SERVER_PID=$!

# Ensure the server is killed when the script exits (success or failure)
cleanup() {
    echo "Stopping E2E server (PID $SERVER_PID)..."
    kill "$SERVER_PID" || true
    wait "$SERVER_PID" 2>/dev/null || true
    rm -f e2e-server.log
}
trap cleanup EXIT

# 4. Wait for the server to be ready on port 3001
echo "Waiting for E2E server to be ready on port 3001..."
MAX_ATTEMPTS=30
for i in $(seq 1 $MAX_ATTEMPTS); do
    if curl -sf http://127.0.0.1:3001/health >/dev/null 2>&1; then
        echo "Server is ready!"
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "Error: Server failed to start. Logs:"
        cat e2e-server.log
        exit 1
    fi
    sleep 1
done

if [ "$i" -eq "$MAX_ATTEMPTS" ]; then
    echo "Error: Server timed out waiting to start."
    exit 1
fi

# 5. Run Playwright E2E tests
echo "Running Playwright E2E tests..."
export E2E_BASE_URL=http://localhost:3001
export MAILHOG_API=http://localhost:8025
export E2E_ADMIN_EMAIL=admin@e2e.test
export E2E_ADMIN_PASSWORD=e2e-admin-password

cd e2e
npm run test
