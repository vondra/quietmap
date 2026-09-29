---
title: Peru
intro: Measured daily traffic on part of the MTC Red Vial Nacional 2024, estimates from route classification on the rest. Open-pit, tailings and leach pad outlines for major mines. No train timetable is loaded.
map: { center: [-76, -10], zoom: 5 }
---

## Roads

Road data: the 2024 Red Vial Nacional of MTC Provías, read from a [community mirror](https://services6.arcgis.com/G8JFnqCHKQ9vb8YW/) because the ministry's servers are unreachable from abroad. Some sections carry a measured daily traffic (IMD); most do not. The departmental road network has no traffic data.

An OSM motorway, trunk or primary road takes the nearest network road within 500 m and uses its IMD where present. Otherwise the volume is estimated from the classification:

| Road in the network | Vehicles per day |
|---|---:|
| Concession on the Longitudinal de la Costa (Panamericana) | 20,000 |
| Other concession | 10,000 |
| Paved Longitudinal de la Costa | 12,000 |
| Paved Longitudinal de la Sierra or de la Selva | 6,000 |
| Paved transversal | 4,000 |
| Paved branch, variant or departmental road | 2,500 |
| Other paved | 2,000 |
| Unpaved | 1,200 |

Measured volumes are used as published. Estimates are doubled inside Lima and Callao and multiplied by 1.4 in 24 other cities: Arequipa, Trujillo, Chiclayo, Piura, Iquitos, Cusco, Chimbote, Huancayo, Tacna, Juliaca, Ica, Cajamarca, Pucallpa, Sullana, Ayacucho, Chincha Alta, Huánuco, Tarapoto, Puno, Tumbes, Huaraz, Jaén, Huacho and Pisco. The city boxes are drawn manually.

The vehicle mix is an estimate everywhere, including where the total is measured:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Lima | 65% | 6% | 14% | 15% |
| The 24 cities | 68% | 6% | 12% | 14% |
| Mining regions | 45% | 8% | 38% | 9% |
| Sierra | 50% | 10% | 30% | 10% |
| Costa and selva | 60% | 8% | 22% | 10% |

The mining regions are four manually drawn boxes: the south around Arequipa, Moquegua and Tacna; the central Andes around Ancash; Cajamarca; and Apurímac with southern Cusco. Costa, sierra and selva are divided by approximate lines of longitude.

Secondary and smaller roads use world defaults.

## Railways

No timetable is loaded; PeruRail and the Southern Peru copper railway publish no loadable schedule. All lines use the [world railway defaults](/about/methodology).

Lima Metro Line 1 takes the light rail default of 80 trains per day where OSM tags it as light rail. Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

Mining: a community mirror of the supervised mining data gives outlines of open pits, tailings dams, heap leach pads, waste rock dumps, major concessions and active mining units. An OSM industrial area inside one of them is classed as metal ore mining. Active deposits from the INGEMMET register are attached to the nearest OSM industrial area within 2 km.

Power plants: Global Energy Monitor power tracker, operating plants only, with the same 2 km rule.

These sources only classify areas already mapped in OSM; a pit not mapped as an industrial area is not a noise source.

## Ships

Global Fishing Watch AIS vessel density.
