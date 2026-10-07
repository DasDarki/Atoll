CREATE VIRTUAL TABLE page_text USING fts5(
    url UNINDEXED,
    title UNINDEXED,
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);
