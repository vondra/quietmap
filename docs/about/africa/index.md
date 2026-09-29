---
title: Africa
intro: Data sources, defaults and known gaps in the noise map of Africa.
map: { center: [20, 5], zoom: 3 }
---

## Roads

No traffic counts are available for any African country. Roads come from OpenStreetMap
and traffic is estimated per road class.

Nine countries have their own estimates: Algeria, DR Congo, Egypt, Ethiopia, Kenya,
Morocco, Nigeria, Sudan and Tanzania. Each has volumes per road class, multipliers for
listed cities and a higher truck share along its main freight road; the tables are on the
country pages.

All other countries use the [world defaults](/about/methodology). The vehicle mix is the world default: on a motorway, 72%
cars, 8% medium vehicles, 19% heavy trucks and 1% motorcycles. The motorcycle share is
too low for most African cities.

Local streets are adjusted by the buildings along them: 1.5 vehicle trips per dwelling
per day across Africa (2 in Egypt) against 4 in the world default, and 10% motorcycles
(18% in Egypt, 20% in Kenya, 30% in Nigeria).

## Railways

No African country has a timetable feed. The nine countries above use per-line estimates
based on services reported by the operators. Elsewhere every track takes the world
defaults described in the [railway method](/about/methodology). These can
overstate lightly used lines. Surface metro sections are included; underground sections
do not emit outdoor noise.

## Industry

Industrial areas come from OpenStreetMap. Site types come from a register where one
exists: operating power plants from Global Energy Monitor and the
[Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase),
plus Global Energy Monitor's trackers for
[steel](https://globalenergymonitor.org/projects/global-iron-and-steel-tracker/),
[cement](https://globalenergymonitor.org/projects/global-cement-and-concrete-tracker/)
and [coal mines](https://globalenergymonitor.org/projects/global-coal-mine-tracker/).
South Africa adds Eskom's station list. Oil fields, refineries, metal mines and food
plants have no register; they are classified from their OpenStreetMap name, otherwise as
generic industry. No wind turbine register is available for any African country.

## Aircraft

Source: recorded flights. 12 sample days of airline traffic from
[ADSBExchange](https://www.adsbexchange.com/), one per month, and 364 days of adsb.lol
for small planes and helicopters, 2 September 2025 to 1 September 2026. Both rely on
volunteer ground receivers, which are sparse in much of Africa; flights nobody records
are missing, so aircraft noise is understated there.

## Ships

Source: AIS vessel hours. [EMODnet](https://emodnet.ec.europa.eu/en/human-activities)
2024 where its European grid reaches the North African coast,
[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) elsewhere. Vessels
without an AIS transmitter, including most small fishing boats, are not covered.

## Buildings

Footprints come from OpenStreetMap and [Overture](https://overturemaps.org/), heights from
mapped tags and the typical height for the footprint size. Unmapped buildings do not
screen sound; where footprint coverage is poor, the map is too loud behind the first row of houses.

Model description: [methodology](/about/methodology).
