CREATE TABLE identities (
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 2),
    number BLOB NOT NULL CHECK(length(number)=8 AND number != zeroblob(8)),
    installation BLOB NOT NULL CHECK(length(installation)=16 AND installation != zeroblob(16)),
    serial BLOB NOT NULL CHECK(length(serial)=8 AND serial != zeroblob(8)),
    PRIMARY KEY(kind,number), UNIQUE(installation,serial)
) STRICT;
CREATE TABLE revisions (
    id INTEGER PRIMARY KEY CHECK(id > 0),
    document BLOB NOT NULL CHECK(length(document) BETWEEN 1 AND 1048576),
    digest BLOB NOT NULL CHECK(length(digest)=32)
) STRICT;
CREATE TABLE media (
    digest BLOB PRIMARY KEY CHECK(length(digest)=32),
    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 256),
    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 256),
    bytes BLOB NOT NULL CHECK(length(bytes) BETWEEN 1 AND 524288)
) STRICT;
CREATE TABLE revision_media (
    revision INTEGER NOT NULL REFERENCES revisions(id),
    media BLOB NOT NULL REFERENCES media(digest),
    PRIMARY KEY(revision,media)
) STRICT;
CREATE TABLE heads (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    current INTEGER NOT NULL REFERENCES revisions(id),
    previous INTEGER REFERENCES revisions(id)
) STRICT;
CREATE TABLE meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    installation BLOB NOT NULL CHECK(length(installation)=16),
    ledger_count INTEGER NOT NULL CHECK(ledger_count BETWEEN 1 AND 10000),
    ledger_digest BLOB NOT NULL CHECK(length(ledger_digest)=32),
    barrier INTEGER NOT NULL DEFAULT 0
) STRICT;
PRAGMA application_id=1380009025;
PRAGMA user_version=1;
