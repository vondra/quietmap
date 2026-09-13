-- Original source chains, acoustic-piece intervals and exact zero-length source connections.
CREATE TABLE source_ways (
    osm_id INTEGER PRIMARY KEY,
    family TEXT NOT NULL,
    nodes_json TEXT NOT NULL
);
CREATE TABLE source_pieces (
    way_id INTEGER NOT NULL,
    segment_idx INTEGER NOT NULL,
    square TEXT NOT NULL,
    start_vertex INTEGER NOT NULL,
    start_fraction REAL NOT NULL,
    end_vertex INTEGER NOT NULL,
    end_fraction REAL NOT NULL,
    PRIMARY KEY (way_id, segment_idx)
) WITHOUT ROWID;
CREATE INDEX source_pieces_square ON source_pieces(square);
CREATE TABLE node_aliases (
    family TEXT NOT NULL,
    node_id INTEGER NOT NULL,
    canonical_node INTEGER NOT NULL,
    PRIMARY KEY (family, node_id)
) WITHOUT ROWID;
