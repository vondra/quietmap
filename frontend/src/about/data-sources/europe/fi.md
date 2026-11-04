---
title: Finland
intro: Traffic volumes for all state highways (Väylävirasto 2024), passenger railway and Tampere tram timetables. Most city streets not covered; rail freight from the national train-kilometres.
---

## Roads

State highways: [Väylävirasto](https://avoindata.suomi.fi/data/fi/dataset/liikennemaarat) traffic volumes (KVL) 2024, total and heavy traffic per road section. Helsinki also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Other city streets have no counts. Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.1 % of a town street's traffic (WHO 2023); electric cars 3.8 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one).

## Railways

The 2026 map averages the services inside its observation window. The archived feeds cover only part of that window; missing dates use the mean of each selected feed’s covered days, labelled as estimated.

| Feed | What it covers |
|---|---|
| [Fintraffic / VR](https://rata.digitraffic.fi/api/v1/trains/gtfs-passenger.zip) | Passenger railway; 150 of the 365 days |
| [Digitraffic national archive](https://rata.digitraffic.fi/api/v1/trains/gtfs-all.zip) | Passenger route types 102 and 109; 87 days. VR takes priority where both match |
| [HSL](https://infopalvelut.storage.hsldev.com/gtfs/hsl.zip) | The archive has no dates inside the window, so it adds no counts |
| [Tampere](http://data.itsfactory.fi/journeys/files/gtfs/latest/gtfs_tampere.zip) | Tampere tram; 3 days |
| [Föli](http://data.foli.fi/gtfs/gtfs.zip) | Turku, which has buses only, so it adds nothing to rail |

Only passenger services are selected from the mixed national archive; its type-100 freight and non-passenger movements are not treated as passenger trains. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Buildings and terrain

The national topographic database has building data; the bulk download requires an API key and is not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.

## Homes and places

Homes mapped in OpenStreetMap: 47 heat pumps and 4 air conditioners (the region's median) per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.

## Checked against

Silenzi in Quota's quiet mountains ([validation](/about/validation)).

<!-- YEAR SOURCES -->
