#!/bin/bash
set -e

echo "==> Setting up Grengin API development environment..."

# Wait for database to be ready
echo "==> Waiting for PostgreSQL to be ready..."
until pg_isready -h db -U grengin -d grengin; do
  echo "PostgreSQL is not ready yet. Waiting..."
  sleep 2
done
echo "==> PostgreSQL is ready!"

# Build the project to download dependencies
echo "==> Building project and downloading dependencies..."
cargo build

# Run database migrations
echo "==> Running database migrations..."
cargo run --package migration -- up

echo ""
echo "==> Development environment setup complete!"
echo ""
echo "Useful commands:"
echo "  cargo run                    - Start the API server"
echo "  cargo watch -x run           - Start with hot reload"
echo "  cargo test                   - Run tests"
echo "  cargo clippy                 - Run linter"
echo "  sea-orm-cli migrate up       - Run migrations"
echo "  sea-orm-cli migrate down     - Rollback migrations"
echo ""
echo "Services:"
echo "  API Server: http://localhost:8080"
echo "  Swagger UI: http://localhost:8080/swagger-ui"
echo "  PostgreSQL: db:5432 (user: grengin, password: grengin)"
echo ""
