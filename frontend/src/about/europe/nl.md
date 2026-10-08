---
title: Netherlands
intro: National motorway counts (INWEVA), modelled city traffic in Amsterdam, national passenger timetable. Other roads are estimated from the buildings around them. Rail freight from the national train-kilometres.
map: { center: [5.3, 52.2], zoom: 7 }
---

## Roads

Motorways and national roads: [INWEVA](https://www.nationaalgeoregister.nl/geonetwork/srv/api/records/93e99016-9b53-45d6-8b3c-fc9bf8086256), the 2024 Rijkswaterstaat section intensities (CC0), with cars, vans and two truck classes per direction from the loop detectors.

A motorway takes the counted section of its number within 50 m running along it; only sections measured at their own loops count, derived neighbours keep the prior. The detectors cannot see motorcycles, so 1 % of each total is assigned to them.

Amsterdam: the 2025 Amsterdam file of the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). These values come from the city's traffic model, not from counters, and are shown as modelled.

Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 1.1 % of a town street's traffic (the region's median); electric cars 6.0 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (traffic drives 68 on a two-way one), an expressway 100 (84 on a two-way one), a motorway 130.

## Railways

Train, tram and metro counts: Dutch national timetable from [public-transport.earth](https://data.public-transport.earth/gtfs/nl), one busy Wednesday. The timetable is passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Industry

Wind turbines: OpenStreetMap's standing turbines with RIVM's register's power and height.

## Buildings and terrain

Buildings take their measured height from [3DBAG](https://3dbag.nl/) first, then OpenStreetMap tags, floor counts, Overture heights and the typical height for the footprint size. Noise walls along state roads come from the Rijkswaterstaat [barrier inventory](https://data.overheid.nl/dataset/15743-geluidswerende-voorzieningen--gwv-) (CC0).

Terrain from AHN4 (laser survey).

## Homes and places

Homes mapped in OpenStreetMap: 7 heat pumps and 12 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.
