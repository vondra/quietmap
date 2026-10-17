---
title: Data sources
intro: Where every number on the map comes from, with links, licences and dates.
---

The map is computed from open data: OpenStreetMap for the roads, railways, buildings and sites, and
public traffic counts, timetables, flight tracks, ship positions, terrain models and registries for
what they carry and how they stand. Where nobody counted, a documented model or default stands in; the region
and country pages below say which is which.

Each version of the map is built once for the whole world. The current one, built in October 2026,
uses the OpenStreetMap planet of September 2026 and flights from September 2025 to August 2026.

## By region and country

What is counted, what is estimated and what is missing in each place:

<!-- SUBPAGES -->

## Credits

None of this would exist without other people's open data. Thank you to everyone below, and to
the national and regional publishers credited on the region and country pages.

### Roads, buildings and sites

- Roads, railways, airports, buildings, industrial sites, noise walls, bus routes, places of
  worship, bars and restaurants: © [OpenStreetMap](https://www.openstreetmap.org/) contributors
  (ODbL)
- Building outlines OpenStreetMap lacks: [Overture Maps](https://overturemaps.org/), release
  2026-08-19.0
- Measured building heights and floor counts: national and city registers, credited on the pages
  of [Czechia](/about/data-sources/europe/cz), [Germany](/about/data-sources/europe/de),
  [the Netherlands](/about/data-sources/europe/nl) and [Spain](/about/data-sources/europe/es)
- Noise walls from official inventories: credited on the pages of
  [the Netherlands](/about/data-sources/europe/nl), [Poland](/about/data-sources/europe/pl) and
  [the United States](/about/data-sources/north-america/us)

### Terrain, ground and weather

Terrain is the [GEDTM30 v1.2](https://zenodo.org/records/18887460) world model (Ho et al. 2025,
OpenGeoHub, CC BY 4.0) with the national terrain models of 17 countries, credited on their pages:
15 in [Europe](/about/data-sources/europe), [the United States](/about/data-sources/north-america/us)
and [Canada](/about/data-sources/north-america/ca).
[Copernicus GLO-30](https://spacedata.copernicus.eu/collections/copernicus-digital-elevation-model)
covers Antarctic ice and the remaining gaps in Great Britain (produced using Copernicus WorldDEM-30
© DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by
the European Union and ESA; all rights reserved).

- Tree cover: [Hansen/UMD Global Forest Change](https://glad.earthengine.app/view/global-forest-change)
  outside Europe; tree height: [GLAD forest height 2020](https://glad.umd.edu/dataset/GLCLUC2020)
  (Potapov et al. 2022, University of Maryland)
- Hard ground (paved, built over or water): [ESA WorldCover 2021](https://worldcover2021.esa.int/)
  (CC BY 4.0), its gaps filled from C3S/ESA CCI Land Cover 2022 v2.1.1
- Europe's tree cover and hard ground: on the [Europe](/about/data-sources/europe) page
- Weather: [ERA5](https://www.ecmwf.int/en/forecasts/dataset/ecmwf-reanalysis-v5) 1991–2020
  climatology (Copernicus Climate Change Service, CC BY 4.0, via ARCO-ERA5; contains modified
  Copernicus Climate Change Service information); yearly temperatures and degree days from
  [WorldClim 2.1](https://www.worldclim.org/) (Fick and Hijmans 2017, CC BY 4.0)
- Heights above the geoid: EGM2008 (NGA) and the national geoid grids of [PROJ-data](https://github.com/OSGeo/PROJ-data)
- Countries: [geoBoundaries](https://www.geoboundaries.org/) CGAZ ADM0

### Traffic

- Road traffic: national censuses and city counts, listed on the region and country pages.
  Hourly shares: the TMAS counters, Japan's road traffic census,
  Baden-Württemberg's counters, Rijkswaterstaat (INWEVA), the UK Department for Transport, and the
  roadside noise monitors of Madrid, Barcelona and Thailand's Pollution Control Department.
  Free-flow speeds: the UK Department for Transport (SPE0102)
- Vehicle fleets: electric cars from the [IEA](https://www.iea.org/) Global EV Data Explorer 2025, motorcycles
  from the [WHO](https://www.who.int/) Global status report on road safety 2023
- Default speed limits: OpenStreetMap Wiki contributors, "Default speed limits" (CC BY-SA 2.0),
  parsed by [osm-legal-default-speeds](https://github.com/westnordost/osm-legal-default-speeds), and
  the [implicit maxspeed rules](https://wiki.openstreetmap.org/wiki/Key:maxspeed#Implicit_maxspeed_values)
  (CC BY-SA 2.0, adapted into a table of passenger-car rules)
- Trains: public [GTFS](https://gtfs.org/) feeds and national timetables, listed on the country
  pages; the typical train from the Eisenbahn-Bundesamt's railway noise monitoring 2023; Europe's
  train-kilometres, freight corridors and night freight on the [Europe](/about/data-sources/europe) page
- Level-crossing horns: the national inventories of [the United States](/about/data-sources/north-america/us)
  and [Canada](/about/data-sources/north-america/ca)
- Flights: [adsb.lol](https://adsb.lol/) for the year, [ADSBExchange](https://www.adsbexchange.com/)
  for the flights only it received on 11 sample days — both made possible by volunteers who run
  receivers at home. Aircraft noise tables: [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data);
  helicopters: EASA Certification Noise Levels – Helicopters, Issue 52 (source acknowledged as its
  terms require)
- Ships: **Powered by [Global Fishing Watch](https://globalfishingwatch.org)** (CC BY-NC 4.0) on
  inland waterways and outside European seas; European seas on the [Europe](/about/data-sources/europe)
  page

### Industry and other sources

- Industry: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase) (WRI),
  [Global Energy Monitor](https://globalenergymonitor.org/) trackers and the registers named on the
  region and country pages; the base levels calibrated on the Czech strategic noise maps of 2022
- Wind turbines standing today: OpenStreetMap and eight national and regional registers,
  credited on the pages of [the United States](/about/data-sources/north-america/us),
  [Canada](/about/data-sources/north-america/ca), [Germany](/about/data-sources/europe/de),
  [Denmark](/about/data-sources/europe/dk), [Sweden](/about/data-sources/europe/se),
  [Norway](/about/data-sources/europe/no), [Spain](/about/data-sources/europe/es) (Castilla-La Mancha)
  and [the Netherlands](/about/data-sources/europe/nl) (with Belgium)
- Measured numbers behind the other sources: voices (ANSI S3.5 via Rindel and Christensen 2012;
  ÖNORM S 5012; Hayne et al. 2011; Flanagan et al. 2023; Ramón-Turner et al. 2025; Ballesteros
  2014), talker heights (Chauvineau 2025; Jacquesson 2017, Université Gustave Eiffel), terrace
  registers (Madrid's 2026 census, Melbourne's CLUE), people at bar doors (Kaplan et al. 2019),
  dinner times (Eurostat HETUS); church bells (LUBW; the Zurich sleep study); prayer times (the
  PrayTimes.org method, Diyanet, JAKIM and Kemenag tables); home heat pumps and air conditioners
  (EHPA, EurObserv'ER, US RECS, coolinggap.eu, heatpumpmonitor, national surveys); car parks
  (Bavarian Environment Agency, parking study 2007)

### Code we test against

- Propagation is tested on the ISO/TR 17534-4 cases as encoded in
  [NoiseModelling](https://github.com/Universite-Gustave-Eiffel/NoiseModelling) (Université Gustave
  Eiffel, UMRAE; GPL-3.0) and follows its numerical fallback for the ground term of diffraction.
- Loudness is tested against [MoSQITo](https://github.com/Eomys/MoSQITo) (Apache 2.0).
- Aircraft are tested on the test cases of UBA Texte 11/2022.

### The map you look at

- Base map: © [CARTO](https://carto.com/about-carto/), © OpenStreetMap contributors
- Terrain base map: © [OpenTopoMap](https://opentopomap.org/)
- Satellite imagery: © [Esri](https://www.esri.com/), Maxar, Earthstar Geographics
- Rendering: [MapLibre GL JS](https://maplibre.org/) and [deck.gl](https://deck.gl/)
- Places to stay: [Stay22](https://www.stay22.com/)

The measurements the model is checked against are listed on the [validation](/about/validation)
page.
