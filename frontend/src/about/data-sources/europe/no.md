---
title: Norway
intro: Traffic volumes on state and county roads (NVDB), national passenger timetable, wind farm ratings from the NVE register. Municipal streets not covered; rail freight from the national train-kilometres.
---

## Roads

State and county roads: [NVDB](https://nvdbapiles.atlas.vegvesen.no/), the national road database of Statens vegvesen (daily traffic and share of long vehicles per section). On the 2026 map, a quarter of the long vehicles are assigned to the medium class and the rest to heavy; on the 2024 map, the long-vehicle total is retained and the year’s class model estimates its medium and heavy parts. Oslo also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Municipal streets have no counts. Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.1 % of a town street's traffic (the region's median); electric cars 27.6 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one), a motorway 110.

## Railways

Train and tram counts: Norwegian national timetable from [public-transport.earth](https://data.public-transport.earth/gtfs/no), one busy Wednesday on the 2026 map; on the 2024 map, the archived 2024 national timetable (each operator keeping its own calendar, up to 166 of 366 days covered), averaged over 2024 with missing dates estimated from each operator's covered-day mean. The timetable is passenger-only. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), with the [European night share](/about/data-sources/europe#railways).

## Industry

Wind turbines are OpenStreetMap points, matched within 500 m to turbines of operating wind farms in the [NVE register](https://www.nve.no/) (NLOD 2.0). NVE publishes power per wind farm only; each turbine is assigned the farm's power divided by its number of turbines.

The register adds 3 turbines OpenStreetMap lacks on the 2026 map (2 added on the 2024 map).

## Buildings and terrain

Kartverket has measured building heights, available only as county files behind an order form; they are not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.

## Homes and places

Homes mapped in OpenStreetMap: 60 heat pumps and 4 air conditioners (the region's median) per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.

## Checked against

Silenzi in Quota's quiet mountains ([validation](/about/validation)).

<!-- YEAR SOURCES -->
