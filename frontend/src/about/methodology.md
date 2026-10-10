---
title: Methodology
intro: Source models, propagation, standards, known limits and validation.
nav: hidden
---

## Overview

Each source is assigned a sound power from observations or explicit assumptions. Sound
is propagated to a receiver 4 m above ground over terrain and around buildings, and the
contributions of all sources are summed. The result is Nden, how loud the place sounds on
average over the day in sone, and Lden, the annual day-evening-night level, with the day,
evening and night levels and the levels exceeded 5, 10, 50 and 90 % of the time. A click computes its point on the spot
from every source within 12 km, aircraft within 16 km. Layers are computed independently
and can be toggled separately.

Inside a building, the click panel shows the level at its noisiest façade point, placed as
the EU method (CNOSSOS-EU) places receivers for building exposure. Receivers sit at most
5 m apart along each façade, 0.1 m in front of the wall and 4 m above ground, with the
façade's own reflection left out; the loudest by Lden is shown. A building with no exposed
façade gets no answer. Courtyards and open ground show the level at the point itself. No
indoor attenuation is applied anywhere.

## Roads

Emission follows CNOSSOS-EU (rolling and propulsion noise per vehicle class). Inputs are
traffic volume, the shares of medium and heavy vehicles, speed, surface, gradient,
acceleration at junctions and the place's yearly air temperature; doubling traffic adds
about 3 dB. Battery-electric cars, at their country's share of the 2024 stock (IEA), make
rolling noise only. Motorcycles run 3.8 dB louder than cars at the same speed and 2.9 dB
louder pulling away (ASJ RTN-Model 2018); their share comes from counts and national
fleets (WHO 2023).

Geometry comes from OpenStreetMap. Traffic comes from national censuses and city counts
where published. Where nobody counted, a street carries the trips its buildings make:
every building, OpenStreetMap's and the Overture footprints OpenStreetMap lacks, sends its
trips along the street network to the nearest main road. Uncounted motorways, trunk and
primary roads in most of Europe, the United States, Mexico, Chile, Colombia and New Zealand
follow one model of the trips made within 1, 5 and 15 km, fitted on counted roads; secondary
and tertiary roads follow a fit of the same kind in Europe. Elsewhere a road keeps a fixed
estimate for its class and surroundings — about half of the world's main-road length. All
estimates are flagged in the click panel. They are the largest source of error on the map.

The vehicle mix is counted where counted; otherwise a main road takes the counted median of
its class, a local street 1 % medium and 2 % heavy vehicles, and a US road its state's mix
by road group (FHWA, table VM-4). The shares of day, evening and night traffic are measured
in the United States, Japan, Germany, the Netherlands, Great Britain, Spain and Thailand,
elsewhere taken from the region's measurements.

Speeds are the posted limit. An unsigned road takes its country's legal limit for its
class, or the median signed speed of its class in the country where that is lower; a
residential street 30 km/h, a living street, service road or track 20. On a two-way road
outside towns at a limit of 80 km/h or more, traffic drives at 0.845 of the limit, the
free-flow share measured in Great Britain. Lorries and buses drive at most 80 km/h, 90 to
105 in thirteen countries. Uncounted roads also carry the buses of mapped bus routes.

### Road defaults

A three-lane urban one-way motorway without a count or a model is estimated at
3 × 12,543 = 37,629 vehicles per day. The defaults below are length-weighted medians of
counted public roads, fitted in September 2026 by road class, direction and surroundings.
Local counts, the models above and national estimate tables take precedence.

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
| Secondary | One-way | half of two-way (1,030) | half of two-way (3,222) | half of two-way (1,500) |
| Secondary | Two-way | 2,061 | 6,445 | 3,000 |
| Tertiary | One-way | 2,266 | 4,298 | 4,132 |
| Tertiary | Two-way | 1,002 | 2,562 | 1,506 |

Smaller roads where too few buildings are mapped keep section totals: residential 500,
living street 100, unclassified 1,340, service 250 and track 5 vehicles per day, before
lane, carriageway and access adjustments.

Directional counts are kept as published; counts covering both directions are shared
between carriageways. A divided main road can receive half a two-way count when its
opposite carriageway is missing. City street counts describe their own cross-section.

## Railways

