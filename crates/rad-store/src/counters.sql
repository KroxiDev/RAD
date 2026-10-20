CREATE TABLE counters (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    folders BLOB NOT NULL CHECK(length(folders)=8),
    buttons BLOB NOT NULL CHECK(length(buttons)=8),
    actions BLOB NOT NULL CHECK(length(actions)=8),
    serial BLOB NOT NULL CHECK(length(serial)=8),
    digest BLOB NOT NULL CHECK(length(digest)=32)
) STRICT;
