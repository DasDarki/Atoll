CREATE TABLE account (
    account       TEXT        PRIMARY KEY,
    revision      BIGINT      NOT NULL DEFAULT 0,
    pruned_before BIGINT      NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE record (
    account    TEXT        NOT NULL REFERENCES account(account) ON DELETE CASCADE,
    kind       TEXT        NOT NULL,
    id         TEXT        NOT NULL,
    revision   BIGINT      NOT NULL,
    data       BYTEA,
    device     TEXT        NOT NULL,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account, kind, id)
);

CREATE INDEX record_by_revision ON record (account, revision);
CREATE INDEX record_tombstones ON record (changed_at) WHERE data IS NULL;

CREATE TABLE account_key (
    account    TEXT        PRIMARY KEY REFERENCES account(account) ON DELETE CASCADE,
    salt       BYTEA       NOT NULL,
    memory_kib INTEGER     NOT NULL,
    passes     INTEGER     NOT NULL,
    wrapped    BYTEA       NOT NULL,
    version    BIGINT      NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE device (
    account      TEXT        NOT NULL REFERENCES account(account) ON DELETE CASCADE,
    id           TEXT        NOT NULL,
    name         TEXT        NOT NULL DEFAULT '',
    platform     TEXT,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account, id)
);