Emission follows the CNOSSOS-EU rail method, with separate source levels for passenger,
freight, tram and light rail, and a strong dependence on speed. The four levels are set
to the typical train by CNOSSOS-EU's per-vehicle computations and the pass-bys at
Germany's 19 railway noise monitors (2023); freight on European lines is the EU fleet's,
quieter than elsewhere. A heavy train's sound power is taken at no less than 50 km/h, a
light-rail train's at 30; freight runs at no more than 89 km/h.

Train counts are derived from GTFS feeds and national timetables by routing every trip
along the track network. In the September 2026 recomputation, unmatched daily departures
fell to 89 of 14,925 in France and 194 of 43,572 on the German national feed. A stretch of
track between two switches carries one count, the one most of its length has: a short
piece, a bridge above all, no longer keeps a count of its own.

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

In 31 European countries these defaults are scaled so that the country's lines carry its
official train-kilometres (Eurostat 2024; Great Britain 2019). In the EU, freight without
a count weighs four times the average on a TEN-T core corridor, twice on a TEN-T freight
line and half off them, and 37.6 % of it runs between 22 and 06 h, as counted at the
German monitors. Lines mapped as passenger-only carry no freight.

At US and Canadian public level crossings with trains, locomotives sound their
horns on approach: each travel direction is a line segment up to a quarter mile
long ending at the crossing, with half the crossing's trains sounding. Full-day
quiet zones stay silent, as do partial zones at night. Rail yards, heritage lines and
curve squeal are not modelled.

## Aircraft

Aircraft noise is computed from recorded ADS-B flights: a year of flights from adsb.lol,
September 2025 – August 2026, with the flights only ADSBExchange received added from 11
sample days. Each aircraft type uses noise-power-distance data from the EASA ANP database
(a similar aircraft where the database has none), applied according to ECAC Doc 29, fourth
edition: altitude above the geoid, thrust from the observed climb and acceleration, flaps
and gear on approach. Helicopters take their type's certification level. This is an
engineering estimate, not a certified airport study.

- **Ground:** taxiing, take-off and landing rolls on the runways and taxiways, mapped or
  found from the flights' ground tracks.
- **Airborne:** climb and approach up to about 3,000 m above ground.
- **Cruise:** high-altitude overflights, typically around 20 dB at ground level.

The flights are summed into boxes of airspace; at 27 test points a box reads back within
0.2 dB of every flight summed on its own. Beyond Doc 29, flights are screened by terrain and
buildings that block the line of sight. The click lists the ten loudest flights. Only
flights received by volunteer ADS-B stations appear; where there is no receiver,
low-altitude traffic is missing — near nine of 59 large airports (Beijing, Shanghai Pudong,
Cairo, Casablanca, Riyadh, Tehran, Accra, Moscow Sheremetyevo, Lagos) the map reads 5 to
20 dB too quiet.

## Ships

Ship noise is derived from AIS vessel-density products: EMODnet 2024 for European seas
(1 km grid) and Global Fishing Watch elsewhere, including rivers and inner harbours —
12 million water cells and about 131 million vessel-hours per month in total.

