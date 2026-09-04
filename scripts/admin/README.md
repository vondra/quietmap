# Prepared geography

Run after the OSM extract and before country-dependent enrichment:

```sh
python scripts/admin/build_admin.py --prepared-dir <prepared-year> \
  --boundaries <source>/geoBoundariesCGAZ_ADM0_s0005.geojson
```

Requires NumPy, Shapely 2 and PyArrow. Repeat `--square z9/x/y` for a subset.
The supplied CGAZ dataset must be the preserved v6 source used by dev1;
verify it against its independently preserved `SHA256SUMS` before building.
No data is downloaded by the builder.

The dev1 geographic policy is retained: exact country polygons and holes,
explicit disputed-area mappings, a uniquely attributable 2 km coastal buffer,
polar handling, and country-gated metro defaults. Shapely supplies the shared
spatial index and polygon operations; no custom ray-casting index is needed.
Country, city and continent are baked at each road/rail segment midpoint.
Partition admin records supply the receiver fallback in the existing runtime.

The bake preserves row order, record-batch boundaries and all original metadata,
including the spatial batch index. Files are verified before atomic replacement;
an unchanged rerun leaves their bytes untouched. Run it sequentially with other
enrichers, which must not write the same Arrow files concurrently.

```sh
QM_ADMIN_BOUNDARIES=<source>/geoBoundariesCGAZ_ADM0_s0005.geojson \
  python -m unittest discover -s scripts/admin -v
```

The data-bearing test uses the same independently labelled geographic probes
as dev1. Other tests use tiny in-memory polygons and Arrow files.
