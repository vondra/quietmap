---
title: Africa
intro: Data sources, defaults and known gaps in the noise map of Africa.
map: { center: [20, 5], zoom: 3 }
---

## Roads

No traffic counts are available for any African country. Roads come from OpenStreetMap
and traffic is estimated per road class.

Nine countries have their own estimates: Algeria, DR Congo, Egypt, Ethiopia, Kenya,
Morocco, Nigeria, Sudan and Tanzania. Each has volumes per road class and multipliers for
listed cities; the tables are on the country pages.

All other countries use the [world defaults](/about/methodology). Local streets carry the
trips their buildings make. The medium and heavy vehicles follow the counted roads of the
same class elsewhere in the world; motorcycles come from national fleets (WHO 2023) or local
estimates: 13 % of traffic in Egypt, 20 % in Kenya, 30 % in Nigeria.

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

Source: recorded flights. A year of [adsb.lol](https://adsb.lol/), September 2025 to
August 2026, with the flights only [ADSBExchange](https://www.adsbexchange.com/) received
added from 11 sample days. Both rely on volunteer ground receivers, which are sparse in much
of Africa; flights nobody records are missing, so aircraft noise is understated there —
near Cairo, Casablanca, Accra and Lagos airports by 5 to 20 dB.

## Ships

Source: AIS vessel hours. [EMODnet](https://emodnet.ec.europa.eu/en/human-activities)
2024 where its European grid reaches the North African coast,
[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) elsewhere. Vessels
without an AIS transmitter, including most small fishing boats, are not covered.

## Buildings

Footprints come from OpenStreetMap and [Overture](https://overturemaps.org/), heights from
mapped tags and the typical height for the footprint size. Unmapped buildings do not
screen sound; where footprint coverage is poor, the map is too loud behind the first row of houses.

## Homes and places

Homes south of the Sahara emit the air conditioners of South Africa's survey, the region's
only one: 6 per 100 households; North Africa, with no survey in the region, takes the world
value of 16. Mosques call five times a day at their own prayer times; Egypt adds the iqama,
Rwanda has no dawn call.

Model description: [methodology](/about/methodology).
