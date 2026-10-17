---
title: United Kingdom
intro: Department for Transport counts on every road class in Great Britain, street counts in six cities. No rail timetable loaded. Northern Ireland has no counts; its roads are estimated from the buildings around them.
---

## Roads

Traffic volumes: [Department for Transport count points](https://roadtraffic.dft.gov.uk/) (AADF; cars, vans, buses, trucks, motorcycles), most recent year per point, June 2026 release. The file covers Great Britain; Northern Ireland has no counts; its roads are estimated from the buildings around them.

- Motorways, A and B roads take the nearest point of their road number within 15 km whose DfT class fits the OSM class. Slip-road points (a short link named after a slip road, or a motorway or trunk point far below its own road) are never used for the main carriageway.
- C roads and unclassified roads take a manual count (2020 excluded) when the point lies within 12 m of an OSM way and no road of another class runs within 20 m; the whole way takes that count.

London, Birmingham, Manchester, Glasgow, Edinburgh and Cardiff also have counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles the share counted on nearby roads, else 0.6 % of a town street's traffic (WHO 2023); electric cars 3.8 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 48 km/h in towns, 97 outside (cars drive 82 on a two-way one), a motorway 113.

## Railways

No timetable is loaded; the national rail timetable requires registration. Class defaults apply: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branch lines, 120 trams on tram tracks.

Freight comes from the class defaults, scaled to the country's goods train-kilometres (Eurostat 2019), 37 % of it at night as at Germany's railway monitors.

## Checked against

Official English noise map, 120 m from the M25 at junction 17: the model reads 5.8 dB above it (63.1 against 57.2 dB Lden). The cutting and the noise wall, missing from the data when it read about 13 dB too loud, now screen the motorway by 10 to 15 dB. Details under [how we check it](/about/validation).

## Buildings and terrain

Terrain from Ordnance Survey's [Terrain 50](https://osdatahub.os.uk/) (© Ordnance Survey, Open Government Licence 3.0); its gaps from Copernicus GLO-30.

## Homes and places

Homes mapped in OpenStreetMap: 2 heat pumps and 4 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays. Pubs without mapped hours close at 23:00 (midnight at weekends).
