---
title: Ecuador
intro: No traffic counts; state and provincial roads are estimated from their classification in the CONGOPE network layers. No train timetable is loaded. Power plants from Global Energy Monitor.
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

The estimate is doubled inside Quito and Guayaquil, Durán included, and multiplied by 1.4 in 16 other cities: Cuenca, Santo Domingo, Machala, Manta, Portoviejo, Ambato, Loja, Riobamba, Esmeraldas, Ibarra, Latacunga, Milagro, Babahoyo, Quevedo, Lago Agrio and Tulcán.

These roads take an estimated vehicle mix:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Quito, Guayaquil | 67% | 6% | 12% | 15% |
| The 16 cities | 68% | 6% | 12% | 14% |
| Oil roads of Sucumbíos and the SOTE pipeline | 45% | 8% | 37% | 10% |
| Sierra | 55% | 8% | 27% | 10% |
| Oriente | 50% | 8% | 32% | 10% |
| Costa | 60% | 8% | 22% | 10% |

Costa, sierra and oriente are divided by lines of longitude, an approximation.

Vehicle mix on other roads: a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 8.8 % of a town street's traffic (WHO 2023).

Motorways, trunks and primaries not near a mapped network road use the world estimate per lane. Secondary and tertiary roads follow the trips made within 5 km of them, streets the trips their buildings make, else class defaults ([world defaults](/about/data-preparation)).

Unsigned roads: a primary road 50 km/h in towns, 80 outside.

## Railways

No timetable is loaded. Lines that OSM maps as working railways take the class default, up to 80 passenger and 20 freight trains per day; mapped disused or abandoned lines are excluded.

Surface metro sections are included; see the [railway method](/about/data-preparation).

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. The mining and energy regulators publish no loadable register. Mines, oil fields and the Esmeraldas refinery are OSM industrial areas with a type inferred from name and tags.

## Ships

Coast and Galápagos: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
