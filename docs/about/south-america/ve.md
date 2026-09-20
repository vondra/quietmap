---
title: Venezuela
intro: No traffic counts; roads are estimated from the road-type layer of the Venezuela360 archive, which ends around 2019. Power plants, oil wells and substations from the same archive. No train timetable is loaded.
map: { center: [-66, 8], zoom: 6 }
---

## Roads

Almost no Venezuelan government data server is reachable. Road data comes from the community mirror [Venezuela360](https://services6.arcgis.com/lpJCO3ug8HhNiEOV/), which preserves the national planning dataset as of about 2013 to 2019. Its road layer gives a type for every road and no traffic.

An OSM motorway, trunk or primary road within 400 m of a road in that layer is estimated from the type:

| Road type in the archive | Vehicles per day |
|---|---:|
| Autopista | 22,000 |
| Paved, more than two lanes | 12,000 |
| Paved | 8,000 |
| Gravel, more than two lanes | 5,000 |
| Gravel | 3,000 |
| Camino | 1,800 |
| Dirt | 1,500 |
| Camino or path | 1,000 |
| Path | 500 |
| Not stated | 2,000 |

The estimate is doubled inside Caracas, Maracaibo, Valencia and Barquisimeto and multiplied by 1.4 in 19 other cities: Ciudad Guayana, Maracay, Maturín, Barcelona, Puerto La Cruz, San Cristóbal, Cumaná, Mérida, Ciudad Bolívar, Cabimas, Coro, Los Teques, Guarenas, Guanare, Valera, Punto Fijo, Acarigua, Barinas and San Felipe.

Vehicle mix: 60% light, 5% medium, 10% heavy and 25% motorcycles in the four big cities; 62, 6, 12 and 20 in the 19 cities; 58, 8, 22 and 12 elsewhere.

The estimates describe the network as archived, not current traffic; no data on changes since is available.

Motorways, trunks and primaries not in the archive use the world estimate per lane. Secondary and smaller roads use class defaults ([world defaults](/about/methodology)).

## Railways

No timetable is loaded. Lines use the [world railway defaults](/about/methodology). This applies to the Caracas to Cúa commuter line and the Ferrominera ore railway alike. Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

Sources, from the Venezuela360 archive and Global Energy Monitor:

- power plants the archive lists with current output above zero; most thermal plants are listed at zero and are excluded
- operating plants from the Global Energy Monitor power tracker, which adds the Caroní dams
- oil processing plants
- oil wells in the Orinoco belt and the Maracaibo basin, classed as oil extraction
- electrical substations

Each record is attached to an OSM industrial area within 2 km. The refineries at Paraguaná, El Palito and Puerto La Cruz and the steel and aluminium works at Ciudad Guayana are not in these layers; they are OSM industrial areas with a type inferred from name and tags, modelled as operating normally regardless of their current state.

## Ships

Coast and Lake Maracaibo: Global Fishing Watch AIS vessel density.
