# Methodology

A click sums every source within 12 km of the point (aircraft within 16 km) at a receiver 4 m above
the ground and reports Lden, the day (07–19), evening (19–23) and night (23–07) levels, the levels
exceeded 5, 10, 50 and 90 % of the time and the loudness. Ground sources carry a sound power per
octave band (63 Hz to 8 kHz) and period; flights carry Doc 29's noise-power-distance energies. Inside
a building the click answers at the building's loudest façade point, placed as CNOSSOS-EU places
receivers for building exposure, without indoor attenuation; a building with no exposed façade gets
no answer.

## Roads

Emission follows CNOSSOS-EU for light, medium and heavy vehicles: rolling and propulsion noise, road
surface, gradient, acceleration at junctions and the place's yearly air temperature (WorldClim 2.1);
unpaved roads add 2 dB to rolling noise. Battery-electric cars, at their country's share of the 2024
stock (IEA, 44 countries), make rolling noise only. Motorcycles run 3.8 dB louder than cars at the
same speed and 2.9 dB louder pulling away at junctions (ASJ RTN-Model 2018).

Traffic is counted where a national census, a road department or a city publishes counts; the
country pages say where ([Africa](/about/africa), [Asia](/about/asia), [Europe](/about/europe),
[North America](/about/north-america), [Oceania](/about/oceania),
[South America](/about/south-america)). Where nobody counted:

- A local street carries the trips of the buildings it serves. Every building, OpenStreetMap's and
  the Overture footprints OpenStreetMap lacks, makes trip ends that follow the street network to
  the nearest main road; no street carries more than the one it drains into. Where too few
  buildings are mapped, streets keep a fixed estimate.
- An unclassified road outside a town carries the trips routed down it; in a town, the larger of
  those and how the counted roads of Great Britain, France and Sweden relate to the trip ends within
  2 km.
- Secondary and tertiary roads whose only estimate was their class's follow how the counted roads of
  13 European countries relate to the trip ends within 5 km, shifted for Czechia, Finland, Britain
  and Sweden, and tertiary roads in 41 European countries lowered where only the busier ones were
  counted.
- Motorways, trunk and primary roads whose only estimate was their class's follow, in Europe, the
  United States, Mexico, Chile, Colombia, Japan and New Zealand, one model on the trip ends within 1,
  5 and 15 km, the lanes and one-way: held out a country at a time it misses by 2.3 dB; Mexico, left
  out of the fit, by 2.9 dB.
- Everywhere else, and wherever a country's own estimate table exists (Japan, Russia, Turkey,
  Ukraine and others), main roads keep a fixed estimate per class and lane: about half of the
  world's main-road length. There a level reads a class, not the road.
- Thailand's national highways carry the highway department's vehicle-kilometres per province; the
  rural roads department's roads its counts, or where uncounted a fit on them.

The vehicle mix is counted where counted. Otherwise a main road takes the counted median of its class
(16 countries), a local street 1 % medium and 2 % heavy vehicles; in the United States a road takes
its state's mix by road group and area (FHWA Highway Statistics 2023, table VM-4). Motorcycles come
from counts and national fleets (WHO 2023).

Day, evening and night shares are measured in the United States, Japan, Germany (Baden-Württemberg's
counters, lorries apart), the Netherlands' national roads, Great Britain, Spain and Thailand;
elsewhere the region's measured median, or a default. Within each period three hourly profiles of
242 counters in Baden-Württemberg shape the levels exceeded part of the time, not Lden.

Speeds are the posted limit. An unsigned road takes its country's legal limit for its class (50 to
100 km/h by class where the country has none; a residential street 30 km/h, a living street, service
road or track 20) or, below trunk roads, the median signed speed of its class in the country where
that is lower. On a two-way road outside towns, other than a motorway, at its country's limit of 80
km/h or more, traffic drives 0.845 of the limit (free-flow speeds in Great Britain). Lorries and
buses run at most 80 km/h, 90 to 105 in 13 countries.

Roads nobody counted carry the buses of OpenStreetMap's routes. Car parks (in the Buildings layer)
follow the Bavarian parking study: 63 dB(A) per movement, 0.4 movements per space and hour from 06
to 22 h and 0.05 at night; street parking carries only the cars of the buildings around it.

## Railways

Four whole-train spectra (passenger trains, high-speed ones included; freight; trams; light rail
with metros), each with a rolling term growing as 30 lg v and a constant traction term, shifted to
the typical train by CNOSSOS-EU per-vehicle computations and the pass-bys at Germany's 19 railway
noise monitors (2023); freight on European lines is the EU fleet's, quieter than elsewhere. A heavy
train's sound power is taken at no less than 50 km/h and a light-rail train's at 30, as CNOSSOS-EU
asks; freight runs at no more than 89 km/h. A period's trains spread over the line as in CNOSSOS-EU.
Heritage lines are silent.

