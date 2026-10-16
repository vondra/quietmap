---
title: Asia
intro: Counted road traffic in Japan, Thailand, Saudi Arabia and Indonesia; timetables in India, Israel, Thailand and the UAE. Most other main roads and railways use class defaults; smaller roads are estimated from the buildings around them.
---

<!-- SUBPAGES -->

## Where Asia has real numbers

Most of Asia publishes no open traffic counts. Road traffic by source:

- Counts: Japan (national road census 2021, on half of the main-road length; the rest takes the census median of its class), Thailand (rural road counts 2024; the Department of Highways' traffic 2021 on national highways), Saudi Arabia (ministry count stations 2024, on a quarter of the main-road length). Indonesia publishes daily traffic for some provincial roads; two thirds of its main-road length takes assigned values.
- Main road network with classes, traffic set by class: China (community dataset), India and the Philippines (official).
- Assigned values by road class and city size: Iran, Iraq, Turkey, Kazakhstan, Uzbekistan.
- Everywhere else: the world default on main roads; smaller roads follow the buildings around them.

Train counts by source:

- Timetable: India, Israel, Thailand, the UAE. No timetable here includes freight.
- Line map with speeds, converted to trains per day: China and India.
- Counts assigned per corridor: Iran, Iraq, Turkey, Kazakhstan, Uzbekistan.
- Everywhere else: class defaults.

## The defaults

Main roads without counts or national estimates use the [world defaults](/about/data-preparation); secondary and tertiary roads follow the trips made by the buildings within 5 km, streets the trips their buildings make.

See the [world railway defaults](/about/data-preparation).

The railway defaults are sized for European lines.

## What is missing across the region

Surface metro sections are included; see the [railway method](/about/data-preparation).

Motorcycles: where counted roads nearby count them, their share; otherwise the country's fleet (WHO 2023): 72 % of a town street's traffic in Vietnam, 64 % in Indonesia, 63 % in India, 48 % in the Philippines, 39 % in Thailand, 4.3 % in China, less on main roads. The network roads of China, India and the Philippines keep their assigned split.

Homes emit their air conditioners: 2.37 per household in Japan, 1.46 in China, 0.29 in South-East Asia, 0.16 in South Asia. Mosques call five times a day at their own prayer times, with country rules: Indonesia adds the recitation before the call, Saudi Arabia the iqama, calling at a third of the power, and Turkey the sala; mosques in China, Tajikistan and Singapore do not call outside, and in Japan, South Korea, Taiwan, Hong Kong and Macao only those with a mapped minaret call, once on Fridays.

No Asian country has a wind turbine register or a national building register loaded: wind turbines are OpenStreetMap's, one without tagged values at 105 dB(A), the fleet median, on a 105 m hub. Building heights come from the global sources described in the [data preparation](/about/data-preparation).

Ships along Asian coasts come from Global Fishing Watch, which has no class for yachts and pleasure boats; leisure traffic is missing. The Mediterranean and Black Sea coasts of Turkey, Syria, Lebanon, Israel and Georgia carry EMODnet 2024 instead, pleasure boats included; the Caspian Sea has almost no ship data.
