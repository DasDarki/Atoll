#!/usr/bin/env bash
# Runs the tests of the sync server against a throwaway Postgres in Docker.
set -euo pipefail

NAME="atoll-sync-test-postgres"
PORT="${ATOLL_SYNC_TEST_PORT:-5545}"

cleanup() {
  docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

cleanup
docker run -d --name "$NAME" \
  -e POSTGRES_USER=atoll -e POSTGRES_PASSWORD=atoll -e POSTGRES_DB=atoll_sync \
  -p "127.0.0.1:$PORT:5432" postgres:17 >/dev/null

for _ in $(seq 1 60); do
  if docker exec "$NAME" pg_isready -U atoll -d atoll_sync >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
docker exec "$NAME" pg_isready -U atoll -d atoll_sync >/dev/null

ATOLL_SYNC_TEST_DATABASE_URL="postgres://atoll:atoll@127.0.0.1:$PORT/atoll_sync" \
  cargo test -p atoll-sync-server "$@"
