---
title: Methodology
intro: Source models, propagation, standards, known limits and validation.
nav: hidden
---

## Overview

Each source is assigned a sound power from observations or explicit assumptions. Sound
is propagated to a receiver 4 m above ground over terrain and around buildings, and the
contributions of all sources are summed. The result is Lden, the annual
day-evening-night level. Layers are computed independently and can be toggled separately.

Accommodation cards show outdoor noise. Listings inside a building use the façade
estimate before wall and window insulation.

## Roads

Emission follows CNOSSOS-EU (rolling and propulsion noise per vehicle class). Inputs are
traffic volume, heavy-vehicle share, speed and surface; doubling traffic adds about 3 dB.

Geometry comes from OpenStreetMap. Traffic comes from national censuses and city counts
where published; elsewhere from a default for the road's class and surroundings, flagged
as an estimate in the click panel. These defaults are the largest source of error on the
map.

### Road defaults

A three-lane urban one-way motorway without a count is estimated at 3 × 12,543 = 37,629
vehicles per day. The defaults below are length-weighted medians of counted public roads,
fitted in September 2026 by road class, direction and surroundings. Local counts and
national estimate tables take precedence.

Motorways, trunks and primary roads with a lane count from 1 to 6 use these rates in
vehicles per day per lane:

| Road | Direction | Rural | Urban | Surroundings unknown |
|---|---|---:|---:|---:|
| Motorway | One-way | 6,174 | 12,543 | 7,331 |
| Motorway | Two-way | 2,305 | 4,396 | 2,305 |
| Trunk | One-way | 2,950 | 6,188 | 3,652 |
| Trunk | Two-way | 1,959 | 4,000 | 2,205 |
| Primary | One-way | 3,314 | 4,980 | 4,579 |
| Primary | Two-way | 1,977 | 4,136 | 2,636 |

Missing or implausible lane counts use the whole carriageway estimates below, in vehicles
per day. Secondary and tertiary roads use these totals regardless of the lane count.

| Road | Direction | Rural | Urban | Surroundings unknown |
|---|---|---:|---:|---:|
| Motorway | One-way | 5,208 | 30,980 | 6,008 |
| Motorway | Two-way | 6,297 | 7,640 | 6,297 |
| Trunk | One-way | 5,061 | 9,744 | 6,652 |
| Trunk | Two-way | 4,260 | 8,289 | 4,840 |
| Primary | One-way | 4,966 | 7,963 | 7,103 |
| Primary | Two-way | 3,644 | 9,322 | 4,500 |
| Secondary | One-way | 6,500 | 9,103 | 8,824 |
| Secondary | Two-way | 2,061 | 6,445 | 3,000 |
| Tertiary | One-way | 2,266 | 4,298 | 4,132 |
| Tertiary | Two-way | 1,002 | 2,562 | 1,506 |

Smaller-road defaults are section totals: residential 500, living street 100,
unclassified 1,340, service 250 and track 5 vehicles per day, before lane, carriageway
and access adjustments. A two-way track without an access tag takes 0.5 vehicles per day.
Local streets can instead be estimated from the buildings they serve. Country pages
list local data and estimates; no generic country multiplier is applied.

Directional counts are kept as published; counts covering both directions are shared
between carriageways. A divided main road can receive half a two-way count when its
opposite carriageway is missing. City street counts describe their own cross-section.

## Railways

Emission follows the CNOSSOS-EU rail method, with separate source levels for passenger,
freight, tram and light rail, and a strong dependence on speed.

Train counts are derived from GTFS feeds and national timetables by routing every trip
along the track network. In the September 2026 recomputation, unmatched daily departures
fell to 89 of 14,925 in France and 194 of 43,572 on the German national feed.

Freight is absent from almost all public timetables and mostly uses estimates. Routing
and allocation across parallel tracks add uncertainty. Above-ground metro sections use
the light-rail model; mapped underground sections do not emit outdoor noise.

Where counts and national estimates are missing, each unknown traffic category uses
these daily defaults. Main, branch and industrial usage must be explicitly mapped;
other ordinary railway lines use the unclassified row. Service-track and parallel-track
allocation can reduce the count assigned to an individual track.

| Railway | Passenger trains/day | Freight trains/day |
|---|---:|---:|
| Main line | 80 | 20 |
| Branch | 30 | 5 |
| Industrial | 0 | 15 |
| Unclassified | 40 | 10 |
| Tram | 120 | 0 |
| Light rail or metro | 80 | 0 |
| Narrow gauge | 10 | 0 |
| Funicular | 40 | 0 |

## Aircraft

Aircraft noise is computed from recorded ADS-B flights: twelve full days of worldwide
airline traffic from ADSBExchange (one per month) and 364 days of general aviation and
helicopters from adsb.lol, covering 2 September 2025 – 1 September 2026. Each aircraft
type uses noise-power-distance data from the EASA ANP database, applied according to
ECAC Doc 29. This is an engineering estimate, not a certified airport study.

