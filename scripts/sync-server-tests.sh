#!/usr/bin/env bash
# Runs the tests of the sync server and the sync engine against a throwaway Postgres in Docker.
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

# The image starts a server for its setup first and restarts it, so it is ready only once it
# says so the second time.
for _ in $(seq 1 120); do
  ready=$(docker logs "$NAME" 2>&1 | grep -c "database system is ready to accept connections" || true)
  [ "$ready" -ge 2 ] && break
  sleep 0.5
done

ATOLL_SYNC_TEST_DATABASE_URL="postgres://atoll:atoll@127.0.0.1:$PORT/atoll_sync" ATOLL_SYNC_NO_KEYRING=1 \
  cargo test -p atoll-sync-server -p atoll-sync-client "$@"
