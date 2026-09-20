---
title: Brazil
intro: No traffic counts; federal highways are estimated from surface and concession status in the DNIT 2017 network. No train timetable is loaded. Power plants from the ANEEL SIGA register.
map: { center: [-53, -14], zoom: 4 }
---

## Roads

No traffic counts are loaded: the DNIT national count programme is served from hosts unreachable outside Brazil. The 2017 federal highway network from [DNIT](https://www.dnit.gov.br/), read from a public mirror, gives the location, surface and concession status of BR highways.

An OSM motorway, trunk or primary road within 400 m of a federal highway is estimated as follows:

| Federal highway | Vehicles per day |
|---|---:|
| Paved, under concession | 35,000 |
| Paved, federal | 25,000 |
| Unpaved, planned or under construction | 3,000 |

The estimate is doubled inside Greater São Paulo, Rio de Janeiro, Brasília and Belo Horizonte, and multiplied by 1.4 in 31 other cities: Salvador, Fortaleza, Curitiba, Manaus, Recife, Porto Alegre, Goiânia, Belém, Guarulhos, Campinas, São Luís, Maceió, Natal, Campo Grande, Teresina, João Pessoa, Nova Iguaçu, São Bernardo, Santo André, Osasco, Ribeirão Preto, Uberlândia, Sorocaba, São José dos Campos, Niterói, Contagem, Joinville, Aracaju, Cuiabá, Florianópolis and Vitória. The city boxes are drawn manually. Vehicle mix on these roads: 70% light, 10% medium, 15% heavy and 5% motorcycles in the cities; 60, 10, 25 and 5 outside them.

All other roads, state highways included, use Brazilian class defaults (vehicles per day):

| OSM class | Brazil | São Paulo and Rio |
|---|---:|---:|
| Motorway | 50,000 | 100,000 |
| Trunk | 25,000 | 50,000 |
| Primary | 12,000 | 24,000 |
| Secondary | 5,000 | 10,000 |
| Tertiary | 2,000 | 4,000 |
| Residential | 1,000 | 2,000 |
| Living street | 400 | |

Brasília and Belo Horizonte use the Brazil column except on federal highways. Residential and service streets are then estimated from the buildings each street serves.

## Railways

No timetable is loaded. Railways, including surface metro sections, use the
[world defaults](/about/methodology). These are not tuned to Brazilian ore railways
such as Carajás and Vitória a Minas.

## Industry

Power plants: the ANEEL SIGA register, published as open map layers of thermal, hydro, nuclear and solar plants; plants in operation only. Each OSM industrial area takes the nearest plant within 2 km, in the order thermal, hydro, nuclear, solar.

Mines, refineries and steelworks are OSM industrial areas with a type inferred from name and tags, plus the steel plants, cement plants and coal mines listed by Global Energy Monitor. No open mining register is loaded for Brazil.

Wind turbines are OSM points. The ANEEL per-turbine data (hub height, rated power) is not merged; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Coast and Amazon: Global Fishing Watch AIS vessel density.