Train counts come from the timetables matched to the track (32 countries; not Britain's main lines,
not freight). Lines without one carry, in 31 European countries, their country's train-kilometres
(Eurostat); elsewhere a fixed 80 passenger and 20 freight trains a day on a main line, 30 and 5 on a
branch, 40 and 10 on an untagged line, 120 trams or 80 light-rail trains. In the EU, freight without
a count weighs 4 on a TEN-T core corridor, 2 on a TEN-T freight line and 0.5 off them; on European
lines 37.6 % of freight trains run between 22 and 06 h, as counted at the German monitors.
Locomotives sound their horn at level crossings in the United States and Canada.

## Aircraft

A year of recorded ADS-B flights (September 2025 – August 2026) from adsb.lol, with the flights only
ADSBExchange received added from 11 sample days on which both are complete. Each aircraft type flies
its EASA ANP aircraft (a similar one where the ANP has none) by ECAC Doc 29, 4th edition: altitude
above the geoid, thrust from the observed climb and acceleration, flaps and gear on approach. Types
nobody mapped, military jets and tankers fly a generic jet; helicopters take their type's
certification level on one helicopter curve.

The flights are summed into boxes of airspace; at 27 points a box reads back within 0.2 dB of every
flight summed on its own in Lden, within 0.3 dB in a period. The levels follow Doc 29's noise power,
distance, lateral attenuation and engine installation in the place's air; hills and buildings near
the receiver screen them. The click lists the ten loudest flights. Taxiing, take-off and landing
rolls run on the runways and taxiways, mapped or found from the flights' ground tracks, at a fixed
level per aircraft family.

Flights are missing where volunteer receivers hear little: near nine of 59 large airports (Beijing,
Shanghai Pudong, Cairo, Casablanca, Riyadh, Tehran, Accra, Moscow Sheremetyevo, Lagos) the map reads
5 to 20 dB too quiet, near most well-covered hubs 0.4 to 1.2 dB.

## Industry and ships

Factories, power plants, mines and quarries emit by their type and area; the type comes from
OpenStreetMap, the sector from E-PRTR, the Global Power Plant Database, Global Energy Monitor and
national registers where matched. Wind turbines, the standing ones of OpenStreetMap and eight
registers (the United States, Canada, Germany, Denmark, Sweden, Norway, Castilla-La Mancha, and the
Netherlands with Belgium), emit the published maximum for their rated power less an operating
allowance; solar farms emit from their inverters by day, substations from their transformers around
the clock.

Ships come from AIS vessel density (EMODnet in European seas, Global Fishing Watch elsewhere), as the
mean number of ships present at 108 dB(A) for large ships, 98 for work boats and, in European seas,
88 for leisure craft.

## Buildings and places

Homes mapped in OpenStreetMap emit the outdoor units their country's households own (heat pumps and
air conditioners per household, from national surveys in 35 countries, the United States, Canada and
the Gulf; elsewhere their region's median) running the hours their climate asks for (WorldClim degree
days): a Prague home 45 dB(A) by day, a Bangkok home 51. Buildings known only from Overture's
footprints emit nothing: in Bangkok 2.1 of 2.3 million footprints. Shops, schools, hotels and other
buildings emit by their class and floor area. Sports pitches, courts, playgrounds, pools, stadiums on
match days and shooting ranges by day emit as open-air leisure areas.

Church bells ring as events, in Europe and Cyprus. Churches and bell towers ring the prayer bells
three times a day for three minutes and a Sunday peal; German and Swiss clocks strike the quarters by
day and in the evening, Swiss ones at night too. Orthodox churches, and in the 11 Orthodox countries
every church of no mapped denomination, ring only before Saturday's and Sunday's services, for five
minutes. A peal is 114 dB(A), a cathedral's 122.

Mosques call to prayer at their place's prayer times, computed for every mosque with its country's
method and time zone: five calls a day of three minutes (the dawn call 3.4) at 118 dB(A), from horns
at three quarters of a mapped minaret's height, else 15 m up (10 m in Indonesia, Malaysia and
Brunei), at least 2 m above the mosque. Country rules change this: Saudi Arabia calls at a third of
the power and, like Egypt, adds the iqama; Indonesia adds the recitation before the call, Turkey and
Cyprus the Friday sala; Rwanda has no dawn call; Chinese, Tajik and Singapore mosques do not call
outside. In Europe (except Albania, Bosnia and Herzegovina, Bulgaria, Cyprus, Greece, Kosovo,
Montenegro, North Macedonia, Romania, Serbia, Russia and Turkey), the Americas, Australia, New
Zealand, Japan, South Korea, Taiwan, Hong Kong and Macao only mosques with a mapped minaret call,
once on Fridays at 100 dB(A).

