---
title: Portugal
intro: City traffic counts in Lisbon, passenger trains from the CP timetable. Other roads are estimated from the buildings around them. Fertagus and Lisbon trams not covered; rail freight from the national train-kilometres.
---

## Roads

Lisbon: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Elsewhere motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km; streets carry the trips their buildings make ([method](/about/data-preparation)).

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.3 % of a town street's traffic (WHO 2023); electric cars 2.3 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside, a motorway 120.

## Railways

[CP Comboios de Portugal](https://publico.cp.pt/gtfs/gtfs.zip) supplies national and commuter
train schedules, averaged over the observation window. The archived Metro do Porto feed does
not cover the 2026 window. Metro Sul do Tejo's archive keeps holiday dates through 2030 but its
regular schedules end on 7 September 2025; it cannot establish annual service and is excluded.
Both light-rail networks retain class estimates. Carris Metropolitana's loaded feed contains
no selected rail service.

Fertagus trains over the 25 de Abril bridge and the Lisbon trams have no feed loaded. The timetables are passenger-only. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Buildings and terrain

Terrain from DGT's laser survey ([MDT LiDAR](https://www.dgterritorio.gov.pt/levantamento-lidar-de-portugal-continental-0), 2 m; CC BY 4.0) for about 11 % of the country; the rest from the GEDTM30 world model.

## Homes and places

Homes mapped in OpenStreetMap: 8 heat pumps and 17 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours open to 02:00 (03:00 at weekends), dinner 20-24 h.

<!-- YEAR SOURCES -->
