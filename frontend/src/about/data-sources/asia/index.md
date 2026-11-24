---
title: Asia
intro: Counted road traffic in Japan, Thailand, Saudi Arabia and Indonesia (by year); timetables in India, Israel, Thailand and the UAE (by year). Most other main roads use network or country estimates on the 2026 map and the global road model on the 2024 map; smaller roads are estimated from the buildings around them, and railways without a timetable, network map or corridor count use class defaults.
---

<!-- SUBPAGES -->

## Where Asia has real numbers

Most of Asia publishes no open traffic counts. Road traffic by source:

- Counts: Japan (national road census 2021, on half of the main-road length; the rest takes the census median of its class), Thailand (rural road counts 2024; the Department of Highways' highway traffic table on national highways, labelled 2021 in the archived file and 2018 by the publisher), Saudi Arabia (ministry count stations, 2024, on a quarter of the main-road length on the 2026 map; located stations on the 2024 map), and Indonesia (on the 2026 map, Bina Marga daily traffic on some provincial roads while two thirds of main-road length takes assigned values; on the 2024 map, archived 2016–2019 Bina Marga provincial counts matched with national-road exclusions while assigned toll and national layers are excluded).
- Main road network with classes, traffic set by class (2026 map; excluded on the 2024 map): China (community dataset), India and the Philippines (official).
- Assigned values by road class and city size (2026 map; excluded on the 2024 map): Iran, Iraq, Turkey, Kazakhstan, Uzbekistan (and Thailand's 18 unmeasured routes and Saudi Arabia's Riyadh street map and transport atlas).
- Everywhere else (and all countries without dated counts on the 2024 map): the [global road model](/about/data-preparation) on main roads; smaller roads follow the buildings around them.

Train counts by source:

- Timetable: Thailand and Israel (both map years), India and the UAE (2026 map; applied as declared fallback vintages on the 2024 map where no 2024 archive exists). No timetable here includes freight.
- Line map with speeds, converted to trains per day: China (both map years, with mainland metros filtered by opening and closing dates on the 2024 map) and India (both map years).
- Counts assigned per corridor (2026 table, also applied as a declared fallback vintage on the 2024 map): Iran, Iraq, Turkey, Kazakhstan, Uzbekistan.
- Everywhere else: class defaults.

## The defaults

Main roads without counts or 2026 national estimates use the [global road model](/about/data-preparation); secondary and tertiary roads follow the trips made by the buildings within 5 km, streets the trips their buildings make.

See the [world railway defaults](/about/data-preparation).

The railway defaults are sized for European lines.

## What is missing across the region

Surface metro sections are included; see the [railway method](/about/data-preparation).

Motorcycles: where counted roads nearby count them, their share; otherwise the country's fleet (WHO 2023): 72 % of a town street's traffic in Vietnam, 64 % in Indonesia, 63 % in India, 48 % in the Philippines, 39 % in Thailand, 4.3 % in China, less on main roads. On the 2026 map, the network roads of China, India and the Philippines keep their assigned vehicle split.

Homes emit their air conditioners: 2.37 per household in Japan, 1.46 in China, 0.29 in South-East Asia, 0.16 in South Asia. Mosques call five times a day at their own prayer times, with country rules: Indonesia adds the recitation before the call, Saudi Arabia the iqama, calling at a third of the power, and Turkey the sala; mosques in China, Tajikistan and Singapore do not call outside, and in Japan, South Korea, Taiwan, Hong Kong and Macao only those with a mapped minaret call, once on Fridays.

No Asian country has a wind turbine register loaded: wind turbines are OpenStreetMap's, one without tagged values at 105 dB(A), the fleet median, on a 105 m hub. Building heights use Japan's measured PLATEAU survey in Tokyo, Osaka and Sapporo and the global sources described in [data preparation](/about/data-preparation) elsewhere.

Ships along Asian coasts come from Global Fishing Watch, which has no separate class for yachts and pleasure boats: boats without AIS are missing, while pleasure craft with AIS count as large ships on the 2026 map and are not separated from work boats on the 2024 map. The Mediterranean and Black Sea coasts of Turkey, Syria, Lebanon, Israel and Georgia carry EMODnet 2024 instead, pleasure boats included; the Caspian Sea has almost no ship data.

<!-- YEAR SOURCES -->
