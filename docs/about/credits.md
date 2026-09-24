---
title: Credits & terms
intro: The data and tools behind the map, the terms for using it, and what we do and don't log.
nav: hidden
---

## Attribution

None of this would exist without other people's open data. Thank you to everyone below.

**Geometry**

- Roads, railways, airports, buildings, industrial sites, noise walls:
  © [OpenStreetMap](https://www.openstreetmap.org/) contributors (ODbL)
- Additional building outlines: [Overture Maps](https://overturemaps.org/)

**Terrain, ground and heights**

- Elevation: [Copernicus GLO-30 DEM](https://spacedata.copernicus.eu/collections/copernicus-digital-elevation-model) (ESA/Copernicus)
- Forest canopy: [Copernicus Tree Cover Density](https://land.copernicus.eu/en/products/high-resolution-layer-tree-cover-density)
  in Europe, [Hansen/UMD Global Forest Change](https://glad.earthengine.app/view/global-forest-change) elsewhere
- Sealed ground: [ESA WorldCover 2021](https://worldcover2021.esa.int/) (CC BY 4.0), refined by
  [Copernicus Imperviousness Density](https://land.copernicus.eu/en/products/high-resolution-layer-imperviousness) in Europe
- Building heights: [GHS-BUILT-H](https://human-settlement.emergency.copernicus.eu/ghs_buH2023.php) (JRC, CC BY 4.0),
  and for Prague the [IPR Praha building height model](https://opendata.geoportalpraha.cz/maps/ad9aca20e9c042d2b52eb31ff18961b6) (CC BY)

**Traffic**

- Flights: [ADSBExchange](https://www.adsbexchange.com/) for airline traffic,
  [adsb.lol](https://adsb.lol/) for small planes and helicopters — both made possible by
  volunteers who run receivers at home.
- Aircraft noise tables: [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data)
- Ships in European waters: [EMODnet Human Activities](https://emodnet.ec.europa.eu/en/human-activities),
  vessel density 2024 (CC BY 4.0)
- Ships in the rest of the world: **Powered by [Global Fishing Watch](https://globalfishingwatch.org)**
  (CC BY-NC 4.0)
- Road traffic: national censuses and city counts, listed on the country pages. European city
  streets: [EU harmonized traffic volumes](https://github.com/XavB64/traffic-volume-data-EU-cities)
  (CC BY 4.0), with each city's own terms: ODbL for Paris, Grenoble, Rennes and Montpellier,
  CC BY-SA 4.0 for Brno
- Default speed limits: OpenStreetMap Wiki contributors, "Default speed limits" (CC BY-SA 2.0),
  parsed by [osm-legal-default-speeds](https://github.com/westnordost/osm-legal-default-speeds)
- Trains: public [GTFS](https://gtfs.org/) feeds and national timetables, listed on the country pages
- Industry: [E-PRTR](https://industry.eea.europa.eu/) (EEA),
  [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase) (WRI),
  [Global Energy Monitor](https://globalenergymonitor.org/) trackers, national wind turbine registries

**The map you look at**

- Base map: © [CARTO](https://carto.com/about-carto/), © OpenStreetMap contributors
- Terrain base map: © [OpenTopoMap](https://opentopomap.org/)
- Satellite imagery: © [Esri](https://www.esri.com/), Maxar, Earthstar Geographics
- Noise colours: Beate Tomio, [coloringnoise.com](https://www.coloringnoise.com/theoretical_background/new-color-scheme/) (CC BY-NC-ND 4.0)
- Rendering: [MapLibre GL JS](https://maplibre.org/)

## Licence and terms of use

The map, its tiles and its API are provided as a **service**, free to use and embed with
attribution.

- **You may** link to quietmap.org and embed the map or screenshots in your articles, apps
  and projects, with a visible credit "**quietmap.org**" linking back here, plus
  "© OpenStreetMap contributors".
- **You may not** bulk-download tiles, scrape or mirror the dataset, or republish a copy of
  the map as your own service. Raw model data is not distributed.
- **Commercial or high-volume use:** [talk to us first](mailto:info@quietmap.org) — we're
  friendly. Some of our sources are licensed for non-commercial use only, so we need to
  look at each case.

Noise values are model estimates, not measurements; the [methodology](/about/methodology)
explains how far to trust them. **No warranty.** quietmap.org is offered as-is, for
information and orientation — please don't rely on it alone for legal, health, safety or
property decisions.

<details>
<summary><strong>Privacy</strong></summary>

quietmap.org uses **no cookies, no trackers and no analytics scripts** — which is why there is no consent banner to click away.

Like almost every website, our server keeps a standard technical **access log** — IP
address, browser, and requested URLs, including the map coordinates you click — for
security and operations (legitimate interest, GDPR art. 6(1)(f)). Logs rotate automatically
and are kept for at most **30 days**. From them we derive anonymous, aggregated statistics
(visitor countries, browser types, popular areas of the map) — never profiles of
individual visitors, and nothing is shared with third parties.

To open the map in your part of the world, the first view is approximated to **country
level** from your IP address, using an offline database on our own server
([IP Geolocation by DB-IP](https://db-ip.com)). The lookup happens in memory, the result is
not stored, and your IP is never sent to anyone else. Your precise location is used only
if you tap the locate button and grant your browser's permission prompt.

Questions: [info@quietmap.org](mailto:info@quietmap.org).

</details>
