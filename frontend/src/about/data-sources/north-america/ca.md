---
title: Canada
intro: Quebec DJMA 2024 traffic counts, timetables for VIA Rail and seven city systems, Transport Canada crossing horns (and through-train counts on the 2024 map), Natural Resources Canada wind turbine database. Uncounted main roads use class defaults on the 2026 map (the building-trip model on the 2024 map); other roads are estimated from the buildings around them.
---

## Roads

Traffic volumes, Quebec only: [Ministère des Transports](https://www.donneesquebec.ca/recherche/dataset/debit-de-circulation) DJMA (annual average daily traffic) for the provincial highway network, 2024 edition, with a published truck percentage per section. An OSM road takes the nearest counted section of the same route number and a similar road class. On the 2026 map, motorcycles are 1 % of vehicles and a fifth of trucks are medium; on the 2024 map, the year’s vehicle model divides the truck total into medium and heavy vehicles and estimates missing motorcycles.

Canada has no national traffic database, and the Ontario, British Columbia and Alberta publications are not loaded; Quebec's counts reach 12 % of the motorway, trunk and primary length. Other motorways, trunks and primaries use the world estimate per lane on the 2026 map and the model of trips made within 1, 5 and 15 km on the 2024 map where enough buildings are mapped ([world defaults](/about/data-preparation)); secondary and tertiary roads follow the trips made within 5 km of them, streets the trips their buildings make.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 0.6 % of a town street's traffic (WHO 2023); electric cars 2.4 % of cars, rolling noise only (IEA 2024); lorries and buses at most 105 km/h in 2026 and 80 km/h in 2024.

Unsigned roads: a primary road 50 km/h in towns, 90 outside (cars drive 76 on a two-way one).

## Railways

Passenger trains come from eight timetables: one busy Wednesday on the 2026 map, and archived 2024 calendars averaged over 2024 on the 2024 map (23 to 122 of 366 days covered by system; missing dates estimated from each operator's covered-day mean).

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

Surface metro sections are included; see the [railway method](/about/data-preparation).

Freight and level-crossing horns: CN and CPKC publish no schedules. On the 2026 map, freight lines take the class default (20 freight trains a day on a main line, 5 on a branch), and the [Transport Canada Grade Crossings Inventory](https://open.canada.ca/data/en/dataset/d0f54727-6c0b-4e5a-aa04-ea1463cf9f4c) (2023 update; contains information licensed under the Open Government Licence – Canada) supplies level-crossing horns. On the 2024 map, non-spur crossings in the same inventory also supply daily train counts along junction-free heavy-rail runs: where a timetable covers the line, its passenger trains are subtracted to leave freight; otherwise three untimetabled passenger operators' crossings (EXO / Réseau de Transport Métropolitain, Alberta Prairie Railway and Prairie Dog Central Railway) count as passenger trains and other carriers' crossings count as freight, while lines without a crossing count keep the class default. Lines without a usage tag use the [unclassified railway default](/about/data-preparation). Public crossings with trains sound on approach; whistling cessation is not in the inventory, so every sounding crossing is labelled cessation-unknown.

## Industry

Wind turbines: [Canadian Wind Turbine Database](https://open.canada.ca/data/en/dataset/79fdad93-9025-49ad-ba16-c26d718cc070) from Natural Resources Canada (Open Government Licence – Canada). Each database turbine is paired with one OSM turbine within 500 m, nearest pairs first, and gives it the database's rated power and hub height.

Power plants: WRI Global Power Plant Database. Steel plants, cement plants and coal mines: Global Energy Monitor. Other factories are OSM industrial areas with an inferred type. The federal National Pollutant Release Inventory is not used.

The register adds 110 turbines OpenStreetMap lacks and removes 8 it calls dismantled.

## Ships

Global Fishing Watch AIS vessel density.

## Buildings and terrain

Terrain from [MRDEM-30](https://open.canada.ca/data/en/dataset/957782bf-847c-4644-a757-e383c0057995) (Natural Resources Canada, Open Government Licence – Canada).

## Homes and places

Homes: heat pumps and air conditioners as Statistics Canada's Households and the Environment Survey counts them. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours close at 02:00.

<!-- YEAR SOURCES -->
