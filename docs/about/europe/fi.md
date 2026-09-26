---
title: Finland
intro: Traffic volumes for all state highways (Väylävirasto 2024), four timetables for trains, metro and trams. Most city streets and rail freight not covered.
map: { center: [25.5, 64.0], zoom: 5 }
---

## Roads

State highways: [Väylävirasto](https://avoindata.suomi.fi/data/fi/dataset/liikennemaarat) traffic volumes (KVL) 2024, total and heavy traffic per road section. Helsinki also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Other city streets have no counts. Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train counts, one busy Wednesday:

| Feed | What it covers |
|---|---|
| [Fintraffic / VR](https://rata.digitraffic.fi/api/v1/trains/gtfs-passenger.zip) | All passenger trains |
| [HSL](https://infopalvelut.storage.hsldev.com/gtfs/hsl.zip) | Helsinki commuter trains, metro and trams |
| [Tampere](http://data.itsfactory.fi/journeys/files/gtfs/latest/gtfs_tampere.zip) | Tampere tram |
| [Föli](http://data.foli.fi/gtfs/gtfs.zip) | Turku, which has buses only, so it adds nothing to rail |

Freight is not covered: the timetable is passenger-only.

## Buildings and terrain

The national topographic database has building data; the bulk download requires an API key and is not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.
