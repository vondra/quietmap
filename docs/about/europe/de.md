---
title: Germany
intro: Federal road census (BASt 2021), national DELFI passenger timetable, turbine ratings from the Marktstammdatenregister. State and municipal roads and rail freight not covered.
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

Landesstraßen, Kreisstraßen and other city streets have no counts. Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train, tram and U-Bahn counts: national DELFI timetable from [gtfs.de](https://gtfs.de/en/feeds/de_full/) and [public-transport.earth](https://data.public-transport.earth/gtfs/de), one busy Wednesday. Freight is not covered: DB InfraGO publishes no freight paths.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of a [Marktstammdatenregister](https://www.marktstammdatenregister.de/) entry takes its rated power and hub height from the register.

## Buildings and terrain

The federal building model LoD1-DE is not open, and no state model is loaded. Heights: OpenStreetMap tags, Overture and the typical height for the footprint size.
