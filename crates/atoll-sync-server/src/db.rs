//! Everything the server keeps, in Postgres. Records are stored sealed, as they arrive: the
//! server never sees what they hold.
//!
//! Every account counts its own revisions. Writing takes a lock on the account row, so the
//! changes of one account get their revisions in the order they become visible. Without that a
//! device reading changes could see revision 12 committed before revision 11 and never learn
//! about 11.

use std::time::Duration;

use atoll_sync::wire::{Change, DeviceName, Platform, Stored, StoredKey, WireEnvelope};
use atoll_sync::{Kind, SyncId};
use sqlx::{PgPool, Row};

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

/// Makes sure the account and the asking device are known, and notes when the device was seen.
pub async fn touch(pool: &PgPool, account: &str, device: SyncId) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO account (account) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(account)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO device (account, id) VALUES ($1, $2)
         ON CONFLICT (account, id) DO UPDATE SET last_seen_at = now()",
    )
    .bind(account)
    .bind(device.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// Stores `changes` of `device` and answers with the revision the last one got.
/// Stores `changes`, `None` when they are sealed with `key_id` and the account keeps another
/// key or none. The key is checked under the lock a reset takes as well.
pub async fn push(
    pool: &PgPool,
    account: &str,
    device: SyncId,
    key_id: Option<&str>,
    changes: &[Change],
) -> Result<Option<i64>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let mut revision: i64 =
        sqlx::query_scalar("SELECT revision FROM account WHERE account = $1 FOR UPDATE")
            .bind(account)
            .fetch_one(&mut *transaction)
            .await?;
    if let Some(key_id) = key_id {
        let kept: Option<String> =
            sqlx::query_scalar("SELECT key_id FROM account_key WHERE account = $1")
                .bind(account)
                .fetch_optional(&mut *transaction)
                .await?;
        if !kept.is_some_and(|kept| kept.is_empty() || kept == key_id) {
            return Ok(None);
        }
    }
    for change in changes {
        revision += 1;
        sqlx::query(
            "INSERT INTO record (account, kind, id, revision, data, device, changed_at)
             VALUES ($1, $2, $3, $4, $5, $6, now())
             ON CONFLICT (account, kind, id) DO UPDATE
             SET revision = EXCLUDED.revision, data = EXCLUDED.data,
                 device = EXCLUDED.device, changed_at = EXCLUDED.changed_at",
        )
        .bind(account)
        .bind(change.kind.as_str())
        .bind(change.id.to_string())
        .bind(revision)
        .bind(change.data.as_deref())
        .bind(device.to_string())
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("UPDATE account SET revision = $2 WHERE account = $1")
        .bind(account)
        .bind(revision)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Some(revision))
}

/// Where reading changes has to start again from the snapshot: changes up to this revision
/// may have been removed.
pub async fn pruned_before(pool: &PgPool, account: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT pruned_before FROM account WHERE account = $1")
        .bind(account)
        .fetch_one(pool)
        .await
}

/// Up to `limit` changes after `since`, oldest first.
pub async fn changes(
    pool: &PgPool,
    account: &str,
    since: i64,
    limit: i64,
) -> Result<Vec<Stored>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT revision, kind, id, data, device FROM record
         WHERE account = $1 AND revision > $2 ORDER BY revision LIMIT $3",
    )
    .bind(account)
    .bind(since)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter().map(stored).collect()
}

/// Every record that exists and the revision they stand at, read at one moment.
pub async fn snapshot(pool: &PgPool, account: &str) -> Result<(Vec<Stored>, i64), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *transaction)
        .await?;
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM account WHERE account = $1")
        .bind(account)
        .fetch_one(&mut *transaction)
        .await?;
    let rows = sqlx::query(
        "SELECT revision, kind, id, data, device FROM record
         WHERE account = $1 AND data IS NOT NULL ORDER BY revision",
    )
    .bind(account)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let records = rows.iter().map(stored).collect::<Result<_, _>>()?;
    Ok((records, revision))
}

pub async fn current_revision(pool: &PgPool, account: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT revision FROM account WHERE account = $1")
        .bind(account)
        .fetch_one(pool)
        .await
}

fn stored(row: &sqlx::postgres::PgRow) -> Result<Stored, sqlx::Error> {
    let kind: String = row.try_get("kind")?;
    let id: String = row.try_get("id")?;
    let device: String = row.try_get("device")?;
    let decode = |what: &str| sqlx::Error::Decode(format!("stored {what} is not valid").into());
    Ok(Stored {
        revision: row.try_get("revision")?,
        kind: Kind::parse(&kind).ok_or_else(|| decode("kind"))?,
        id: id.parse().map_err(|_| decode("id"))?,
        data: row.try_get("data")?,
        device: device.parse().map_err(|_| decode("device"))?,
    })
}

