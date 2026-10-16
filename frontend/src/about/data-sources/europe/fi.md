---
title: Finland
intro: Traffic volumes for all state highways (Väylävirasto 2024), four timetables for trains, metro and trams. Most city streets not covered; rail freight from the national train-kilometres.
---

## Roads

State highways: [Väylävirasto](https://avoindata.suomi.fi/data/fi/dataset/liikennemaarat) traffic volumes (KVL) 2024, total and heavy traffic per road section. Helsinki also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Other city streets have no counts. Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.1 % of a town street's traffic (WHO 2023); electric cars 3.8 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one).

## Railways

Train counts, one busy Wednesday:

| Feed | What it covers |
|---|---|
| [Fintraffic / VR](https://rata.digitraffic.fi/api/v1/trains/gtfs-passenger.zip) | All passenger trains |
| [HSL](https://infopalvelut.storage.hsldev.com/gtfs/hsl.zip) | Helsinki commuter trains, metro and trams |
| [Tampere](http://data.itsfactory.fi/journeys/files/gtfs/latest/gtfs_tampere.zip) | Tampere tram |
| [Föli](http://data.foli.fi/gtfs/gtfs.zip) | Turku, which has buses only, so it adds nothing to rail |

The timetable is passenger-only. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Buildings and terrain

The national topographic database has building data; the bulk download requires an API key and is not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.

## Homes and places

Homes mapped in OpenStreetMap: 47 heat pumps and 4 air conditioners (the region's median) per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.

## Checked against

Silenzi in Quota's quiet mountains.
