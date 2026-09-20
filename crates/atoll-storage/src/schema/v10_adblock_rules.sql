CREATE TABLE adblock_rule (
    scope      TEXT NOT NULL,
    pattern    TEXT NOT NULL,
    active     INTEGER,
    banners    INTEGER,
    popups     INTEGER,
    redirects  INTEGER,
    new_tabs   INTEGER,
    changed_at INTEGER NOT NULL,
    PRIMARY KEY (scope, pattern)
);

INSERT INTO adblock_rule (scope, pattern, active, changed_at)
    SELECT 'site', site, 0, added_at FROM adblock_allowed_site;

INSERT INTO adblock_rule (scope, pattern, banners, changed_at)
    SELECT 'global', '', 0, 0 FROM setting WHERE key = 'adblock_enabled' AND value = 'false';

DELETE FROM setting WHERE key = 'adblock_enabled';

DROP TABLE adblock_allowed_site;