pub async fn key(pool: &PgPool, account: &str) -> Result<Option<StoredKey>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT key_id, salt, memory_kib, passes, wrapped, version FROM account_key WHERE account = $1",
    )
    .bind(account)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(StoredKey {
            envelope: WireEnvelope {
                key_id: row.try_get("key_id")?,
                salt: row.try_get("salt")?,
                memory_kib: row.try_get::<i32, _>("memory_kib")? as u32,
                passes: row.try_get::<i32, _>("passes")? as u32,
                wrapped: row.try_get("wrapped")?,
            },
            version: row.try_get("version")?,
        })
    })
    .transpose()
}

/// Stores the wrapped key. Without `replaces` only when there is none, otherwise only over the
/// version given. Answers with the new version, or `None` when someone else was faster.
pub async fn put_key(
    pool: &PgPool,
    account: &str,
    envelope: &WireEnvelope,
    replaces: Option<i64>,
) -> Result<Option<i64>, sqlx::Error> {
    let version = match replaces {
        None => sqlx::query_scalar(
            "INSERT INTO account_key (account, salt, memory_kib, passes, wrapped, version, key_id)
                 VALUES ($1, $2, $3, $4, $5, 1, $6)
                 ON CONFLICT (account) DO NOTHING RETURNING version",
        )
        .bind(account)
        .bind(&envelope.salt)
        .bind(envelope.memory_kib as i32)
        .bind(envelope.passes as i32)
        .bind(&envelope.wrapped)
        .bind(&envelope.key_id)
        .fetch_optional(pool)
        .await?,
        Some(previous) => {
            sqlx::query_scalar(
                "UPDATE account_key
                 SET salt = $2, memory_kib = $3, passes = $4, wrapped = $5,
                     version = version + 1, updated_at = now(), key_id = $7
                 WHERE account = $1 AND version = $6 RETURNING version",
            )
            .bind(account)
            .bind(&envelope.salt)
            .bind(envelope.memory_kib as i32)
            .bind(envelope.passes as i32)
            .bind(&envelope.wrapped)
            .bind(previous)
            .bind(&envelope.key_id)
            .fetch_optional(pool)
            .await?
        }
    };
    Ok(version)
}

/// Removes every record and the key of an account. All cursors handed out so far end up before
/// the new pruning mark, so every device reads the empty snapshot next.
pub async fn reset(pool: &PgPool, account: &str) -> Result<i64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let revision: i64 = sqlx::query_scalar(
        "UPDATE account SET revision = revision + 1, pruned_before = revision + 1
         WHERE account = $1 RETURNING revision",
    )
    .bind(account)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM record WHERE account = $1")
        .bind(account)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM account_key WHERE account = $1")
        .bind(account)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(revision)
}

pub async fn name_device(
    pool: &PgPool,
    account: &str,
    device: SyncId,
    name: &DeviceName,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE device SET name = $3, platform = $4 WHERE account = $1 AND id = $2")
        .bind(account)
        .bind(device.to_string())
        .bind(&name.name)
        .bind(platform_name(name.platform))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn devices(
    pool: &PgPool,
    account: &str,
) -> Result<Vec<(SyncId, String, Option<Platform>, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, platform, (extract(epoch FROM last_seen_at) * 1000)::BIGINT AS seen
         FROM device WHERE account = $1 ORDER BY last_seen_at DESC",
    )
    .bind(account)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            let platform: Option<String> = row.try_get("platform")?;
            Ok((
                id.parse()
                    .map_err(|_| sqlx::Error::Decode("stored device is not valid".into()))?,
                row.try_get("name")?,
                platform.as_deref().and_then(parse_platform),
                row.try_get("seen")?,
            ))
        })
        .collect()
}

pub async fn forget_device(
    pool: &PgPool,
    account: &str,
    device: SyncId,
) -> Result<bool, sqlx::Error> {
    let removed = sqlx::query("DELETE FROM device WHERE account = $1 AND id = $2")
        .bind(account)
        .bind(device.to_string())
        .execute(pool)
        .await?;
    Ok(removed.rows_affected() > 0)
}

/// Removes records removed longer ago than `age`, and moves the pruning mark of their accounts
/// past them. Answers with the number of records removed.
pub async fn prune(pool: &PgPool, age: Duration) -> Result<u64, sqlx::Error> {
    let seconds = age.as_secs() as f64;
    let removed: Option<i64> = sqlx::query_scalar(
        "WITH gone AS (
             DELETE FROM record
             WHERE data IS NULL AND changed_at < now() - make_interval(secs => $1)
             RETURNING account, revision
         ), marks AS (
             UPDATE account SET pruned_before = GREATEST(account.pruned_before, newest.revision)
             FROM (SELECT account, max(revision) AS revision FROM gone GROUP BY account) newest
             WHERE account.account = newest.account
             RETURNING 1
         )
         SELECT count(*) FROM gone",
    )
    .bind(seconds)
    .fetch_one(pool)
    .await?;
    Ok(removed.unwrap_or_default() as u64)
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Desktop => "desktop",
        Platform::Android => "android",
    }
}

fn parse_platform(name: &str) -> Option<Platform> {
    match name {
        "desktop" => Some(Platform::Desktop),
        "android" => Some(Platform::Android),
        _ => None,
    }
}
