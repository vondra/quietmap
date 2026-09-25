---
title: United Kingdom
intro: Department for Transport counts on every road class in Great Britain, street counts in six cities. No rail timetable loaded. Northern Ireland uses class defaults.
map: { center: [-2.5, 54.5], zoom: 6 }
---

## Roads

Traffic volumes: [Department for Transport count points](https://roadtraffic.dft.gov.uk/) (AADF; cars, vans, buses, trucks, motorcycles), most recent year per point, June 2026 release. The file covers Great Britain; Northern Ireland has no counts and uses class defaults.

- Motorways, A and B roads take the nearest point of their road number within 15 km whose DfT class fits the OSM class. Slip-road points (a short link named after a slip road, or a motorway or trunk point far below its own road) are never used for the main carriageway.
- C roads and unclassified roads take a manual count (2020 excluded) when the point lies within 12 m of an OSM way and no road of another class runs within 20 m; the whole way takes that count.

London, Birmingham, Manchester, Glasgow, Edinburgh and Cardiff also have counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

No timetable is loaded; the national rail timetable requires registration. Class defaults apply: 80 passenger and 85 freight trains per day on main lines, 30 and 5 on branch lines, 120 trams on tram tracks.

## Checked against

Official English noise map, 120 m from the M25: the model was about 13 dB too loud because a cutting and a noise wall were missing from the data. Details under [how we check it](/about/methodology).
