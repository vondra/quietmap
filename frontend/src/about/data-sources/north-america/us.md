---
title: United States
intro: FHWA HPMS 2022 traffic counts on federal-aid highways, the timetables of Amtrak and 53 commuter, metro and light-rail feeds, FRA crossing horns, USGS wind turbine database. Local streets are estimated from their buildings; freight uses class defaults.
---

## Roads

Traffic volumes: [FHWA Highway Performance Monitoring System](https://www.fhwa.dot.gov/policyinformation/hpms.cfm), 2022 edition, read from the public [HPMS feature service](https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0). An OSM road takes the nearest HPMS segment line within 100 m that runs along it and has a compatible functional class. A one-way segment counts one direction, unless it repeats the total of a two-way segment of the same road within 300 m; then it is that road's two-way total.

HPMS publishes one total per segment; the vehicle mix comes from FHWA's state shares (below).

Day, evening and night split: [FHWA TMAS](https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/) hourly counts of 8,056 stations in 2025, one split per road group (motorways and trunks, primary roads, other roads) for the whole country. TMAS publishes hourly totals only; trucks are assumed to follow the same hourly profile as cars.

Minor collectors and local streets are not in HPMS, whose values reach 60 % of the length of motorways, trunks and primaries and 1 % of secondary, tertiary and unclassified roads. Uncounted local streets carry the trips their buildings make, secondary and tertiary roads the trips made within 5 km of them. Motorways, trunks and primaries without a count follow one model of the trips made around them, fitted on counted roads ([method](/about/data-preparation)).

Vehicle mix: where not counted, FHWA's state mix by road group and area ([Highway Statistics](https://www.fhwa.dot.gov/policyinformation/statistics.cfm), table VM-4), motorcycles included; electric cars 1.9 % of cars, rolling noise only (IEA 2024); lorries and buses at most 105 km/h.

Unsigned roads: a primary road 64 km/h in towns, 89 outside (cars drive 75 on a two-way one).

## Railways

Passenger trains come from 54 timetables: Amtrak's and those of the commuter, metro, light-rail and streetcar systems below. Each is the version the [Mobility Database](https://mobilitydatabase.org/) archived last before 31 August 2026 that covers four weeks of the year to that day; Amtrak, WMATA and LA Metro publish only one to two weeks ahead and give the days they cover (7, 11 and 15), and SunRail's and Sacramento's archives were stored after that day but cover part of the year. A line's trains are the mean of the days covered, weekends included, and on a shared track the operators' trains add up (Amtrak's copy of MARC's Penn Line is left to MARC's own timetable). Commuter and intercity trains are routed along the tracks stop to stop; metro, light-rail and streetcar tracks within 500 m of a station take its departures in both directions (a station is a stop's parent in the timetable, else the stops of its name within 150 m), the line counting each station once and its tracks sharing them, and tracks farther from every station keep the defaults.

| Operator | What it covers | Timetable |
|---|---|---|
| Amtrak | Intercity trains, the state corridors and Shore Line East | [mdb-11-202608250016](https://files.mobilitydatabase.org/mdb-11/mdb-11-202608250016/mdb-11-202608250016.zip) |
| Metro-North | New York's Hudson, Harlem and New Haven lines | [mdb-524-202608150129](https://files.mobilitydatabase.org/mdb-524/mdb-524-202608150129/mdb-524-202608150129.zip) |
| Long Island Rail Road | Long Island commuter rail | [mdb-507-202608010127](https://files.mobilitydatabase.org/mdb-507/mdb-507-202608010127/mdb-507-202608010127.zip) |
| New York City Transit | Subway and Staten Island Railway | [mdb-516-202608280038](https://files.mobilitydatabase.org/mdb-516/mdb-516-202608280038/mdb-516-202608280038.zip) |
| NJ Transit | New Jersey commuter rail and its three light-rail lines | [mdb-509-202608060121](https://files.mobilitydatabase.org/mdb-509/mdb-509-202608060121/mdb-509-202608060121.zip) |
| PATH | New York and New Jersey rapid transit | [mdb-517-202506070024](https://files.mobilitydatabase.org/mdb-517/mdb-517-202506070024/mdb-517-202506070024.zip) |
| SEPTA | Philadelphia Regional Rail | [mdb-503-202607240144](https://files.mobilitydatabase.org/mdb-503/mdb-503-202607240144/mdb-503-202607240144.zip) |
| PATCO | Philadelphia–Lindenwold line | [mdb-3035-202607090020](https://files.mobilitydatabase.org/mdb-3035/mdb-3035-202607090020/mdb-3035-202607090020.zip) |
| SEPTA | Philadelphia's subway, elevated, trolleys and Norristown line | [mdb-502-202607030104](https://files.mobilitydatabase.org/mdb-502/mdb-502-202607030104/mdb-502-202607030104.zip) |
| MBTA | Boston subway, light rail and commuter rail | [mdb-437-202608110036](https://files.mobilitydatabase.org/mdb-437/mdb-437-202608110036/mdb-437-202608110036.zip) |
| Metra | Chicago commuter rail | [mdb-2854-202608230137](https://files.mobilitydatabase.org/mdb-2854/mdb-2854-202608230137/mdb-2854-202608230137.zip) |
| CTA | Chicago 'L' | [mdb-389-202607110020](https://files.mobilitydatabase.org/mdb-389/mdb-389-202607110020/mdb-389-202607110020.zip) |
| South Shore Line | Chicago–South Bend commuter rail | [mdb-585-202604240015](https://files.mobilitydatabase.org/mdb-585/mdb-585-202604240015/mdb-585-202604240015.zip) |
| WMATA | Washington Metrorail | [mdb-1847-202608190125](https://files.mobilitydatabase.org/mdb-1847/mdb-1847-202608190125/mdb-1847-202608190125.zip) |
| MARC | Maryland commuter rail | [mdb-468-202607030106](https://files.mobilitydatabase.org/mdb-468/mdb-468-202607030106/mdb-468-202607030106.zip) |
| Maryland Transit Administration | Baltimore light rail | [mdb-469-202607100108](https://files.mobilitydatabase.org/mdb-469/mdb-469-202607100108/mdb-469-202607100108.zip) |
| Maryland Transit Administration | Baltimore Metro Subway | [mdb-470-202603050033](https://files.mobilitydatabase.org/mdb-470/mdb-470-202603050033/mdb-470-202603050033.zip) |
| VRE | Virginia commuter rail | [tld-61-202608310057](https://files.mobilitydatabase.org/tld-61/tld-61-202608310057/tld-61-202608310057.zip) |
| Caltrain | San Francisco Peninsula commuter rail | [mdb-54-202608290127](https://files.mobilitydatabase.org/mdb-54/mdb-54-202608290127/mdb-54-202608290127.zip) |
| BART | San Francisco Bay Area rapid transit | [mdb-53-202608050057](https://files.mobilitydatabase.org/mdb-53/mdb-53-202608050057/mdb-53-202608050057.zip) |
| SFMTA | San Francisco Muni Metro and streetcars | [mdb-2886-202607250136](https://files.mobilitydatabase.org/mdb-2886/mdb-2886-202607250136/mdb-2886-202607250136.zip) |
| VTA | San José light rail | [mdb-57-202607160107](https://files.mobilitydatabase.org/mdb-57/mdb-57-202607160107/mdb-57-202607160107.zip) |
| ACE | Stockton–San José commuter rail | [mdb-2684-202608110143](https://files.mobilitydatabase.org/mdb-2684/mdb-2684-202608110143/mdb-2684-202608110143.zip) |
| SMART | Sonoma–Marin commuter rail | [mdb-3222-202606300118](https://files.mobilitydatabase.org/mdb-3222/mdb-3222-202606300118/mdb-3222-202606300118.zip) |
| LA Metro | Los Angeles metro and light rail | [mdb-30-202608160127](https://files.mobilitydatabase.org/mdb-30/mdb-30-202608160127/mdb-30-202608160127.zip) |
| Metrolink | Southern California commuter rail | [mdb-96-202605230122](https://files.mobilitydatabase.org/mdb-96/mdb-96-202605230122/mdb-96-202605230122.zip) |
| MTS | San Diego Trolley | [mdb-13-202608220023](https://files.mobilitydatabase.org/mdb-13/mdb-13-202608220023/mdb-13-202608220023.zip) |
| Sound Transit | Seattle Link light rail and Sounder | [mdb-268-202608080148](https://files.mobilitydatabase.org/mdb-268/mdb-268-202608080148/mdb-268-202608080148.zip) |
| TriMet | Portland MAX, WES and streetcar | [mdb-247-202608250120](https://files.mobilitydatabase.org/mdb-247/mdb-247-202608250120/mdb-247-202608250120.zip) |
| MARTA | Atlanta rapid transit | [mdb-368-202604190110](https://files.mobilitydatabase.org/mdb-368/mdb-368-202604190110/mdb-368-202604190110.zip) |
| Miami-Dade Transit | Metrorail and Metromover | [mdb-331-202608010003](https://files.mobilitydatabase.org/mdb-331/mdb-331-202608010003/mdb-331-202608010003.zip) |
| Tri-Rail | South Florida commuter rail | [tld-492-202510290055](https://files.mobilitydatabase.org/tld-492/tld-492-202510290055/tld-492-202510290055.zip) |
| Brightline | Miami–Orlando intercity trains | [mdb-1126-202608040149](https://files.mobilitydatabase.org/mdb-1126/mdb-1126-202608040149/mdb-1126-202608040149.zip) |
| SunRail | Orlando commuter rail | [mdb-626-202609290129](https://files.mobilitydatabase.org/mdb-626/mdb-626-202609290129/mdb-626-202609290129.zip) |
| DART | Dallas light rail | [mdb-152-202607200103](https://files.mobilitydatabase.org/mdb-152/mdb-152-202607200103/mdb-152-202607200103.zip) |
| Trinity Metro | TEXRail | [mdb-2890-202608290102](https://files.mobilitydatabase.org/mdb-2890/mdb-2890-202608290102/mdb-2890-202608290102.zip) |
| METRO | Houston light rail | [mdb-2060-202607120126](https://files.mobilitydatabase.org/mdb-2060/mdb-2060-202607120126/mdb-2060-202607120126.zip) |
| CapMetro | Austin commuter rail | [mdb-150-202607260154](https://files.mobilitydatabase.org/mdb-150/mdb-150-202607260154/mdb-150-202607260154.zip) |
| RTD | Denver light rail and commuter rail | [mdb-178-202608200126](https://files.mobilitydatabase.org/mdb-178/mdb-178-202608200126/mdb-178-202608200126.zip) |
| UTA | Salt Lake TRAX, FrontRunner and S-Line | [mdb-170-202607280109](https://files.mobilitydatabase.org/mdb-170/mdb-170-202607280109/mdb-170-202607280109.zip) |
| Valley Metro | Phoenix light rail and streetcar | [mdb-147-202607280152](https://files.mobilitydatabase.org/mdb-147/mdb-147-202607280152/mdb-147-202607280152.zip) |
| Metro Transit | Minneapolis–St Paul light rail | [mdb-205-202608020145](https://files.mobilitydatabase.org/mdb-205/mdb-205-202608020145/mdb-205-202608020145.zip) |
| Metro St. Louis | MetroLink | [mdb-190-202605150006](https://files.mobilitydatabase.org/mdb-190/mdb-190-202605150006/mdb-190-202605150006.zip) |
| Pittsburgh Regional Transit | Light rail and inclines | [mdb-409-202607080039](https://files.mobilitydatabase.org/mdb-409/mdb-409-202607080039/mdb-409-202607080039.zip) |
| Greater Cleveland RTA | Rapid transit | [mdb-406-202607290022](https://files.mobilitydatabase.org/mdb-406/mdb-406-202607290022/mdb-406-202607290022.zip) |
| SacRT | Sacramento light rail | [mdb-3621-202611091544](https://files.mobilitydatabase.org/mdb-3621/mdb-3621-202611091544/mdb-3621-202611091544.zip) |
| CATS | Charlotte Lynx and streetcar | [mdb-2265-202608040036](https://files.mobilitydatabase.org/mdb-2265/mdb-2265-202608040036/mdb-2265-202608040036.zip) |
| CTrail | Hartford Line | [mdb-2840-202607140121](https://files.mobilitydatabase.org/mdb-2840/mdb-2840-202607140121/mdb-2840-202607140121.zip) |
| Hampton Roads Transit | Norfolk Tide light rail | [mdb-473-202605120002](https://files.mobilitydatabase.org/mdb-473/mdb-473-202605120002/mdb-473-202605120002.zip) |
| RTA | New Orleans streetcars | [ntd-60032-202605140120](https://files.mobilitydatabase.org/ntd-60032/ntd-60032-202605140120/ntd-60032-202605140120.zip) |
| City and County of Honolulu | Skyline | [mdb-2350-202607090002](https://files.mobilitydatabase.org/mdb-2350/mdb-2350-202607090002/mdb-2350-202607090002.zip) |
| Cincinnati Metro | Connector streetcar | [mdb-307-202607010002](https://files.mobilitydatabase.org/mdb-307/mdb-307-202607010002/mdb-307-202607010002.zip) |
| Detroit People Mover | People Mover | [mdb-2281-202606270026](https://files.mobilitydatabase.org/mdb-2281/mdb-2281-202606270026/mdb-2281-202606270026.zip) |
| King County Metro | Seattle streetcars | [mdb-1330-202608290047](https://files.mobilitydatabase.org/mdb-1330/mdb-1330-202608290047/mdb-1330-202608290047.zip) |

Not loaded: San Diego's Coaster and Sprinter (no archive of the year), Buffalo's Metro Rail (not in its operator's open feed), the Kansas City streetcar (its archives start after the year), the Alaska Railroad (no open timetable). Their lines use the [world railway defaults](/about/data-preparation). Lines without a usage tag use the [unclassified railway default](/about/data-preparation). Surface metro sections are included; see the [railway method](/about/data-preparation).

Freight: BNSF, Union Pacific, CSX and Norfolk Southern publish no schedules; main lines take the class default of 20 freight trains a day, branches 5.

Level-crossing horns: [FRA Highway-Rail Crossing Inventory](https://data.transportation.gov/Railroads/Crossing-Inventory-Data-Form-71-Current/m2f8-22s6) (Form 6180.71; public domain). Public at-grade crossings with trains sound on approach; full-day quiet zones and Chicago-excused crossings stay silent, partial zones at night.

## Industry

Wind turbines: [US Wind Turbine Database](https://energy.usgs.gov/uswtdb/) from USGS, LBNL and DOE (public domain). Each database turbine is paired with one OSM turbine within 500 m, nearest pairs first, and gives it the database's rated power and hub height.

Power plants: WRI Global Power Plant Database. Steel plants, cement plants and coal mines: Global Energy Monitor. Other factories are OSM industrial areas with an inferred type. The EPA Toxics Release Inventory is not used.

The register adds 1,610 turbines OpenStreetMap lacks.

## Ships

Global Fishing Watch AIS vessel density.

## Buildings and terrain

Terrain from USGS's [3DEP](https://www.sciencebase.gov/catalog/item/4f70aa9fe4b058caae3f8de5) (the 48 states and Alaska; public domain); noise barriers from OpenStreetMap and the inventories of Washington ([WSDOT Noise Walls](https://geo.wa.gov/datasets/WSDOT::wsdot-noise-walls/about)), Florida ([FDOT Noise Barriers](https://www.geoplan.ufl.edu/noise-barrier-inventory/)) and Virginia ([VDOT Noise Barrier Study](https://www.arcgis.com/home/item.html?id=695b47c2cede44aebcade31fd8bc3901)).

## Homes and places

Homes: heat pumps and air conditioners as the RECS survey counts them. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours close at 02:00.

## Checked against

Minneapolis–St Paul airport's monitors ([validation](/about/validation)).