Source sound powers are estimated at 108 dB(A) for large ships, 98 dB(A) for work boats
and 88 dB(A) for leisure boats, based on [Fredianelli et al. (2020)](https://doi.org/10.3390/su12051740)
and [Schiavoni et al. (2022)](https://pmc.ncbi.nlm.nih.gov/articles/PMC9518360/).
These class averages do not describe individual vessels. Sound travels over hard water,
with terrain and building screening.

Limitations: vessel speed is not modelled, so ships under way may be a few decibels louder
than shown. Activity is assumed constant over 24 hours, which overstates leisure traffic
at night.

## Industry

Factories, power plants, mines, quarries and wind turbines. Sites come from OpenStreetMap;
plant type comes from E-PRTR (Europe, by Annex I sub-activity), the Global Power Plant
Database and the Global Energy Monitor trackers for steel, cement and coal. A registry
site claims the smallest mapped polygon containing it; where several registry points fall
inside one plant, the loudest plant type wins. Sound power is estimated from plant
type and size. Coal and lignite mines run day and night; other mines and quarries,
offices and warehouses keep day-oriented hours. Inactive sites are silent.

Wind turbines are the standing ones of OpenStreetMap and of eight national registers (the
United States, Canada, Germany, Denmark, Sweden, Norway, Castilla-La Mancha, and the
Netherlands with Belgium), which add turbines OpenStreetMap lacks and remove those they
call dismantled. They emit their annual operating level: the published maximum for their
rated power, minus an operating allowance for calm and part-load hours (about 2 dB on
average). Solar farms emit from their inverters during daylight (about 88 dB(A) per MW,
from manufacturer data; untagged farms assume 0.55 MW per hectare) and are silent at
night. Substations hum around the clock from their transformers (IEC 551, from the
transformer rating or a class median). Wind-farm outlines themselves are silent — only
the turbines emit.

## Buildings and places

**As sources.** Homes mapped in OpenStreetMap emit the outdoor units of heat pumps and air
conditioners their country's households own (national surveys in 35 countries, the United
States, Canada and the Gulf; elsewhere their region's median), running the hours their
climate asks for: a Prague home 45 dB(A) by day, a Bangkok home 51. Buildings known only
from Overture's footprints emit nothing. Shops, schools, hotels and other buildings use
estimated sound power by type, area and assumed operating hours. These extensions beyond
the transport standards are not measurements of individual heat pumps, shops or sports
grounds. Sports pitches are the exception with documented hours: both kinds radiate the
Sport England typical in-use level (58 dB LAeq,1h at 10 m from the sideline); grass
pitches take 5 hours a week over a September–May season, floodlit artificial turf 40
hours a week year-round, and both are silent at night. Outdoor shooting ranges use
published per-shot levels with 20,000 shots a year, daytime only.

Open car parks use the [Bavarian parking study (LfU, 2007, sixth edition)](https://www.lfu.bayern.de/publikationen/get_pdf.htm?art_nr=lfu_lae_00045&pdf_nr=0),
with 63 dB(A) sound power for one movement per hour and its searching-traffic term.
We assume 0.4 movements per space per hour from 06:00–22:00 and 0.05 from 22:00–06:00,
then average these into the map's day, evening and night periods. Capacity is estimated
from mapped area. Street parking carries only the cars of the buildings around it.

Church bells ring in Europe and Cyprus: the prayer bells three times a day for three
minutes and a Sunday peal; German and Swiss church clocks strike the quarters by day and
in the evening, Swiss ones at night too. Orthodox churches ring only before Saturday's and
Sunday's services. A peal is 114 dB(A), a cathedral's 122.

Mosques call to prayer at their place's prayer times, from horns at three quarters of a
mapped minaret's height, else 15 m up: five calls a day of about three minutes at
118 dB(A). Country rules change this: Saudi Arabia calls at a third of the power,
Indonesia adds the recitation before the call, Turkey and Cyprus the Friday sala; Chinese,
Tajik and Singapore mosques do not call outside. In most of Europe, the Americas,
Australia, New Zealand and East Asia only mosques with a mapped minaret call, once on
Fridays.

The people outside bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food
places sit on their terraces and stand at their doors, by the mapped opening hours, else
the country's custom; nightlife streets add their crowds at night. Terrace seats come from
the registers of Madrid and Melbourne, occupied 75 % on weekend evenings and nights and
30 % by day — an assumption, as nothing is measured.

**As obstacles.** Mapped above-ground buildings and noise barriers screen sound. Open
parking areas, yards, explicitly underground footprints, carports and open roofs do not:
a roof on posts has no wall. Building heights come, in this order, from measured
surveys (North Rhine-Westphalia, the Netherlands, Prague), mapped heights,
floor counts (Czechia's RÚIAN and Spain's Catastro registers fill those OpenStreetMap
lacks), Overture heights, and finally the typical height of a building of that footprint
size. Noise walls come from official barrier inventories where one is open (Poland, the
Netherlands, Washington, Florida, Virginia) and from OpenStreetMap elsewhere; a wall
without a measured height takes its country's average wall height. Footprints come from
OpenStreetMap and Overture Maps.

## Propagation

CNOSSOS-EU in eight octave bands: geometric spreading, atmospheric absorption (ISO
9613-1), ground effect, diffraction over terrain, buildings and barriers, and the share of
favourable, downward-refracting weather by direction. Forest takes nothing off: CNOSSOS-EU
has no term for foliage, and the ground under trees counts as soft ground.

Terrain is a bare-earth model: national LiDAR surveys in 17 countries (see the credits
page) over the GEDTM30 world model; ground sealing comes from imperviousness data, water
counts as hard ground. All inputs can be inspected in the map's Advanced panel.
Meteorology (favourable-weather probability, temperature, humidity) is a 1991–2020 ERA5
climatology.

Reflections are not traced. A receiver among buildings gains 3 dB when more than half of
nine points 75 m apart around it fall inside buildings taller than 5 m, 1.5 dB when more
than a fifth do; flights gain nothing.

## Time and loudness

How the level spreads over each period is computed, not drawn: for every hour of road
traffic and each of ten weather states (one for all sources at a time), each road, railway
and flight path is a random stream of passes at its own rate, bells and calls sound in
their share of the period, and industry, buildings and ships are steady; their sum's spread
gives the levels exceeded 5, 10, 50 and 90 % of the time.

Nden, the headline, is the mean over the whole day of how loud every moment is: loudness in
sone after ISO 532-1 (Zwicker), the period's received spectrum set to the moment's level, the
evening counted 5 dB and the night 10 dB louder, as Lden counts them, and the periods
weighed by their hours. A sound counts by how loud it is and how long it lasts: a car every
few hours by a window adds little, a constant hum all of its loudness. Each source in the
list shows its own Nden, as if it sounded alone.

## Standards

- [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226): road, rail and
  industrial emission; propagation; receivers at façades.
- [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod)
  with [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data)
  data: aircraft.
- [ISO 532-1](https://www.iso.org/standard/63077.html): loudness.
- [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng): Lden indicator and
  receiver height.

The model uses methods from these standards with the simplifications described above;
it is not certified against them. It is an engineering estimate for comparison and
exploration — not a measurement, a legal noise map, or a property certificate.

## Known limits

- Traffic counts, freight volumes, registry status and ADS-B coverage vary widely between
  countries and usually dominate the uncertainty.
- About half of the world's main-road length and every railway outside Europe without a
  timetable carry fixed estimates, not their own traffic.
- Natural sound (wind, water, birds) is not modelled: where human-made sound is faint, a
  place sounds louder than the map says.
- Façade reflections are simplified.
- Building and leisure emissions rest on assumed use; source data often cannot distinguish
  a warehouse from a workshop.
- Not modelled: roads and railways in tunnels, rail yards, motorsport, deliveries, building
  sites, garden machinery, music and festivals.

## Validation

The model is compared with public measurements from city, airport and railway monitoring
networks before a version goes live. Official strategic noise maps serve as cross-checks,
never as calibration targets. A deviation is first attributed to input data, a justified
difference in method, or a model defect; only a defect leads to a fix.

Model minus measurement on the current version: the median over the stations, and the
difference half of them stay within.

| Stations | n | Median | Half within |
|---|---:|---:|---:|
| Paris near its airports, Bruitparif, Lden 2024 | 22 | +0.0 dB | 1.3 dB |
| Paris streets, Lden 2025 | 16 | −2.6 dB | 2.6 dB |
| Barcelona, Lden 2025 | 154 | −1.7 dB | 4.9 dB |
| Madrid, Lden 2025 | 31 | +2.4 dB | 3.2 dB |
| Dublin, Lden 2025 | 14 | −4.4 dB | 5.2 dB |
| Poland's roads, 06–22, 2018–2024 | 6,616 | +0.4 dB | 3.0 dB |
| German railways, 06–22 and 22–06, 2023 | 19 | −0.1 and −3.0 dB | 1.0 and 3.1 dB |
| Prague airport, aircraft, 06–22 and 22–06, January 2026 | 14 | −0.2 and +1.1 dB | 1.6 and 2.2 dB |
| Minneapolis–St Paul airport, aircraft, DNL 2025–26 | 39 | +0.3 dB | 1.1 dB |
| Zurich airport, aircraft, 06–22, 2024 | 4 | −6.1 dB | 6.1 dB |
| Accra, 24 h, 2019 | 146 | −9.7 dB | 9.7 dB |

The rail emission is fitted on the German stations, so that row is no independent test.
All of these stand in loud places. The quiet places the map is for are hardly checked
yet: the only set so far, Silenzi in Quota's 73 single-day samples in quiet mountains,
measures everything heard, nature included, and reads 12.9 dB above the model, as it
should; but at 15 of the points the model is louder than everything measured, each time
because of a road.

Example: 120 m from the M25 near London the model showed 70.1 dB against 57.2 dB on the
official English map. The motorway's cutting and noise wall were missing from the input
data; with both included, the model is about 2 dB above the official figure.
