---
title: Methodology
intro: Source models, propagation, standards, known limits and validation.
nav: hidden
---

## Overview

Each source is assigned a sound power from real-world data. Sound is propagated to a
receiver 4 m above ground over actual terrain and around actual buildings, and the
contributions of all sources are summed. The result is Lden, the annual
day-evening-night level. Layers are computed independently and can be toggled separately.

## Roads

Emission follows CNOSSOS-EU (rolling and propulsion noise per vehicle class). Inputs are
traffic volume, heavy-vehicle share, speed and surface; doubling traffic adds about 3 dB.

Geometry comes from OpenStreetMap. Traffic comes from national censuses and city counts
where published; elsewhere from a default for the road's class and surroundings, flagged
as an estimate in the click panel. These defaults are the largest source of error on the
map.

## Railways

Emission follows the CNOSSOS-EU rail method, with separate source levels for passenger,
freight, tram and light rail, and a strong dependence on speed.

Train counts are derived from GTFS feeds and national timetables by routing every trip
along the track network. In the September 2026 recomputation, unmatched daily departures
fell to 89 of 14,925 in France and 194 of 43,572 on the German national feed.

Limitations: freight is absent from almost all public timetables and mostly uses corridor
defaults. On multi-track corridors the total is correct, but its split between parallel
tracks is arbitrary. Metro lines are not yet included.

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

Source levels are 108 dB(A) for large cargo, tanker and passenger ships, 98 dB(A) for tugs,
fishing and service vessels, and 88 dB(A) for yachts, based on pass-by and at-berth
measurements in Livorno, Rotterdam and Gothenburg (Fredianelli et al. 2020; Bernardini et
al. 2022; NEPTUNES; SHIPNOISE). Propagation follows the industrial-source method over
acoustically hard water, with terrain and building screening, up to 12 km.

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

**As sources.** An extension beyond the standards: buildings and leisure facilities
(supermarkets, restaurants, heat pumps, sports grounds, stadiums) are assigned a level by
type and floor area, with a day/night profile. These are assumptions, shown as such in the
click panel.

**As obstacles.** Every building screens with its actual footprint and height. Footprints
come from OpenStreetMap and Overture Maps. Heights come from mapped height or floor count,
from measured regional surveys where available (Prague), and otherwise from the GHSL
average building height. Mapped noise barriers are treated the same way.

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

The model follows these standards but is not certified against them. It is an engineering
estimate for comparison and exploration — not a measurement, a legal noise map, or a
certificate for a particular property.

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
