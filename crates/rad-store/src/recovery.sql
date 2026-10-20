CREATE TABLE recovery (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    hold INTEGER NOT NULL CHECK(hold >= 0)
) STRICT;
INSERT INTO recovery VALUES(1,0);