- **Ground:** taxiing, take-off roll and apron movements, from recorded movements and
  airport geometry.
- **Airborne:** climb and approach up to about 3,000 m above ground.
- **Cruise:** high-altitude overflights, typically around 20 dB at ground level.

Beyond Doc 29, low-altitude flights are screened by terrain and buildings that block the
line of sight. Only flights received by volunteer ADS-B stations appear; where there is no
receiver, low-altitude traffic is missing.

## Ships

Ship noise is derived from AIS vessel-density products: EMODnet 2024 for European seas
(1 km grid) and Global Fishing Watch elsewhere, including rivers and inner harbours —
12 million water cells and about 131 million vessel-hours per month in total.

Source sound powers are estimated at 108 dB(A) for large ships, 98 dB(A) for work boats
and 88 dB(A) for leisure craft, based on [Fredianelli et al. (2020)](https://doi.org/10.3390/su12051740)
and [Schiavoni et al. (2022)](https://pmc.ncbi.nlm.nih.gov/articles/PMC9518360/).
These class averages do not describe individual vessels. Propagation uses the
industrial-source method over hard water, with terrain and building screening, up to 11.8 km.

Reference results (ships only): Singapore Marina 62.5 dB, Rotterdam Waalhaven 58.0 dB,
Dover 56.3 dB, Rhine at Duisburg 48.5 dB, Danube in Vienna 40.7 dB.

Limitations: vessel speed is not modelled, so ships under way may be a few decibels louder
than shown. Activity is assumed constant over 24 hours, which overstates leisure traffic
at night.

## Industry

Factories, power plants, mines, quarries and wind turbines. Sites come from OpenStreetMap;
plant type comes from E-PRTR (Europe), the Global Power Plant Database and the Global
Energy Monitor trackers for steel, cement and coal. Sound power is estimated from plant
type and size. Operating hours and operating status are unknown.

## Buildings

**As sources.** Buildings and leisure facilities use estimated sound power by type,
area and assumed operating hours. These extensions beyond the transport standards are
not measurements of individual heat pumps, shops or sports grounds.

Open car parks use the [Bavarian parking study (LfU, 2007, sixth edition)](https://www.lfu.bayern.de/publikationen/get_pdf.htm?art_nr=lfu_lae_00045&pdf_nr=0),
with 63 dB(A) sound power for one movement per hour and its searching-traffic term.
We assume 0.4 movements per space per hour from 06:00–22:00 and 0.05 from 22:00–06:00,
then average these into the map's day, evening and night periods. Capacity is estimated
from mapped area. These residential-parking defaults can understate busy shopping sites;
the study's impulse rating surcharge is not included in the sound-energy calculation.
Garages and carports share a generic emission profile; actual ventilation and vehicle
movements are unknown. Explicitly mapped carports and open roof structures receive no
indoor attenuation, but their footprints still screen as solid obstacles.

**As obstacles.** Mapped above-ground buildings and noise barriers screen sound. Open
parking areas, yards and explicitly underground footprints do not. Building heights
come from mapped heights or floor counts, measured surveys where available (Prague),
and otherwise area averages or defaults. Footprints come from OpenStreetMap and Overture
Maps.

## Propagation

ISO 9613-2 in eight octave bands: geometric spreading, atmospheric absorption, ground
effect (per CNOSSOS-EU, verified against the standard's test cases), diffraction over
terrain and buildings, and attenuation by forest according to canopy density.

Terrain is the Copernicus GLO-30 elevation model; canopy density and ground sealing come
from satellite land-cover data. All inputs can be inspected in the map's Advanced panel.
Meteorology is a fixed long-term average.

## Standards

- [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226): road, rail and
  industrial emission; ground effect.
- [ISO 9613-2](https://www.iso.org/standard/74047.html): propagation.
- [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod)
  with [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data)
  data: aircraft.
- [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng): Lden indicator and
  receiver height.

The model uses methods from these standards with the simplifications described above;
it is not certified against them. It is an engineering estimate for comparison and
exploration — not a measurement, a legal noise map, or a property certificate.

## Known limits

- Traffic counts, freight volumes, registry status and ADS-B coverage vary widely between
  countries and usually dominate the uncertainty.
- Roads do not yet include every CNOSSOS correction (gradients, junctions, local
  meteorology).
- Façade reflections are simplified.
- Building and leisure emissions rest on assumed use; source data often cannot distinguish
  a warehouse from a workshop.

## Validation

The model is compared with public measurements from city, airport and railway monitoring
networks. Official strategic noise maps serve as cross-checks, never as calibration
targets. A deviation is first attributed to input data, a justified difference in method,
or a model defect; only a defect leads to a fix.

Example: 120 m from the M25 near London the model showed 70.1 dB against 57.2 dB on the
official English map. The motorway's cutting and noise wall were missing from the input
data; with both included, the model is about 2 dB above the official figure.

Formulas and constants are documented in the
[engine specification](https://github.com/vondra/quietmap/blob/main/engine/noise-compute/SPEC.md).
