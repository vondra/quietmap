---
title: Canada
intro: Quebec DJMA 2024 traffic counts, timetables for VIA Rail and seven city systems, Transport Canada crossing horns, Natural Resources Canada wind turbine database. Roads outside Quebec and freight use class defaults.
map: { center: [-95.0, 60.0], zoom: 3 }
---

## Roads

Traffic volumes, Quebec only: [Ministère des Transports](https://www.donneesquebec.ca/recherche/dataset/debit-de-circulation) DJMA (annual average daily traffic) for the provincial highway network, 2024 edition, with a published truck percentage per section. An OSM road takes the nearest counted section of the same route number and a similar road class. Motorcycles are 1% of vehicles.

Canada has no national traffic database, and the Ontario, British Columbia and Alberta publications are not loaded. Roads outside Quebec use the [world defaults](/about/methodology).

## Railways

Passenger trains come from eight timetables.

| Operator | What it covers | Feed |
|---|---|---|
| VIA Rail | Intercity trains | [viarail.ca](https://www.viarail.ca/sites/all/files/gtfs/viarail.zip) |
| GO Transit | Toronto and Hamilton commuter rail | [metrolinx.com](https://www.metrolinx.com/) |
| TTC | Toronto subway and streetcars | [open.toronto.ca](https://open.toronto.ca/) |
| STM | Montreal metro | [stm.info](http://www.stm.info/sites/default/files/gtfs/gtfs_stm.zip) |
| TransLink | Vancouver SkyTrain | [gtfs-static.translink.ca](https://gtfs-static.translink.ca/gtfs/google_transit.zip) |
| OC Transpo | Ottawa O-Train | OC Transpo open data |
| Calgary Transit | CTrain | [data.calgary.ca](https://data.calgary.ca/) |
| Edmonton ETS | LRT | [gtfs.edmonton.ca](https://gtfs.edmonton.ca/) |

Surface metro sections are included; see the [railway method](/about/methodology).

Freight is not covered: CN and CPKC publish no schedules. Their lines default to 20 freight trains per day on a main line and 5 on a branch. Lines without a usage tag use the [unclassified railway default](/about/methodology).

Level-crossing horns: [Transport Canada Grade Crossings Inventory](https://open.canada.ca/data/en/dataset/d0f54727-6c0b-4e5a-aa04-ea1463cf9f4c), 2023 update (contains information licensed under the Open Government Licence – Canada). Public crossings with trains sound on approach; whistling cessation is not in the inventory, so every sounding crossing is labelled cessation-unknown. Rail yards mapped in OpenStreetMap emit as round-the-clock industrial sites.

## Industry

Wind turbines: [Canadian Wind Turbine Database](https://open.canada.ca/data/en/dataset/79fdad93-9025-49ad-ba16-c26d718cc070) from Natural Resources Canada. An OSM turbine without specs takes rated power and hub height from the nearest database turbine within 500 m.

Power plants: WRI Global Power Plant Database. Steel plants, cement plants and coal mines: Global Energy Monitor. Other factories are OSM industrial areas with an inferred type. The federal National Pollutant Release Inventory is not used.

## Ships

Global Fishing Watch AIS vessel density.
