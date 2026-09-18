---
title: Portugal
intro: City traffic counts in Lisbon, four timetables for trains and light rail. Other roads use class defaults. Fertagus, Lisbon trams and rail freight not covered.
map: { center: [-8.0, 39.5], zoom: 7 }
---

## Roads

Lisbon: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Elsewhere class defaults apply, with motorway, trunk and primary scaled by 1.175 (vehicles per kilometre of road); the motorway default is 35,250 vehicles per day.

## Railways

Train counts, one busy Wednesday:

| Feed | What it covers |
|---|---|
| [CP Comboios de Portugal](https://publico.cp.pt/gtfs/gtfs.zip) | National and commuter trains |
| Metro do Porto | Porto light rail |
| [Metro Sul do Tejo](https://mts.pt/imt/MTS-20240129.zip) | Almada and Seixal light rail |
| [Carris Metropolitana](https://api.carrismetropolitana.pt/v2/gtfs) | Lisbon area. Mostly buses; not used |

Fertagus trains over the 25 de Abril bridge and the Lisbon trams have no feed loaded. Freight is not covered: the timetables are passenger-only.
