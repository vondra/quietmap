# quietmap.org

**Make noise visible. Make quiet possible.**

I'm building this map for two reasons. The first is finding quiet places, free of human-made
noise. The second is making noise visible: what you can't measure, you can't manage, and once
it's visible, things can start to change. In the long run I'd like a map for every year, so that a
city, or a single street, can see whether it is getting quieter.

Only human-made sound is in the numbers: roads and car parks, railways and trams, aircraft and
airports, ships, factories, power plants and wind turbines, the heat pumps and air conditioners of
homes and the hum of other buildings, church bells, calls to prayer and people outside bars and
restaurants. Wind, water and birds are not.

## How it works

1. **Every source gets its sound power from data.** Traffic counts where someone counted them;
   where nobody did, the traffic the buildings along the streets make, and on half of the world's
   main roads a fixed estimate per road class. Timetables for trains, a year of recorded flights,
   registries for factories, power plants and wind turbines, and OpenStreetMap for where everything
   is.
2. **Your click computes the rest.** The server follows the sound of every source within 12 km
   (aircraft 16 km) to a point 4 m above the ground where you clicked (on a building, its loudest
   façade): the distance, the air, the ground, hills, buildings and noise barriers on the way. On
   the ground by the EU method CNOSSOS-EU, in the air by ECAC Doc 29. Nothing is looked up in a
   precomputed map: every point on Earth gets its own answer.
3. **You see what the level is made of.** The panel shows the level, Lden (the EU's
   day-evening-night level) and the loudness in sone, then the sources, loudest first, each with
   the data it came from and how it is heard: a steady hum, so many cars an hour, a train every
   so many minutes. The detailed calculation shows the pieces of road, track and flight path that
   count most, and what the distance, the ground, the buildings and the air took off on each path.

![A click on náměstí Míru in Prague: the level and its sources, the loudest road opened with its data and drawn on the map](/about/images/click.jpg)
![The same click's detailed calculation: each period, each layer, the weather, and the paths from the loudest pieces drawn on the map](/about/images/calculation.jpg)

## How we check it

Before a version goes live it is compared with public measuring stations. Model minus
measurement: the median over the stations, and the difference half of the stations stay within.

| Stations | n | Median | Half within |
|---|---:|---:|---:|
| Paris near its airports, Bruitparif, Lden 2024 | 22 | +0.0 dB | 1.3 dB |
| Paris streets, Lden 2025 | 16 | −2.6 dB | 2.6 dB |
| Barcelona, Lden 2025 | 154 | −1.7 dB | 4.9 dB |
| Madrid, Lden 2025 | 31 | +2.4 dB | 3.2 dB |
| Dublin, Lden 2025 | 14 | −4.4 dB | 5.2 dB |
| Poland's roads, 06–22, 2018–2024 | 6,616 | +0.4 dB | 3.0 dB |
| German railways, 06–22 and 22–06, 2023 | 19 | −0.1 and −3.0 dB | 1.0 and 3.1 dB |
| Prague airport, aircraft, 06–22 and 22–06, January 2026 | 14 | −0.2 and +1.1 dB | 1.6 and 2.2 dB |
| Minneapolis–St Paul airport, aircraft, DNL 2025–26 | 39 | +0.3 dB | 1.1 dB |
| Zurich airport, aircraft, 06–22, 2024 | 4 | −6.1 dB | 6.1 dB |
| Accra, 24 h, 2019 | 146 | −9.7 dB | 9.7 dB |

Rail emission is fitted on the German stations, so that row is no independent test. All of these
stand in loud places. The quiet places the map is for are hardly checked yet: the only set so far,
Silenzi in Quota's 73 single-day samples in quiet mountains (54 in the Italian Alps, 19 in Norway,
Britain and Finland, 2022–2024), measures everything heard, nature included, and reads 12.9 dB
above the model, as it should; but at 15 of the points the model is louder than everything
measured, each time because of a road (by up to 18 dB beside the road to the Karersee).

## What is known where

The data differ by country: counted traffic or estimates, timetables, terrain surveys, registers.
Each region's page says what holds across it, each country's page what the map knows there:
[Africa](/about/africa) · [Asia](/about/asia) · [Europe](/about/europe) ·
[North America](/about/north-america) · [Oceania](/about/oceania) ·
[South America](/about/south-america).

## Who builds it

quietmap.org is built by one person working with AI coding agents: **Claude** as lead developer,
**Codex** and **Grok** as reviewers, and a rotating cast of other models, Gemini and open-source
ones among them, as second opinions. Development started in June 2025 on Opus 4; every major
Opus, GPT and Gemini release since has been tried on this codebase. Progress accelerated markedly
with [OpenClaw](https://openclaw.ai/) and Opus 4.6, and it has kept getting better since.

Some of it was built while hiking the forests of La Palma: changes discussed with the models over
Telegram through OpenClaw, on a mobile signal that kept cutting out. It worked surprisingly well.
Fitting, for a map about quiet.

quietmap.org is an internal project of [Miton](https://www.miton.cz/en/). The
[product code is open source](https://github.com/vondra/quietmap), and the computations are
reproducible from public data.

## Contact

- [info@quietmap.org](mailto:info@quietmap.org): wrong numbers on your street (with the address),
  data you can share, anything else.
- [status.quietmap.org](https://status.quietmap.org): whether the map and the click are up.