The people outside bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food places sit
on their terraces and stand at their doors. A person present emits a talker's sound power with a third
of the people talking: 63.6 dB(A) while eating, 70.7 lively, 73.0 standing at night. Terrace seats come
from the terrace registers of Madrid and Melbourne, occupied 75 % on weekend evenings and nights and
30 % by day (an assumption: nothing is measured). Bars and pubs keep 2.4 people at the door from 19 h,
nightclubs whenever open; clusters of bars add their street crowd at night and empty into the street
after closing. Hours are the mapped opening hours, else the country's custom; terraces count in the
warm part of the year.

Buildings, noise barriers and terrain screen sound. Building heights come from measured surveys,
mapped heights, floor counts, Overture heights and finally the typical height of the footprint. Noise
barriers come from OpenStreetMap and the inventories of the Netherlands, Washington, Florida, Virginia
and Poland.

## Propagation

Ground sources follow CNOSSOS-EU in eight octave bands: geometric spreading, air absorption (ISO 9613-1)
averaged over the place's 3-hourly weather of 1991–2020, the ground's effect (soft or hard by how much
of it is sealed or water), diffraction over terrain, roofs and barriers, and the share of favourable
(downward-refracting) weather by direction from the same ERA5 climatology (0.5° grid). Forest does
not attenuate: CNOSSOS-EU has no foliage term.

Terrain is a one-arc-second lattice (about 30 m): the GEDTM30 world model with national models in 17
countries, listed on the [credits page](/about/credits). Open national models not yet used include
England's lidar, Japan's, Mexico's and New Zealand's. Where GEDTM30 is the terrain it keeps buildings
and trees: in Prague, before the national model replaced it, dense blocks stood 12.5 m high and
screened the squares.

Reflections are not traced. Instead of CNOSSOS-EU's image sources a receiver among buildings gains 3
dB when more than half of nine points 75 m apart around it fall inside buildings taller than 5 m, 1.5
dB when more than a fifth do, else nothing; flights gain nothing.

## Time and loudness

The levels exceeded part of the time come from 2,000 moments of each period, drawn with the click as
seed: one weather state for all sources, one hour for the roads (the hourly profiles above), each
road, railway and airport line, and the flights as one, a Poisson stream of passes at its own rate
(Kurze), bells and calls in their share of the period; industry, buildings and ships are steady.

Loudness in sone follows ISO 532-1, Zwicker's method for steady sound. Each period's received
spectrum — the octave bands shared equally by their thirds, flights by the loudest flight's Doc 29
spectral class — is set to the period's level exceeded 5 % of the time and taken as steady: an
approximation of the N5 that ISO 532-1 defines for time-varying sound. The whole day's loudness is
that of the three periods' spectra averaged over their hours with the evening 5 dB and the night 10
dB up, as in Lden, so it can exceed the loudness of every single period.

## Standards

- [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226): road and rail emission, propagation,
  receivers at façades
- [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod)
  with [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data)
  data: aircraft (helicopters by their certification levels)
- [ISO 532-1](https://www.iso.org/standard/63077.html): loudness
- [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng): Lden and receiver height

The model uses these methods with the inputs above; it is not certified against them. It is an
estimate for comparison and exploration, not a measurement, a legal noise map or a property
certificate.

## Known limits

- Natural sound (wind, water, birds, insects) is not modelled: where human-made sound is faint, a
  place sounds louder than the map says. But at 15 of 73 measured quiet places in the mountains the
  model is louder than everything measured, by up to 18 dB, each time because of a road.
- Half of the world's main-road length and every railway outside Europe without a timetable carry
  fixed estimates, not their own traffic.
- Flights are missing where receivers are few (see Aircraft).
- Barcelona's and Madrid's wide avenues read 3 to 6.5 dB too loud.
- Crowds on squares and pedestrians by day are missing: Barcelona's nightlife stations read 6 dB under
  the measurement.
- Railways at night read 3 dB under Germany's 19 monitors, the same monitors the rail model is fitted
  on; the cause is not known.
- OpenStreetMap maps about half of Turkey's mosques, 10 to 14 % of Indonesia's and 1.4 to 3 % of
  Egypt's, India's and Pakistan's; unmapped mosques do not call.
- Silent: roads and railways in tunnels (and their portals), heritage railways, rail yards, curve
  squeal, motorsport, inactive industry, worship other than church bells and calls to prayer, homes
  known only from Overture's footprints, deliveries, building sites, garden machinery, music and
  festivals.
