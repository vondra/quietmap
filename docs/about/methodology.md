---
title: Methodology
intro: How the map computes noise — sources, propagation physics, standards used, and where the model is honest about its limits.
nav: hidden
---

## How the map works

The map computes environmental noise in three steps:

1. **Sources emit noise** — roads, trains, planes, factories, wind turbines,
   buildings, and leisure areas use public geometry and source data.
2. **Sound travels and fades** — distance, air, ground, terrain, vegetation,
   buildings, and barriers change the received level.
3. **You see the result** — the base atlas samples about every 12 m; selected
   areas can publish an optional z13 tier at about 6 m. Each source layer is
   independent and can be toggled in the map.

![quietmap.org — noise visualization](map-overview.jpg)

## Five noise layers

Each source is modelled independently so visitors can compare it with the combined
result. The canonical formulas, defaults, and reference vectors live in the
[`engine/noise-compute/SPEC.md`](https://github.com/vondra/quietmap/blob/main/engine/noise-compute/SPEC.md).

### Roads

Roads use CNOSSOS-EU Annex II for rolling and propulsion noise. OpenStreetMap
geometry is combined with measured or enriched traffic counts where available;
class defaults fill genuine data gaps. Traffic volume, vehicle mix, speed, and
surface are the main inputs. Doubling traffic adds about 3 dB of source energy.

### Railways

Rail uses the CNOSSOS-EU railway approach with separate treatment for passenger,
freight, tram, and other railway families. Timetables and corridor estimates are
used where available; missing operations remain explicit uncertainty rather than
being presented as measurements.

### Aircraft

Airborne aircraft use an NPD-based approach informed by ECAC Doc 29. Flight traces
come from community ADS-B sources, so coverage is strongest where receivers are
dense. Ground operations are modelled from observed movements and airport
geometry. This is an engineering estimate, not a certified airport study.

### Industrial sources

Factories, power plants, mines, and wind turbines use public registries and
OpenStreetMap classifications. Emissions are estimated from source type, scale,
and operating assumptions; registry coverage and operating status vary by country.

### Buildings and settlements

Buildings and mapped leisure areas are an atlas-scale extension rather than a
standardised strategic-noise category. Their source levels use tagged function,
footprint, height, and activity assumptions. Buildings also affect propagation:
vector footprints and explicit barriers provide screening where the obstacle data
is present. A raster height value is not treated as a silent replacement for a
missing footprint obstacle.

## Propagation

Surface sources use ISO 9613-2 propagation over eight octave bands before
A-weighting. Distance spreads sound; atmosphere absorbs it; ground, terrain,
vegetation, buildings, and barriers alter the path. Terrain and screening are
combined according to the standard's barrier rule, while vegetation and the
building-enclosure reflection heuristic are applied separately. Weather is a
fixed long-term assumption, not a forecast for a particular hour.

The production tile painter requires its region's obstacle input to be complete;
a missing vector obstacle shard is a build error, never a silent substitution with
a different building model. This keeps failed input visible instead of publishing a
map with different physics from the requested model.

## Standards and scope

- [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226) supplies the
  surface-source emission framework.
- [ISO 9613-2](https://www.iso.org/standard/74047.html) supplies the propagation
  framework for surface sources.
- Aircraft use an NPD approach informed by
  [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod).
- The Lden indicator follows
  [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng).

The result is an engineering estimate for comparison and exploration. It is not a
measurement, a legal noise map, or a certificate for a particular property.

## Known limits

- Traffic, rail operations, registry status, and ADS-B reception are uneven across
  countries and can dominate the uncertainty.
- Roads do not yet include every CNOSSOS correction, such as detailed gradients,
  intersections, or local meteorology.
- The receiver grid is a Web-Mercator pixel grid, not a façade survey.
- Buildings and leisure activity use stated assumptions where no measurements
  exist; indoor and outdoor uses can be indistinguishable in source data.
- Tile propagation stores the combined result. The popup exposes useful path and
  source detail, but it is not a substitute for a full acoustic study.

## Validation

Validation compares the model with commensurable public measurements from city,
airport, and railway monitoring networks. Official strategic maps are useful
cross-checks, but they are not calibration targets. A deviation is first assigned
to input coverage, a justified methodology difference, or a model defect; only a
defect becomes a fix. The project records the reasoning behind confirmed fixes so
that later dataset generations remain comparable.

For implementation-level formulas, constants, and reference vectors, see the
[noise-compute specification](https://github.com/vondra/quietmap/blob/main/engine/noise-compute/SPEC.md).
