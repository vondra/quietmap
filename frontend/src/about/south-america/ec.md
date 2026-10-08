---
title: Ecuador
intro: No traffic counts; state and provincial roads are estimated from their classification in the CONGOPE network layers. No train timetable is loaded. Power plants from Global Energy Monitor.
map: { center: [-78, -1.5], zoom: 6 }
---

## Roads

Ecuador publishes no machine-readable traffic counts, and the transport ministry's servers are unreachable from abroad. Road classification comes from network layers mirrored by CONGOPE, the consortium of provincial governments: the [Red Vial Estatal](https://services6.arcgis.com/pYn2F4v1aESZqj1u/) with arterial and collector classes, and a wider layer with the provincial roads.

An OSM motorway, trunk or primary road within 400 m of one of those roads is estimated as follows:

| Road in the network | Vehicles per day |
|---|---:|
| State road, arterial | 15,000 |
| State road, collector | 6,000 |
| State road, class not stated | 10,000 |
| State road, unpaved | 2,000 |
| Provincial road, paved | 4,000 |
| Provincial road, unpaved | 1,200 |

The estimate is doubled inside Quito and Guayaquil and multiplied by 1.4 in 17 other cities: Cuenca, Santo Domingo, Machala, Durán, Manta, Portoviejo, Ambato, Loja, Riobamba, Esmeraldas, Ibarra, Latacunga, Milagro, Babahoyo, Quevedo, Lago Agrio and Tulcán.

The vehicle mix is an estimate:

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 8.8 % of a town street's traffic (WHO 2023).

Costa, sierra and oriente are divided by lines of longitude, an approximation.

Motorways, trunks and primaries not near a mapped network road use the world estimate per lane. Secondary and smaller roads use class defaults ([world defaults](/about/methodology)).

Unsigned roads: a primary road 50 km/h in towns, 80 outside.

## Railways

No timetable is loaded. Lines that OSM maps as working railways take the class default, up to 80 passenger and 20 freight trains per day; mapped disused or abandoned lines are excluded.

Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. The mining and energy regulators publish no loadable register. Mines, oil fields and the Esmeraldas refinery are OSM industrial areas with a type inferred from name and tags.

## Ships

Coast and Galápagos: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
