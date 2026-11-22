# Atoll Sync on Coolify

The sync server keeps the end-to-end encrypted workspaces, islands, tabs and archive of every
Atoll account. It never sees their content. Accounts are the users of Onyx.

## Onyx

In Onyx open Admin, Apps and create an app:

| Field | Value |
|---|---|
| Name | Atoll |
| Homepage | `https://atoll.dasdarki.de` |
| Redirect URIs | `atoll://auth/callback` (Android; the desktop uses `http://127.0.0.1:*`, which Onyx always accepts) |
| Scopes | `openid`, `username`, `avatar` |
| Public client | yes (no secret, PKCE) |
| Skip consent | as you like |

Note the client ID.

## Coolify

1. New resource, Docker Compose, this repository, compose file `deploy/sync-server/docker-compose.yml`.
2. Domain of the service `atoll-sync`: `https://atoll.dasdarki.de:8080`. The `:8080` tells Coolify
   which container port Traefik routes to. The service only exposes the port, nothing is
   published on the host.
3. Environment variables:

| Variable | Value |
|---|---|
| `ATOLL_SYNC_ONYX_URL` | The public URL of Onyx, exactly as its tokens name the issuer, for example `https://auth.dasdarki.de` |
| `ATOLL_SYNC_CLIENT_ID` | The client ID of the Atoll app in Onyx |
| `POSTGRES_PASSWORD` | A strong password |
| `ATOLL_SYNC_TOMBSTONE_DAYS` | Optional, default `180`: how long removals are kept for devices that were offline |

4. Deploy. The health check runs `atoll-sync-server health`; `GET /healthz` answers `ok` when
   the database is reachable.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `ATOLL_SYNC_DATABASE_URL` | set by the compose file | Postgres connection string |
| `ATOLL_SYNC_LISTEN` | `0.0.0.0:8080` | Listen address |
| `ATOLL_SYNC_ONYX_URL` | required | Onyx issuer |
| `ATOLL_SYNC_JWKS_URL` | `<issuer>/oauth/jwks` | Where the signing keys of Onyx are |
| `ATOLL_SYNC_CLIENT_ID` | required | Audience the tokens must carry |
| `ATOLL_SYNC_TOMBSTONE_DAYS` | `180` | Age after which removals are forgotten |
| `RUST_LOG` | `atoll_sync_server=info` | Log level |

## Tests

`scripts/sync-server-tests.sh` runs the server tests against a throwaway Postgres in Docker.
