use atoll_core::Timestamp;
use rusqlite::params;

use crate::connection::Connected;
use crate::error::StorageResult;

/// Sites the ad blocker is switched off for.
pub trait AdblockStore: Connected {
    fn adblock_allowed_sites(&self) -> StorageResult<Vec<String>> {
        let mut statement = self
            .connection()
            .prepare_cached("SELECT site FROM adblock_allowed_site ORDER BY site")?;
        let sites = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(sites)
    }

    fn set_adblock_site_allowed(
        &self,
        site: &str,
        allowed: bool,
        at: Timestamp,
    ) -> StorageResult<()> {
        if allowed {
            self.connection()
                .prepare_cached(
                    "INSERT INTO adblock_allowed_site (site, added_at) VALUES (?1, ?2)
                     ON CONFLICT(site) DO NOTHING",
                )?
                .execute(params![site, at.0])?;
        } else {
            self.connection()
                .prepare_cached("DELETE FROM adblock_allowed_site WHERE site = ?1")?
                .execute([site])?;
        }
        Ok(())
    }
}

impl<T: Connected + ?Sized> AdblockStore for T {}
