---
title: Czech Republic
intro: National road census, city counts in Prague and Brno, passenger timetable (CIS JR), measured building heights in Prague. Rail freight from the national train-kilometres.
map: { center: [15.5, 49.8], zoom: 7 }
---

## Roads

- Motorways and classified roads: [Road and Motorway Directorate (ŘSD)](https://scitani.rsd.cz/) national census 2020 (cars, vans, trucks, buses, motorcycles).
- Prague: [TSK Praha](https://tsk-praha.cz/) traffic intensities 2025.
- Brno: [city detectors](https://data.brno.cz/datasets/intenzita-dopravy-intenzita-vozidel-vehicle-traffic-intensity)
  and the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

City counts from 2020 and 2021 are excluded as pandemic years. Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 4.2 % of a town street's traffic (WHO 2023).

Unsigned roads: a primary road 50 km/h in towns, 90 outside (traffic drives 76 on a two-way one), an expressway 110 (93 on a two-way one), a motorway 130.

## Railways

Train counts are derived from the national timetable
([CIS JR](https://portal.cisjr.cz/pub/draha/celostatni/szdc/2026/)), routed along
OpenStreetMap tracks.

The public timetable is passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors. Lines without any
scheduled service default to 2 passenger and 1 freight train per day.

## Industry

Industrial sites are OpenStreetMap areas, classified using the European register
[E-PRTR](https://industry.eea.europa.eu/) and global lists of power plants, steelworks,
cement plants and coal mines. The Czech [IRZ register](https://www.irz.cz/) is not loaded
yet.

## Buildings and terrain

Prague: measured height for every building, from the
[IPR Praha relative building heights](https://opendata.geoportalpraha.cz/maps/ad9aca20e9c042d2b52eb31ff18961b6)
(one-metre aerial survey).

Elsewhere: floor count and building use from the
[RÚIAN register](https://www.cuzk.cz/Uvod/Produkty-a-sluzby/RUIAN/) where a register
point lies within 30 m of the building; otherwise the typical height for the footprint size.

Terrain from ČÚZK's laser survey DMR 5G.

## Homes and places

Homes mapped in OpenStreetMap: 6 heat pumps and 16 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.

## Checked against

Prague airport's monitors.
