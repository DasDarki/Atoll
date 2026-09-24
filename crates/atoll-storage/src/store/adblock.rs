use atoll_core::Timestamp;
use rusqlite::{Row, params};

use crate::connection::Connected;
use crate::error::StorageResult;

/// What the user decided about the protections of one page, one site or of everything. A field
/// that is `None` says nothing and leaves the decision to the wider rule.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdblockRule {
    /// `global`, `site` or `page`.
    pub scope: String,
    /// The site or the address the rule is about, empty for the rule that counts everywhere.
    pub pattern: String,
    pub active: Option<bool>,
    pub banners: Option<bool>,
    pub popups: Option<bool>,
    pub redirects: Option<bool>,
    pub new_tabs: Option<bool>,
    pub vpn: Option<bool>,
}

impl AdblockRule {
    /// Whether the rule decides nothing, in which case it is not kept.
    pub fn says_nothing(&self) -> bool {
        [
            self.active,
            self.banners,
            self.popups,
            self.redirects,
            self.new_tabs,
            self.vpn,
        ]
        .iter()
        .all(Option::is_none)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            scope: row.get(0)?,
            pattern: row.get(1)?,
            active: row.get(2)?,
            banners: row.get(3)?,
            popups: row.get(4)?,
            redirects: row.get(5)?,
            new_tabs: row.get(6)?,
            vpn: row.get(7)?,
        })
    }
}

/// What the ad blocker does where: everywhere, on one site or on one address.
pub trait AdblockStore: Connected {
    fn adblock_rules(&self) -> StorageResult<Vec<AdblockRule>> {
        let mut statement = self.connection().prepare_cached(
            "SELECT scope, pattern, active, banners, popups, redirects, new_tabs, vpn
             FROM adblock_rule ORDER BY scope, pattern",
        )?;
        let rules = statement
            .query_map([], AdblockRule::from_row)?
            .collect::<Result<_, _>>()?;
        Ok(rules)
    }

    /// Writes the rule down, or removes it once it decides nothing.
    fn set_adblock_rule(&self, rule: &AdblockRule, at: Timestamp) -> StorageResult<()> {
        if rule.says_nothing() {
            self.connection()
                .prepare_cached("DELETE FROM adblock_rule WHERE scope = ?1 AND pattern = ?2")?
                .execute(params![rule.scope, rule.pattern])?;
            return Ok(());
        }
        self.connection()
            .prepare_cached(
                "INSERT INTO adblock_rule
                     (scope, pattern, active, banners, popups, redirects, new_tabs, vpn,
                      changed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(scope, pattern) DO UPDATE SET
                     active = ?3, banners = ?4, popups = ?5, redirects = ?6, new_tabs = ?7,
                     vpn = ?8, changed_at = ?9",
            )?
            .execute(params![
                rule.scope,
                rule.pattern,
                rule.active,
                rule.banners,
                rule.popups,
                rule.redirects,
                rule.new_tabs,
                rule.vpn,
                at.0
            ])?;
        Ok(())
    }

    /// Removes every rule about a single site or address, the one for everything stays.
    fn clear_adblock_exceptions(&self) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM adblock_rule WHERE scope <> 'global'")?
            .execute([])?;
        Ok(())
    }
}

impl<T: Connected + ?Sized> AdblockStore for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Storage;

    fn rule(scope: &str, pattern: &str) -> AdblockRule {
        AdblockRule {
            scope: scope.to_owned(),
            pattern: pattern.to_owned(),
            ..AdblockRule::default()
        }
    }

    #[test]
    fn rules_survive_a_round_trip_and_leave_when_they_say_nothing() {
        let storage = Storage::open_in_memory().unwrap();
        let mut site = rule("site", "example.org");
        site.popups = Some(false);
        site.banners = Some(true);
        storage.set_adblock_rule(&site, Timestamp(7)).unwrap();
        let mut global = rule("global", "");
        global.redirects = Some(true);
        storage.set_adblock_rule(&global, Timestamp(7)).unwrap();
        assert_eq!(storage.adblock_rules().unwrap(), vec![global, site.clone()]);

        site.popups = None;
        site.banners = None;
        storage.set_adblock_rule(&site, Timestamp(8)).unwrap();
        assert_eq!(storage.adblock_rules().unwrap().len(), 1);
    }

    #[test]
    fn the_rules_of_single_sites_can_be_thrown_away_together() {
        let storage = Storage::open_in_memory().unwrap();
        for scope in ["site", "page"] {
            let mut entry = rule(scope, "example.org");
            entry.active = Some(false);
            storage.set_adblock_rule(&entry, Timestamp(1)).unwrap();
        }
        let mut global = rule("global", "");
        global.banners = Some(false);
        storage.set_adblock_rule(&global, Timestamp(1)).unwrap();

        storage.clear_adblock_exceptions().unwrap();
        let left = storage.adblock_rules().unwrap();
        assert_eq!(left, vec![global]);
    }
}
