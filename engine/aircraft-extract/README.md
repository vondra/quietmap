# Aircraft extraction

Observed ADS-B flights become the three popup aircraft artifacts under
`PREPARED_YEAR_DIR/z9/x/y/`; no heatmap tiles are built here.

1. Parse complete daily archives, deduplicate flights, and preserve feed identity.
2. Sample the DEM once, classify phases, and emit bounded segment batches.
3. Shuffle airborne/ground by z9 and discover unmapped airstrips.
4. Write airborne flight geometry, fine z15 cruise aggregates, and ground energy.
5. Union ground movement identities globally, then stamp each airport's summary
   into the `qm_airport_summaries` footer of every `airport_traffic.arrow` owning
   its ground traffic.

Airborne and ground geometry uses exact Int32 z30 coordinates; airborne heights
use Int16 metres. Cruise rows carry explicit Float64 centroids. The producer and
runtime share current contracts through `square-store::aircraft_contract`.

Every day of the exposure year reads the primary provider (adsb.lol); its
month-firsts also read the secondary provider (ADSBexchange), which adds only
what the primary did not receive. Stage 0 writes a content receipt per
provider-day; a day failing its receipts is missing, never observed-empty.
`baseline_days` and `increment_days` are the only shuffle manifests, and every
output carries their counts and SHA-256 (`SamplingWindow`). Wrong dates,
corrupt archives or malformed Arrow inputs fail loudly.

Run `scripts/run-aircraft-extract.sh --help` for the wrapper. It requires explicit
source, raster-root and year-output paths. To execute one phase use
`run-all --from-stage <phase> --until-stage <phase>`; single phases share its
validation. A regional build needs a fresh year output tree: regional flight
identities cannot replace an existing global union. The wrapper preserves
partial work and never deletes it to force a restart.

Acoustic kernels live in `engine/noise-compute`. Tests cover
archive corruption, window routing, actual producer-to-popup IPC roundtrips,
cruise distance conservation and ground energy conservation across partition edges.
