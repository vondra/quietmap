---
title: Data preparation
intro: How loud each road, railway, flight, ship, factory and building is, and where the numbers come from.
---

<!-- SECTIONS -->

Every source of noise on the map has a sound power: how much noise it gives off by day, evening and
night. Where somebody counted, the count is used. London's Marylebone Road carries the Department for
Transport's count of 42,783 vehicles a day. Where nobody counted, an estimate stands in, and the
[country pages](/about/data-sources) say which is which.

## Roads

A road gets louder with more vehicles, more lorries and higher speed: twice the traffic adds about
3 dB. Speed is the signed limit, else the country's legal limit for the road's class.

Traffic is where the map is least sure. Of the world's 5.8 million km of motorways, trunk and
primary roads, 31 % have official traffic figures. Counts cover 18 %, such as the 148,831 vehicles a
day the [<abbr title="Highway Performance Monitoring System, the US federal road traffic database">HPMS</abbr>](https://www.fhwa.dot.gov/policyinformation/hpms.cfm)
gives New York's Brooklyn–Queens Expressway (2022). National estimates cover the rest of them, in
China, India and a few other countries.

Another 14 % take their traffic from a model trained on the counted roads. It predicts a road's
traffic from the trips the buildings within 1, 5 and 15 km make. The remaining 54 % have only an
average for their road class in their country (October 2026). A side street nobody counted carries
the trips its own buildings make.

<details>
<summary>How a road's sound power is computed</summary>

The emission follows <abbr title="Common Noise Assessment Methods in Europe, the EU's method for road, railway and industrial noise">CNOSSOS-EU</abbr>
from the traffic, the vehicle mix, the speed, the surface and the gradient, with acceleration at
junctions and the place's yearly air temperature.

- **Traffic without a count.** Secondary and tertiary roads with only a class estimate follow a
  model of the same kind, fitted on 13 European countries; where a country's default stands
  instead (Russia, Turkey, Japan, Iran, Ukraine), it is kept. A street carries the trips its
  buildings make, OpenStreetMap's and Overture's, routed along the street network to the nearest
  main road.
- **A count beside its road.** A count that a dataset put on a service road, side lane, street or
  ramp beside the counted road, within 10 % of that road's own and within 30 m of it, or on a side
  lane of the road's name with its very count, belongs to the road: the way takes the model of its
  class (a ramp the major roads'), and where none gives it a count it carries only its mapped buses.
  At London's Baker Street the service road beside Marylebone Road carried the road's 43,031
  vehicles a day a second time. A tunnel, which emits nothing and keeps no traffic in the data,
  counts as its road at its portals: in Brno, Kohoutova over the Husovice tunnel carried the
  tunnel's 44,000 vehicles a day; in Madrid the avenues over the buried M-30 carried the
  motorway's.
- **Vehicle mix.** A count's own mix where it gives one; otherwise a main road takes the counted
  median of its class, a local street 1 % medium and 2 % heavy vehicles, a US road its state's mix
  ([<abbr title="the US Federal Highway Administration">FHWA</abbr>](https://www.fhwa.dot.gov/policyinformation/statistics.cfm),
  table VM-4). Day, evening and night shares are measured in the United States, Japan, Germany, the
  Netherlands, Great Britain, Spain and Thailand, elsewhere taken from the region's median or a
  default.
- **Electric cars and motorcycles.** Battery-electric cars, at their country's 2024 share
  ([<abbr title="International Energy Agency">IEA</abbr>](https://www.iea.org/data-and-statistics/data-tools/global-ev-data-explorer)),
  make rolling noise only. Motorcycles run 3.8 dB louder than cars, 2.9 dB pulling away (ASJ
  RTN-Model 2018), at shares from counts and national fleets (WHO 2023).
- **Speed.** An unsigned road takes its country's legal limit for its class, else 100 km/h on a
  motorway and 70 on a trunk road; on primary, secondary, tertiary and unclassified roads the
  median signed speed of the class in the country replaces it where that is lower or no legal limit
  is known. A residential street takes 30 km/h and a living street, service road or track 20, or
  the class's lower median sign (a German living street 10). On a two-way road outside towns, other
  than a motorway, at its country's open-road limit of 80 km/h or more, cars drive at 0.845 of it,
  the free-flow share measured in Great Britain. Lorries and buses drive at most 80 km/h, 90 to 105
  in thirteen countries.
- **Buses and bridges.** Uncounted roads also carry the buses of the lines mapped on them, a line at
  most twice, for its two directions, however many of its variants are mapped. One line direction
  carries 30 buses a day where 100,000 trips a day begin or end within 2 km; a street's buses grow
  with the 0.67 power of its line directions and the 0.47 power of those trips up to 600,000, and
  never fall below 12.5 % of that. A trolleybus line counts as 2.75 bus lines. Where the buildings
  are not mapped, the trips' part is 0.43 outside built-up areas and 1.40 inside. A line that tags
  its interval runs its shortest one for 18 hours in each direction, a coach line 4 times a day.
  The law is fitted on the
  <abbr title="General Transit Feed Specification, the format of public timetables">GTFS</abbr>
  timetables of 35 cities and rural areas in 14 countries, where it reads the buses 0.4 dB high in
  the median place, from 7.1 dB low in Tel Aviv to 3.2 dB high on the Setúbal peninsula; in eight
  German cities left out of the fit, 2.2 dB high. Buses keep the timetables' shares of their stop
  calls: 73 % by day, 16 % in the evening and 11 % at night. A road on a bridge emits from its deck,
  straight between the ground at the bridge's two ends.

</details>

<details>
<summary>Default traffic by road class</summary>

Defaults are length-weighted medians of counted public roads, fitted in September 2026 by class,
direction and surroundings; local counts, the models above and national tables come first.
Motorways, trunk and primary roads with 1 to 6 lanes take these vehicles a day per lane (a
three-lane urban one-way motorway: 3 × 12,543 = 37,629):

| Road | Direction | Rural | Urban | Surroundings unknown |
|---|---|---:|---:|---:|
| Motorway | One-way | 6,174 | 12,543 | 7,331 |
| Motorway | Two-way | 2,305 | 4,396 | 2,305 |
| Trunk | One-way | 2,950 | 6,188 | 3,652 |
| Trunk | Two-way | 1,959 | 4,000 | 2,205 |
| Primary | One-way | 3,314 | 4,980 | 4,579 |
| Primary | Two-way | 1,977 | 4,136 | 2,636 |

Without a plausible lane count, and on secondary and tertiary roads, the whole carriageway takes
these vehicles a day:

| Road | Direction | Rural | Urban | Surroundings unknown |
|---|---|---:|---:|---:|
| Motorway | One-way | 5,208 | 30,980 | 6,008 |
| Motorway | Two-way | 6,297 | 7,640 | 6,297 |
| Trunk | One-way | 5,061 | 9,744 | 6,652 |
| Trunk | Two-way | 4,260 | 8,289 | 4,840 |
| Primary | One-way | 4,966 | 7,963 | 7,103 |
| Primary | Two-way | 3,644 | 9,322 | 4,500 |
| Secondary | One-way | 1,030 | 3,222 | 1,500 |
| Secondary | Two-way | 2,061 | 6,445 | 3,000 |
| Tertiary | One-way | 2,266 | 4,298 | 4,132 |
| Tertiary | Two-way | 1,002 | 2,562 | 1,506 |

In squares with too few mapped buildings, uncounted roads keep the fixed estimate for their class or
country, and local streets the estimate of the prepared road tables.
Directional counts are kept as published; counts of both directions are shared between
carriageways, and a divided main road whose other carriageway is missing takes half.

</details>

## Railways

A railway is as loud as its trains: how many pass, how long and heavy they are and how fast they
run. Passenger trains come from public timetables. Freight, which almost no country publishes,
is estimated, and much of it runs at night.

Train counts come from <abbr title="General Transit Feed Specification, the format of public timetables">GTFS</abbr>
feeds and national timetables, every trip routed along the track network. Freight comes from
defaults, scaled in 28 European countries and Turkey to the country's official goods
train-kilometres ([Eurostat](https://ec.europa.eu/eurostat/web/transport/database) 2024; the United
Kingdom 2019). At public level crossings in the United States and Canada, locomotives sound their
horns.

<details>
<summary>How a train's sound power is computed</summary>

- **The train.** Each kind of train (passenger, freight, tram, light rail) has one sound power
  spectrum, its rolling noise growing with speed, set to a typical train by CNOSSOS-EU's
  per-vehicle computations: a passenger train is the mean of Germany's railway noise monitoring in
  2023 (26 axles, 176 m) up to 200 km/h and nears a 16-car high-speed set at 300; freight in the
  EU, Switzerland, Norway and Great Britain is the EU fleet's, quieter than elsewhere; light rail,
  with no typical train of its own, takes the regional train's correction. A heavy train's sound
  power is taken at no less than 50 km/h, a light-rail train's at 30, and freight runs at no more
  than 89 km/h; the trains of a period are spread along the line as CNOSSOS-EU spreads them.
- **The counts.** A stretch of track between two switches carries one count. Belgium and Greece
  publish no goods train-kilometres and keep the defaults' total; Switzerland keeps its defaults.
  In the EU, freight is weighted 4 on a
  <abbr title="Trans-European Transport Network, the EU's main transport corridors">TEN-T</abbr>
  core corridor, 2 on another TEN-T line and 0.5 off them within the country's total; in the EU,
  Switzerland, Norway and Great Britain 37.6 % of it runs between 22 and 06 h, as counted at the
  German monitors. Default passenger trains are lowered to fit the country's passenger
  train-kilometres in 12 European countries and Turkey. Lines mapped as passenger-only take no
  default freight.
- **Metro and crossings.** Above-ground metro uses the light-rail model; track underground emits
  nothing outdoors. At a level crossing, horns sound over up to a quarter mile before the crossing,
  for half the crossing's trains in each direction; US quiet zones stay silent, and partial ones at
  night.

</details>

<details>
<summary>Default trains a day</summary>

Where no count or national estimate exists; six countries have main-line freight defaults of their
own (Germany 24.5 trains a day, Switzerland 31.4). The country factors above then scale them: a
French main line carries 23 passenger trains a day, a German one 7 to 54 freight trains by its
TEN-T tier. Main, branch and industrial use must be mapped; other lines take the unclassified row.
Parallel tracks share their line's count.

| Railway | Passenger | Freight |
|---|---:|---:|
| Main line | 80 | 20 |
| Branch | 30 | 5 |
| Industrial | 0 | 15 |
| Unclassified | 40 | 10 |
| Tram | 120 | 0 |
| Light rail or metro | 80 | 0 |
| Narrow gauge | 10 | 0 |
| Funicular | 40 | 0 |

</details>

## Aircraft

Aircraft noise comes from a year of real flights, recorded by volunteers' receivers of the
<abbr title="Automatic Dependent Surveillance–Broadcast, the position signal aircraft send">ADS-B</abbr>
signal aircraft send. Each flight is as loud as its type at its height, its thrust and its flaps
and gear. Where no receiver is near, low flights are missing and the map reads too quiet.

The flights are [adsb.lol](https://adsb.lol)'s from September 2025 to August 2026, with the flights
only [ADSBExchange](https://www.adsbexchange.com) received added from 11 sample days. Each type
flies on its noise tables from the
<abbr title="European Union Aviation Safety Agency">EASA</abbr>
[<abbr title="Aircraft Noise and Performance">ANP</abbr> database](https://www.aircraftnoisemodel.org)
by <abbr title="the European Civil Aviation Conference's method for noise around airports">ECAC</abbr>
[Doc 29](https://www.ecac-ceac.org/activities/environment/doc-29), 4th edition. Near nine of 59 large
airports the map reads 5 to 23 dB too quiet. They are Beijing, Shanghai Pudong, Cairo, Casablanca,
Riyadh, Tehran, Accra, Moscow Sheremetyevo and Lagos.

<details>
<summary>How a flight's noise is computed</summary>

- **The flight.** Height above the geoid, thrust from the observed climb and acceleration, flaps
  and gear on approach; a type the database lacks flies as a similar aircraft. Helicopters take
  their type's certification level. A flight of unknown type flies as a helicopter or a Cessna 172
  by its transponder's category, as a Cessna 172 when its median speed in the air is under 140 kt
  and it carries no airline flight number, else as a generic jet.
- **Its phases.** On the ground: taxiing, take-off and landing rolls on the runways and taxiways,
  mapped or found where the flights' ground tracks run. Airborne: up to 7,620 m (25,000 ft) above
  the ground, the last distance of the noise tables; a flight turns to cruise at 8,000 m and back
  below 7,200 m. Cruise: above it.
- **The year in boxes.** The flights are summed into boxes of airspace: a cell of the map's grid
  and a slab of height above the highest ground around it. A box holds its flights' sound energy by
  day, evening and night at the ten distances of the noise tables, and its two loudest flights for
  the popup's list; a box's Lden reads back within 0.25 dB of its flights summed one by one, and so
  does each period.

</details>

## Ships

Ships are counted where their
<abbr title="Automatic Identification System, the position signal ships send">AIS</abbr>
signals are: at sea, on rivers and in harbours. A large ship gives off 108 dB(A), a work boat 98
and a leisure boat 88 ([Fredianelli et al. 2020](https://doi.org/10.3390/su12051740),
[Schiavoni et al. 2022](https://pmc.ncbi.nlm.nih.gov/articles/PMC9518360/)).

Vessel density comes from [EMODnet](https://emodnet.ec.europa.eu/en/human-activities) 2024 in
European seas (a 1 km grid) and [Global Fishing Watch](https://globalfishingwatch.org) elsewhere.
Leisure boats are counted only in European seas. A cell's ships are spread over the 90 m cells of
[ESA WorldCover](https://worldcover2021.esa.int/) 2021 that hold water, by their share of it (the
open sea where it maps no land), never over a dry one; south of 60° S and north of 82.75° N it maps
no land at all, so there the land reads as water. Speed is not modelled, so ships under way may be a
few decibels louder. Activity is constant over the day, which overstates leisure boats at night.

## Industry

A factory, a quarry or a power plant is as loud as its sector and its size. Its sound power is
spread over the site, a point every 75 m. A point inside one of the site's buildings emits from
that building, which does not screen it. A building on the site known to the data only as a building
or a block of flats is the site's, not a home. Wind turbines, solar farms and substations each
have a model of their own.

Sites and their kind are [OpenStreetMap](https://www.openstreetmap.org)'s. Four sources add the
sector:
[<abbr title="European Pollutant Release and Transfer Register">E-PRTR</abbr>](https://industry.eea.europa.eu)
in Europe, the [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase),
[Global Energy Monitor](https://globalenergymonitor.org)'s trackers (power, steel, cement, coal)
and national registers. The levels per sector and area are calibrated on the Czech strategic noise
maps of 2022.

<details>
<summary>Hours, turbines, solar farms and substations</summary>

- **Hours.** Coal and lignite mines run day and night; other mines, quarries and offices run 5 to
  8 dB quieter in the evening and 20 dB at night, warehouses 3 and 8 dB; inactive sites are silent.
- **Wind turbines.** The standing ones of OpenStreetMap and eight registers (the United States,
  Canada, Germany, Denmark, Sweden, Norway, Castilla-La Mancha, the Netherlands with Belgium), which
  add the turbines OpenStreetMap lacks and remove those they call dismantled. Each emits its annual
  operating level: the published maximum for its rated power, about 2 dB less for calm and
  part-load hours.
- **Solar farms and substations.** Solar farms hum from their inverters by day (88 dB(A) a MW at
  full output; 0.55 MW a hectare where untagged), substations from their transformers around the
  clock (<abbr title="the International Electrotechnical Commission's standard for transformer noise">IEC 551</abbr>).

</details>

## Buildings and places

Homes, shops and schools hum too: heat pumps and air conditioners, a car park's cars, the guests
on a bar's terrace, church bells. Each is modelled from what its country owns and uses, at the hours
it runs; a Bangkok home gives off 51 dB(A) by day. These are estimates, not measurements.

Buildings and noise walls also block sound. Their heights come from measured surveys where they
exist, then mapped heights and floor counts, then the typical height for the footprint. Walls come
from official inventories and OpenStreetMap. The country pages name the surveys and inventories.

Not modelled: roads and railways in tunnels, rail yards, heritage lines, curve squeal, motorsport,
deliveries, building sites, garden machinery, music and festivals.

<details>
<summary>Each kind of building and place</summary>

- **Homes** emit the outdoor units of the heat pumps and air conditioners their country's
  households own (surveys in 35 countries, the United States, Canada and the Gulf), for the hours
  their climate asks. Buildings known only from Overture's footprints emit nothing.
- **Food shops and supermarkets** emit their rooftop refrigeration by their footprint, day and
  night, 2 dB quieter in the evening and at night.
- **Other shops and offices** emit their cooling plant: sized by their floor and their climate, as
  [Heat Roadmap Europe](https://heatroadmap.eu/wp-content/uploads/2018/11/HRE4_D3.2.pdf) sizes
  and runs the cooling of the service sector, in the share of floor it finds cooled in each EU
  country (Czechia 10 %, Spain 73 %; Europe's other countries their region's middle, Norway 8 %;
  the rest of the world where cooling saturates, 85 %), at the sound power manufacturers declare
  for units of that capacity. It runs in business hours, Monday to Saturday 09-20, for the hours
  of full load the climate asks, and is heard while it runs, silent at night: in Czechia, where
  summers are cool, about 30 hours a year.
- **Buildings of restaurants, cafés and bars** emit their cooling while their places are open, and
  a restaurant's kitchen extract while its kitchen runs, an hour either side of its hours: the
  fan law over the footprint, at the 72 dB(A) measured at a fast-food restaurant's exhaust of
  316 m2, up to the size of a large restaurant.
- **Schools, hotels and other buildings** emit by type, area and assumed hours; the data often
  cannot tell a warehouse from a workshop.
- **Sports pitches** radiate [Sport England](https://www.sportengland.org)'s typical in-use level
  (58 dB LAeq,1h at 10 m) for their hours of play, a grass pitch 4 hours a week by day for 40
  weeks; courts, playgrounds, pools and terraces their active level for about an eighth of the day,
  stadiums on match days. Each is heard while in use, not as a steady hum at its mean. Outdoor
  shooting ranges fire 20,000 shots a year by day.
- **Car parks** follow the [Bavarian parking study](https://www.lfu.bayern.de/publikationen/get_pdf.htm?art_nr=lfu_lae_00045&pdf_nr=0)
  (LfU 2007): 63 dB(A) for one movement an hour, 0.4 movements a space an hour from 06 to 22 h and
  0.05 at night at most; where the buildings are mapped, a car park carries no more cars than
  the trip ends of the buildings around it: street parking those of the cars that park on the
  street, 15 % of a house's (up to four dwellings) and 55 % of a block of flats' and of the
  visitors', the lots the other 45 %, a house's other cars on its plot; each trip end parks once;
  elsewhere every space carries the study's rate. A movement sounds as 4 seconds at the peak of a
  car pulling away (the study's 67 dB(A) at 7.5 m; SEL 71 dB against LAmax 65 dB at 10 m in
  [Hepworth Acoustics](https://www.hepworth-acoustics.co.uk)'s measured car parks); a lot's search
  for a space makes it longer. A shooting range's shots sound a second each at their energy; a real
  shot is shorter, so a range reads somewhat loud.
- **Church bells** ring in Europe and Cyprus: prayer bells three times a day and a Sunday peal;
  Orthodox churches, and in 11 Orthodox countries every church of unmapped denomination, ring only
  before Saturday's vespers and Sunday's liturgy. German and Swiss church clocks strike the hours
  and quarters, Swiss ones at night too. A peal is 114 dB(A), a cathedral's 122.
- **Calls to prayer** sound at the place's prayer times, five a day at 118 dB(A) at every mapped
  mosque, from its minaret's horns or 10 to 15 m up; where mosques rarely call aloud, as in most of
  Europe, the Americas, Australia and East Asia, only a mosque with a mapped minaret calls, at
  Friday noon at 100 dB(A). In China, Tajikistan and Singapore no call sounds outside.
- **Terraces and doors.** People sit on the terraces of bars, cafés and restaurants and stand at the
  doors of bars and pubs from 19 h and of nightclubs, by their mapped opening hours, else the
  country's custom; terrace seats come from Madrid's and Melbourne's registers. On weekend evenings
  and nights three quarters of the seats are taken (ÖNORM's design value), on weekdays fewer by
  Melbourne's and Paris's counts; the daytime 30 % is an assumption. The guests are heard while
  they are there, the seats in the terrace season and the opening hours, the door crowd whenever
  the place is open, all year, not as a murmur all year at their mean; a bar's, pub's,
  nightclub's or beer garden's last hour is heard again 5 dB down in the night hour after it
  closes, as its guests leave. A terrace mapped within 30 m of a place is that place's seats,
  unless the place is tagged without outdoor seating. The guests sit 1.5 m in front of the nearest
  wall of the building the place is mapped in where that spot lies in no building (in front of a
  party wall it lies in the neighbour); walled in on every side, in front of the nearest wall.
- **What screens.** A wall without a height stands 4 m. Open car parks, carports, open roofs and
  underground footprints do not screen.

</details>

## Terrain, ground and weather

On its way to a point, sound crosses the terrain, the ground, the trees and the air:

- the terrain's height comes from national laser surveys in 17 countries and from a world model
  elsewhere;
- the ground is hard where it is sealed, built over or water, and soft elsewhere;
- the trees count by their cover and height;
- the weather is the climate of 1991–2020: what it does to a level is told on
  [Noise at a point](/about/noise-at-a-point#the-weather).

The map's Advanced layers show the terrain, the trees and the hard ground.

<details>
<summary>Terrain, ground and weather in detail</summary>

- **Terrain** is the bare earth of the national models, most from laser surveys, and elsewhere of
  the [GEDTM30](https://zenodo.org/records/18887460) world model, which keeps part of the forest
  canopy and of dense city blocks: around Prague it stood about 10 m above Czechia's national model
  in forests and 12.5 m in dense blocks, and the Old Town read 2 dB quieter on it. Cities without a
  national model, such as São Paulo, keep that bias.
- **Near a ray's two ends**, a national model of 5 m or finer adds what the one-arc-second grid
  misses: in Czechia, Germany, Switzerland, the Netherlands, Belgium and part of Portugal.
- **Weather** is the 1991–2020 climatology of
  [<abbr title="the fifth reanalysis of the European Centre for Medium-Range Weather Forecasts">ERA5</abbr>](https://www.ecmwf.int/en/forecasts/dataset/ecmwf-reanalysis-v5)
  every half degree: how often sound bends down to the ground from each of 16 directions by day,
  evening and night, and what the air absorbs in each octave
  ([how it enters a level](/about/noise-at-a-point#the-weather)).

The datasets are credited on [Data sources](/about/data-sources).

</details>

## The files

Everything the popup and the painter read is one tree of files: 1.07 TB for the world, in 377,082
files.

<details>
<summary>The squares and their files</summary>

The world is cut into squares 49 km wide in London (zoom 9 of the web map's tiles). Each square
keeps one file for each kind of data: the terrain, the buildings and walls, the sound sources and
three of flights. The popup reads only the tiles around its point, each with one read.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 244" role="img" aria-label="A zoom-9 square of eight by eight tiles with tile 2046/1362 of central London marked, and the square's directory holding its six files, one a kind of data, beside the year's weather table and flight tracks" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="8" y="34" width="128" height="128" fill="none" stroke="currentColor" stroke-width="1.2"/>
<path d="M24 34V162M40 34V162M56 34V162M72 34V162M88 34V162M104 34V162M120 34V162M8 50H136M8 66H136M8 82H136M8 98H136M8 114H136M8 130H136M8 146H136" fill="none" stroke="currentColor" stroke-opacity=".3" stroke-width=".75"/>
<rect x="104" y="66" width="16" height="16" fill="#2f7fd8"/>
<text x="8" y="22" fill="currentColor" font-weight="600">z9 square 255/170</text>
<text x="8" y="180" fill="currentColor" fill-opacity=".7">8 × 8 tiles of zoom 12,</text>
<text x="8" y="196" fill="currentColor" fill-opacity=".7">each 6.1 km in London</text>
<path d="M165 27V36M165 36H171M181 45V230M181 54H187M181 72H187M181 230H187" fill="none" stroke="currentColor" stroke-opacity=".45"/>
<rect x="198" y="83" width="192" height="106" rx="4" fill="#2f7fd8" fill-opacity=".13"/>
<g fill="currentColor" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace">
<text x="160" y="22">release/</text>
<text x="176" y="40">2026/</text>
<text x="192" y="58">weather</text>
<text x="192" y="76">255/170/</text>
<g font-size="11.5">
<text x="206" y="99">terrain</text>
<text x="206" y="115">obstacles</text>
<text x="206" y="131">sources</text>
<text x="206" y="147">aircraft</text>
<text x="206" y="163">aircraft-far</text>
<text x="206" y="179">aircraft-events</text>
</g>
<text x="192" y="234">aircraft-tracks/</text>
</g>
<text x="206" y="206" fill="currentColor" fill-opacity=".7" font-size="11">each the square's 64 tiles of its kind</text>
<text x="314" y="234" fill="currentColor" fill-opacity=".7" font-size="11.5">256 files</text>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Tile 2046/1362 in central London lies in zoom-9 square 255/170, whose directory keeps the square's 64 tiles of each kind of data in one file, a directory of where each begins first. London has no finer national terrain, so no terrain-fine file. The year's weather table and flight tracks lie beside the squares.</figcaption>
</figure>

</details>

<details>
<summary>Each kind of file</summary>

A square with nothing of a kind has no file of it, and a tile with nothing takes no bytes: the open
sea has no terrain. A tile holds its kind's records, most numbers in the fewest bytes their meaning
needs, to a step below what moves an answer, read in place without decoding: no general compressor,
a value or a name repeated in a tile kept once in a table of it. Only the terrain carries a coder:
a smooth field's information is the residual of a planar prediction of its heights, stored in zstd.

| Kind | A tile holds | The world |
|---|---|---|
| `terrain` | the ground's height every arc-second (31 m north to south), how much of it is sealed, its tree cover and how tall the trees stand | 202 GB, 7.0 million tiles in 117,367 files |
| `terrain-fine` | where a 5 m national model exists, what the arc-second grid misses, every sixth of an arc-second | 6.2 GB, 15,289 tiles in 301 files |
| `obstacles` | the outlines and heights of buildings and walls, listed by cells 48 m wide in London | 130 GB, 1.4 million tiles in 37,434 files |
| `sources` | every ground source, as points and straight pieces | 169 GB, 2.2 million tiles in 70,810 files |
| `aircraft` | a year of flights, summed into boxes of airspace | 466 GB, 2.6 million tiles in 57,670 files |
| `aircraft-far` | the same in boxes four times larger, read beyond 3 km | 64 GB, 2.6 million tiles in 57,670 files |
| `aircraft-events` | per cell 380 m wide in London, the flights of a day whose loudest moment reaches 50, 60 and 70 dB | 9.6 GB, 1.6 million tiles in 35,830 files |

Beside the squares lie the year's flight tracks, 28 GB in 256 files, read only to draw a listed
flight, and its weather table, 16.6 MB for every half degree: a 30-year climatology, the same in
every year.

</details>

<details>
<summary>A tile's sources</summary>

A tile's sources are pieces and the records they share. A piece is a point or a straight line —
its two ends, to 19 cm in London — and the number of its record. A record holds what the pieces of
one source have in common: its layer and group, its height above the ground, the ground under it,
its sound power in eight octave bands by day, evening and night, and what the popup shows of it, such as a road's vehicles a day, speed and surface. A piece on a bridge also stores the deck's
altitude at its two ends.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 240" role="img" aria-label="A road's way drawn as straight pieces, a long segment split in two and the way cut at the tile edge; in the tile's sources five pieces point to two records, and both records to one display" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<defs><marker id="hiw-arrow-records" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto"><path d="M0 0L8 4L0 8z" fill="currentColor"/></marker></defs>
<path d="M262 6V122" fill="none" stroke="currentColor" stroke-opacity=".55" stroke-dasharray="4 3"/>
<text x="254" y="16" text-anchor="end" fill="currentColor" fill-opacity=".7" font-size="11.5">tile 2046/1362</text>
<text x="270" y="16" fill="currentColor" fill-opacity=".7" font-size="11.5">2047/1362</text>
<path d="M16 100L64 96L112 88L166 70L220 56L262 50" fill="none" stroke="#d9622b" stroke-width="3" stroke-linejoin="round"/>
<path d="M262 50L316 44L388 40" fill="none" stroke="#d9622b" stroke-opacity=".35" stroke-width="3" stroke-linejoin="round"/>
<g fill="currentColor"><circle cx="16" cy="100" r="2.6"/><circle cx="64" cy="96" r="2.6"/><circle cx="112" cy="88" r="2.6"/><circle cx="166" cy="70" r="2.6"/><circle cx="220" cy="56" r="2.6"/><circle cx="262" cy="50" r="2.6"/><circle cx="316" cy="44" r="2.6"/><circle cx="388" cy="40" r="2.6"/></g>
<g fill="currentColor" fill-opacity=".75" font-size="11" text-anchor="middle"><text x="40" y="91">0</text><text x="87" y="85">0</text><text x="136" y="72">1</text><text x="190" y="56">1</text><text x="240" y="46">0</text></g>
<text x="160" y="108" text-anchor="middle" fill="currentColor" fill-opacity=".7" font-size="11.5">split into pieces ≤ 250 m</text>
<text x="268" y="72" fill="currentColor" fill-opacity=".7" font-size="11.5">cut at the tile edge</text>
<g fill="none" stroke="currentColor" stroke-opacity=".5" marker-end="url(#hiw-arrow-records)"><path d="M46 160Q117 110 187 159"/><path d="M70 160Q129 118 187 159"/><path d="M142 160Q165 142 187 159"/><path d="M94 160Q170 106 245 159"/><path d="M118 160Q182 114 245 159"/><path d="M189 184Q262 222 334 186"/><path d="M247 184Q291 206 334 186"/></g>
<g fill="#d9622b" fill-opacity=".15" stroke="currentColor" stroke-opacity=".6"><rect x="34" y="160" width="24" height="24"/><rect x="58" y="160" width="24" height="24"/><rect x="82" y="160" width="24" height="24"/><rect x="106" y="160" width="24" height="24"/><rect x="130" y="160" width="24" height="24"/></g>
<g fill="currentColor" fill-opacity=".08" stroke="currentColor" stroke-opacity=".6"><rect x="160" y="160" width="58" height="24"/><rect x="218" y="160" width="58" height="24"/><rect x="284" y="160" width="108" height="24"/></g>
<g fill="currentColor" text-anchor="middle" font-size="11.5"><text x="46" y="176">0</text><text x="70" y="176">0</text><text x="94" y="176">1</text><text x="118" y="176">1</text><text x="142" y="176">0</text><text x="189" y="176">record 0</text><text x="247" y="176">record 1</text><text x="338" y="176">display</text></g>
<g fill="currentColor" fill-opacity=".7" text-anchor="middle" font-size="11.5"><text x="94" y="232">pieces, 12 bytes</text><text x="218" y="232">records, 32 bytes</text><text x="338" y="232">display</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A way's straight segments become pieces of at most 250 m, cut where the way leaves the tile. In the tile's sources the five pieces point to two records, the stretch on a slope having its own, and both records point to one display: what the popup shows of the road, its fields typed by the roads' schema, its name kept once in the tile's table of texts.</figcaption>
</figure>

| Layer | A piece | A source in the popup's list |
|---|---|---|
| Roads | a straight stretch of an OpenStreetMap way, at most 250 m, cut at tile edges | a road by name, else number; with neither, its way |
| Railways | the same along a track | a line by name, else number; with neither, its way (unnamed tram tracks have no route or street identity) |
| Industry | a point, or a point every 75 m over a site above 5,000 m²; a turbine at its hub | a site; a turbine |
| Buildings | a point, or a point every 30 m over a building above 2,000 m² and every 75 m over a pitch or car park above 5,000 m² | a building; a pitch or car park; the guests of a bar; a church's bells |
| Ships | a point every 250 m over the water of a cell of vessel density | a cell |
| Airports | a straight stretch of a runway or taxiway, at most 250 m | an airport's ground operations, in the aircraft row |

Three tiles of central London, from Hammersmith to Bermondsey, hold 69,475 road pieces, half of
them shorter than 18 m; 5,932 railway pieces; 2,514 industrial points, 153,965 points of the
buildings layer and 589 of ships on the Thames. Flights are not pieces: they are the boxes
described under Aircraft.

</details>

## In the code

<details>
<summary>The builder's code: fetch/, build/, physics/ and tiles/</summary>

- `fetch/` downloads the datasets added to OpenStreetMap's: noise barriers, bus routes, national
  terrain models, wind turbines, venues, WorldClim, places of worship and the
  <abbr title="Trans-European Transport Network">TEN-T</abbr> rail network.
- `build/` turns the inputs into the tiles and every source's sound power. Its steps include
  `weather`, `terrain-fine`, `traffic` (the trips buildings make), `airport-traffic`, `geoid`, the
  aircraft chain and `complete`, the marker written last.
- `physics/` holds the emission models the builders run, with the standards' test cases.
- `tiles/` holds each kind's tile format, writer and reader, and the square files that hold the
  tiles (`tiles/src/square.rs`): the only code that knows the bytes, but for the GPU painter's
  kernel, which reads the obstacles tiles as stored (`paint-gpu/kernels/scene.cuh`).

</details>

<details>
<summary>What the readers rely on</summary>

| kind | module | contract |
|---|---|---|
| `terrain` | `tiles/src/terrain.rs` | the one-arc-second node window that brackets the tile: neighbours share their seam nodes; a sample touching a node without data is refused, never guessed; height, sealing, tree cover and crown height a node |
| `terrain-fine` | `tiles/src/terrain_fine.rs` | a national model's detail on the 1/6" lattice, its height less its own 1" average, faded towards its edges and voids; blocks of 128 x 128 cells, each one zstd frame behind a directory, so a point decodes only the blocks its rays' ends touch |
| square files | `tiles/src/square.rs` | `<year>/<x9>/<y9>/<kind>`: a zoom-9 square's 64 tiles of a kind behind a 536-byte directory, the format's magic, the square's x9 and y9 and the 65 offsets where each tile begins, row-major from the north-west; a tile with nothing of the kind takes no bytes; written to a temporary file of its own, synced and renamed into place |
| `obstacles` | `tiles/src/obstacles.rs` | outlines stored whole, listed per z19 cell (128 x 128 a tile): a bitmap marks the occupied cells and counts them before every 64, so a cell finds its runs in place; an outline's footprint id is its square's row in the tile's table and its ordinal |
| `sources` | `tiles/src/sources.rs` | 12-byte lines and 8-byte points, 32-byte records, each distinct emission row (per octave band and period a u16 in 0.01 dB) and display once, 8-byte deck entries (a bridge line's deck altitude at its ends); a display is its layer's fields typed by one schema (`tiles/src/sources/display.rs`: texts in the tile's table, numbers as whole counts of their field's decimal step, a railway's trains a day, a turbine's hub height and rating and an airport's movements as 8-byte reals), as the builder wrote them |
| `aircraft`, `aircraft-far` | `tiles/src/aircraft.rs` | the boxes, each with only the periods it is heard in and its loudest flight pieces, its slab a row of the tile's band table, its flights a day as a 16-bit rate; the tile's flight table; a piece's ends from its box's centroid (the farthest exact); a box finds its periods and pieces from the counts every 32 boxes |
| `aircraft-events` | `tiles/src/aircraft_events.rs` | 16 x 16 z16 cells a tile, the flights a day as 16-bit rates; no tile where no flight reaches 50 dB |
| `aircraft-tracks` | `tiles/src/aircraft_tracks.rs` | 256 files by the address's low byte, each sorted by address and start behind a fence of every 256th part's key, so a listed flight reads one 4 KB page of the directory; a part's points as 2-byte steps at 1e-5 deg |
| `weather` | `physics/src/weather.rs` | 0.5 deg nodes: favourable probability per period and 16 sectors in percent, absorption per octave in 0.01 dB/km |

- Tiles are numbered in the standard web-map XYZ numbering (y grows southwards), a square holding
  the 8 x 8 tiles of its zoom-9 number. Coordinates are int16 relative to the tile centre, step = tile width / 32,768: the
  tile spans +-16,384 steps and half a tile of margin fits on every side.
- Every emission piece lives in exactly one tile: a line is split at tile edges, a point lies in
  the tile holding it, so the sources of a ring never reach into a ring not yet read. An outline is
  stored whole in every tile whose cells it crosses; a wall longer than the frame is clipped into
  parts within it, a ring larger than the frame is left out (the builder counts it). Footprint and
  source ids are global and stable across tiles; holes are kept.
- A release is complete when its builder wrote the completion marker last, its first line naming
  the square files' layout. Only then does a missing file or tile mean "empty"; an unfinished build
  or a tree of another layout is never served.
- The physics tables (NPD, CNOSSOS coefficients, the Doc 29 classes) are compiled into the
  programs. Ground rays take the place's absorption; the NPD curves are moved to the place's air
  from the AIR-1845 atmosphere they come in (Doc 29 Appendix D, with the impedance adjustment), a
  box's at the middle of its z9 square; roads roll at the place's yearly air temperature
  (WorldClim 2.1, CNOSSOS-EU Eq. 2.2.10), with the gradient and junction terms.
- Tiles are read whole, a ring's asked for together: each square file's directory once, then
  every tile with one read of its bytes, one task a read; a fine terrain tile is read whole when a
  ray's end first needs it and its blocks are decoded on first use. The exceptions read by
  position: the weather table's nodes and the flight tracks (a file's fence, then for each listed
  flight a 4 KB page of the directory and its points).

</details>

<details>
<summary>The aircraft chain</summary>

The first step turns a year of ADS-B traces into flight segments per day (`build/src/aircraft`):
altitudes above EGM2008, each segment's Doc 29 phase and operation (A.3.2), and each flight's class,
decided once: its designator's ANP aircraft (55 classes, `scripts/aircraft-classes.py`), else by its
transponder category or slow flight, else a fallback; surface vehicles are ground traffic.
`aircraft-shuffle` sorts the window's segments into z9 squares once; `aircraft-boxes` builds a
square at a time and gives each segment its power by the force balance with the observed climb and
acceleration (`physics/src/doc29/thrust.rs`; an arrival's flap and gear below the landing
configuration height, B11); a power past the NPD rows reads the edge row, as Doc 29 gives Eq. 4-3
between tabulated powers only. A box is a web-map cell x a clearance slab above the highest terrain
within one edge; the edge grows with clearance so that it spans D = 3 dB of the steepest NPD slope
of 15 common classes there, and up to 3.8 dB of a few rarer, steeper curves (first layer about
50 m).

A segment is cut at tile edges, then into the pieces of the boxes it crosses; a box sums its pieces
per period at the ten NPD distances (an average day of the window: primary flights over the
baseline days, flights only the secondary provider saw over the increment days) with the geometry
of one average piece, read as two at the gradient plus and minus the spread of its pieces'
gradients, counts its flights and installation mix per period, and keeps its two loudest pieces for
the flight list (by LAmax at their height above the box's ground, one per flight). Far boxes, read
beyond 3 km, span D = 12 dB.

`aircraft-events` counts at every z16 cell each flight once, at its loudest moment there, with the
boxes' day weights. `aircraft-tracks` keeps every flight of the year flat, its segments' ends within
20 m by Douglas-Peucker, broken at the antimeridian and at any step over 300 km. `aircraft-check`
holds the boxes against every segment, the list against the exact ten loudest and the events
against exact counts. Airport ground operations (`build/src/airport`) put the window's ground legs
on the aeroway lines: a leg on a runway rolls at 40 kt or faster or as a take-off roll, and taxis
otherwise; a flight seen low over a runway without a roll of its own gets its class's typical roll.

</details>
