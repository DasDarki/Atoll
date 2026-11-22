# Atoll Sync on Coolify

The sync server keeps the end-to-end encrypted workspaces, islands, tabs and archive of every
Atoll account. It never sees their content. Accounts are the users of Onyx.

## 1. Onyx

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

## 2. Coolify

1. New resource, Docker Compose, this repository, branch `main`.
2. Base directory `/deploy/sync-server`, Docker Compose location `/docker-compose.yml`.
   Coolify resolves the build context against the base directory, and `../..` from there is the
   repository root the server is built from. With base directory `/` the build fails with
   `lstat /deploy: no such file or directory`.
3. Domain of the service `atoll-sync`: `https://atoll.dasdarki.de:8080`. The `:8080` tells
   Coolify which container port Traefik routes to. The service only exposes the port, nothing is
   published on the host. Postgres gets no domain.
4. Environment variables (Coolify lists the required ones on its own):

| Variable | Value |
|---|---|
| `ATOLL_SYNC_ONYX_URL` | The public URL of Onyx, exactly as its tokens name the issuer (its `ONYX_PUBLIC_URL`, without a trailing slash) |
| `ATOLL_SYNC_CLIENT_ID` | The client ID of the Atoll app in Onyx |
| `POSTGRES_PASSWORD` | A strong password. Any character is fine, it is not put into a URL |
| `ATOLL_SYNC_TOMBSTONE_DAYS` | Optional, default `180`: how long removals are kept for devices that were offline |

5. Deploy. The first build compiles the server and takes a few minutes. The container health
   check runs `atoll-sync-server health`, which asks `GET /healthz`; that answers `ok` when the
   database is reachable.

Check from outside: `https://atoll.dasdarki.de/v1/config` answers with the Onyx URL and the client
ID. Everything else wants a token.

The event stream (`GET /v1/events`) is a long-lived server-sent events response; Traefik passes it
through as it is, no extra configuration is needed.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `ATOLL_SYNC_DATABASE_URL` | set by the compose file | Postgres address, without the password |
| `ATOLL_SYNC_DATABASE_PASSWORD` | set by the compose file | Postgres password |
| `ATOLL_SYNC_LISTEN` | `0.0.0.0:8080` | Listen address |
| `ATOLL_SYNC_ONYX_URL` | required | Onyx issuer |
| `ATOLL_SYNC_JWKS_URL` | `<issuer>/oauth/jwks` | Where the signing keys of Onyx are |
| `ATOLL_SYNC_CLIENT_ID` | required | Audience the tokens must carry |
| `ATOLL_SYNC_TOMBSTONE_DAYS` | `180` | Age after which removals are forgotten |
| `RUST_LOG` | `atoll_sync_server=info` | Log level |

The server refuses to start when a required value is missing or empty.

## Tests

`scripts/sync-server-tests.sh` runs the tests of the server and of the sync engine against a
throwaway Postgres in Docker.
