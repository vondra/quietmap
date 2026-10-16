---
title: Singapore
intro: Main roads use the world defaults; smaller roads are estimated from the buildings around them. MRT and LRT surface sections use class defaults.
---

## Roads

No per-road traffic counts are published. [data.gov.sg](https://data.gov.sg/) has one annual figure, vehicles entering the city; the LTA DataMall speed data needs an API key.

Motorways, trunk and primary roads use the world estimate per lane; secondary and tertiary roads follow the trips made by the buildings within 5 km, streets the trips their buildings make, else class defaults ([world defaults](/about/data-preparation)).

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 2.7 % of a town street's traffic (WHO 2023); electric cars 4.5 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 60 km/h in towns, 50 outside.

## Railways

Mapped surface MRT and LRT sections use the [light-rail default](/about/data-preparation). The LTA timetable needs a registered key and is not loaded.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Jurong Island and the port terminals: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.

## Homes and places

Homes mapped in OpenStreetMap: 29 air conditioners per 100 homes (South-East Asia's figure). Mosques do not call outside. Open-fronted bars and restaurants, their terraces open as long as they are.
