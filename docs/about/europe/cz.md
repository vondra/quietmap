---
title: Czech Republic
intro: National road census, city counts in Prague and Brno, passenger timetable (CIS JR), measured building heights in Prague. Rail freight not covered.
map: { center: [15.5, 49.8], zoom: 7 }
---

## Roads

- Motorways and classified roads: [Road and Motorway Directorate (ŘSD)](https://scitani.rsd.cz/) national census 2020 (cars, vans, trucks, buses, motorcycles).
- Prague: [TSK Praha](https://tsk-praha.cz/) traffic intensities 2025.
- Brno: [city detectors](https://data.brno.cz/datasets/intenzita-dopravy-intenzita-vozidel-vehicle-traffic-intensity)
  and the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

City counts from 2020 and 2021 are excluded as pandemic years. Uncounted streets use the
class default, adjusted for the buildings they serve and the counted roads they connect to.

## Railways

Train counts are derived from the national timetable
([CIS JR](https://portal.cisjr.cz/pub/draha/celostatni/szdc/2026/)), routed along
OpenStreetMap tracks.

Freight is not covered: the public timetable is passenger-only. Lines without any
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
