# Structures regression oracle

The unmodified structures producer at `24980735` on `qm260904` wrote the
22-row `structures.arrow` fixture. Its OSM and normalized observation inputs
include competing footprint matches, null OSM identities, a courtyard hole,
ground activities, open roofs, an actual raster, vector heights overriding it,
and surveyed walls replacing only coincident OSM lines. All 20 consumed columns
are compared exactly. The regression test reads the committed 2026 height table;
it is not the historical-year calibration. Normalized observations carry that
same year and a different-year input is rejected.

The tiny raster and all fixture geometries are synthetic. The schema stamps
and geometry encoding are the original producer's contracts. Regeneration must
use the original producer, never the new implementation's output as its oracle.
