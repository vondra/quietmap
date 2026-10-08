---
title: Germany
intro: Federal road census (BASt 2021), national DELFI passenger timetable, turbine ratings from the Marktstammdatenregister. State and municipal roads not covered; rail freight from the national train-kilometres.
map: { center: [10.5, 51.2], zoom: 6 }
---

## Roads

Autobahnen and Bundesstraßen: [BASt road traffic census](https://www.bast.de/) (SVZ) 2021. BASt vehicle groups map as follows:

| BASt group | On the map |
|---|---|
| LVm, cars and light vans | Light vehicles |
| Bus and LoA, buses and trucks without trailer | Medium vehicles |
| LZ, trucks with trailer | Heavy vehicles |
| Krad | Motorcycles |

Baden-Württemberg: hourly counts for 2025 from the [permanent counting stations](https://mobidata-bw.de/de/dataset/stundenwerte_dauerzaehlstellen). Roads within 200 m of a station take its measured day, evening and night split; elsewhere the split is a default.

Berlin and Hamburg: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Landesstraßen, Kreisstraßen and other city streets have no counts. Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 1.4 % of a town street's traffic (WHO 2023); electric cars 3.4 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 100 outside (traffic drives 84 on a two-way one), a motorway 130.

## Railways

Train, tram and U-Bahn counts: national DELFI timetable from [gtfs.de](https://gtfs.de/en/feeds/de_full/) and [public-transport.earth](https://data.public-transport.earth/gtfs/de), one busy Wednesday. DB InfraGO publishes no freight paths. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of a [Marktstammdatenregister](https://www.marktstammdatenregister.de/) entry takes its rated power and hub height from the register.

The register adds 1,265 turbines OpenStreetMap lacks and removes 523 it calls dismantled.

## Buildings and terrain

North Rhine-Westphalia: measured heights from the [NRW 3D building model LoD1](https://www.opengeodata.nrw.de/produkte/geobasis/3dg/lod1_gml/). Elsewhere: OpenStreetMap tags, floor counts, Overture heights and the typical height for the footprint size. The federal LoD1-DE model is not open.

Terrain from the Länder's laser surveys (DGM1, DGM5).

## Homes and places

Homes mapped in OpenStreetMap: 4 heat pumps and 6 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal; church clocks strike the quarters by day and in the evening. Mosques: only those with a mapped minaret call, once on Fridays.

## Checked against

The Eisenbahn-Bundesamt's railway monitors.
