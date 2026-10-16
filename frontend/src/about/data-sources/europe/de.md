---
title: Germany
intro: Federal road census (BASt 2021), national DELFI passenger timetable, turbine ratings from the Marktstammdatenregister. State and municipal roads not covered; rail freight from the national train-kilometres.
---

## Roads

Autobahnen and Bundesstraßen: [BASt road traffic census](https://www.bast.de/) (SVZ) 2021. BASt vehicle groups map as follows:

| BASt group | On the map |
|---|---|
| LVm, cars and light vans | Light vehicles |
| Bus and LoA, buses and trucks without trailer | Medium vehicles |
| LZ, trucks with trailer | Heavy vehicles |
| Krad | Motorcycles |

Baden-Württemberg's [permanent counting stations](https://mobidata-bw.de/de/dataset/stundenwerte_dauerzaehlstellen) give every German road its day, evening and night shares: their hourly counts of 2025, by road group, cars and lorries apart.

Berlin and Hamburg: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Landesstraßen, Kreisstraßen and other city streets have no counts. Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles the share counted on nearby roads, else 1.4 % of a town street's traffic (WHO 2023); electric cars 3.4 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 100 outside (cars drive 84 on a two-way one), a motorway 130.

## Railways

Train, tram and U-Bahn counts: national DELFI timetable from [gtfs.de](https://gtfs.de/en/feeds/de_full/) and [public-transport.earth](https://data.public-transport.earth/gtfs/de), one busy Wednesday. DB InfraGO publishes no freight paths. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Industry

Wind turbines are OpenStreetMap points. A turbine paired with a [Marktstammdatenregister](https://www.marktstammdatenregister.de/) entry, one to one and nearest first within 500 m, takes its rated power and hub height from the register.

The register adds 1,265 turbines OpenStreetMap lacks and removes 523 it calls dismantled.

## Buildings and terrain

North Rhine-Westphalia: measured heights from the [NRW 3D building model LoD1](https://www.opengeodata.nrw.de/produkte/geobasis/3dg/lod1_gml/). Elsewhere: OpenStreetMap tags, floor counts, Overture heights and the typical height for the footprint size. The federal LoD1-DE model is not open.

Terrain from the laser surveys of 15 Länder (DGM1, DGM5); Berlin's is not loaded.

## Homes and places

Homes mapped in OpenStreetMap: 4 heat pumps and 6 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal; church clocks strike the quarters by day and in the evening. Mosques: only those with a mapped minaret call, once on Fridays.

## Checked against

The Eisenbahn-Bundesamt's railway monitors.
