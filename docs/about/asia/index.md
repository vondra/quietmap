---
title: Asia
intro: Counted road traffic in Japan, Thailand, Saudi Arabia and Indonesia; timetables in India, Israel, Thailand and the UAE. Most other roads and railways use class defaults.
map: { center: [100, 35], zoom: 3 }
---

## Where Asia has real numbers

Most of Asia publishes no open traffic counts. Road traffic by source:

- Counts: Japan (national road census 2021), Thailand (rural road counts 2024), Saudi Arabia (ministry count stations 2024). Indonesia publishes daily traffic for many regional roads.
- Main road network with classes, traffic set by class: China (community dataset), India and the Philippines (official).
- Assigned values by road class and city size: Iran, Iraq, Turkey, Kazakhstan, Uzbekistan.
- Everywhere else: the world default.

Train counts by source:

- Timetable: India, Israel, Thailand, the UAE. No timetable here includes freight.
- Line map with speeds, converted to trains per day: China and India.
- Counts assigned per corridor: Iran, Iraq, Turkey, Kazakhstan, Uzbekistan.
- Everywhere else: class defaults.

## The defaults

Roads without a count get the values below. For motorways, trunk and primary roads the value is scaled by 0.7 to 1.3 per country, according to vehicles per kilometre of road. Each country page shows its own numbers.

| Road class | Vehicles per day |
|---|---:|
| Motorway | 30,000 |
| Trunk | 15,000 |
| Primary | 9,000 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |

Railways without a timetable get these trains per day:

| Line | Passenger | Freight |
|---|---:|---:|
| Main line | 80 | 20 |
| Branch | 30 | 5 |
| Industrial siding | 0 | 15 |
| Tram | 120 | 0 |
| Light rail | 80 | 0 |
| Narrow gauge | 10 | 0 |

The railway defaults are sized for European lines.

## What is missing across the region

Metro lines tagged as subway in OpenStreetMap are not extracted. This excludes most of the metros of Tokyo, Seoul, Singapore, Taipei, Bangkok, Delhi and Dubai. Chinese metro lines tagged as ordinary rail are on the map.

Motorcycles: where a national source gives a split, it is used: up to 60% of traffic in Jakarta, 50% in Manila, 40% in the largest Indian cities. On local streets elsewhere the share comes from WHO registration figures, or a flat 15% where WHO has none.

No Asian country has a wind turbine register or a national building register loaded. Building heights come from the global sources described in the [methodology](/about/methodology).

Ships along Asian coasts come from Global Fishing Watch, which has no class for yachts and pleasure boats; leisure traffic is missing.
