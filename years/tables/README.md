# Model input tables

Each year's JSON files contain `rows`. They are inputs to `build/src/source_tables/`,
resolved by the corresponding manifest id. The year belongs to the containing directory;
observation intervals, publication dates and dependencies belong to the manifest. A missing
or empty required table stops the build. Completion also compares every consumed table with
the published table of that kind; changing one copy alone cannot produce a complete release. A missing country may use the documented world or
regional model; it does not make a missing table admissible.

Country rows have `country` (ISO alpha-2) and `value`. A regional row has `countries` instead:
its value is used only when there is no country row. An empty `countries` list is the world
model, used after regional rows. Overlapping regions and duplicate countries are errors.
Numbers are parsed with `serde_json/float_roundtrip`: the expected pub seats
`22.400000000000002` must retain the bits of `0.56 * 40.0`.

## Road and rail quantities

| Table | Row layout and units | Derivation |
|---|---|---|
| electric-car-share | Country; percentage of cars | IEA Global EV Data Explorer, battery-electric stock / car stock |
| motorcycle-share | Country; fleet share and daily-use weight | WHO and national vehicle fleets, use weights fitted on DfT counts; the fit depends on that count edition |
| road-period-shares | Country; `[road group][light/heavy][day/evening/night]`, fractions | Hourly counts and roadside monitoring; groups are motorway, main road and street; each triple sums to one |
| tagged-speeds | Country; eight road classes, `[rural,urban]`, km/h | Median mapped speed signs; the mapped network is a dependency |
| legal-speeds | Country; `[urban,rural,motorway,motorroad]`, km/h | Published passenger-car rules; zero means no supplied limit, not stationary traffic |
| heavy-speeds | Country; km/h | Truck and bus legal limits and speed limiters |
| road-class-priors | Country; `[secondary,tertiary][unknown,rural,urban]`, vehicles/day | Length-weighted medians of counted roads in Czechia, Britain and Sweden; unknown surroundings use √(rural × urban), and Czech tertiary values use the Swedish/British tertiary-to-secondary ratios |
| road-class-shares | Country; `[secondary,tertiary][rural,urban][medium,heavy]`, percent | Counted vehicle-class medians in the same countries |
| counted-class-shares | Five road classes motorway through tertiary; `[rural,urban][medium,heavy]`, percent | Counted road medians; local streets retain the documented world mix |
| rural-free-flow | One ratio | DfT SPE0102, free-flow car speed / rural single-carriageway limit; applied only at a known rural limit of at least 80 km/h |
| train-scales | Country; passenger/freight multipliers and three corridor weights | National train-kilometres / train-kilometres implied by the mapped network; EU core, other corridor and off-corridor weights distribute freight inside the national total |
| rail-fleet-regions | Country; boolean | Networks represented by the European freight fleet observations |
| freight-period-shares | One day/evening/night triple, fractions | German EBA monitor freight counts |

The global road-period model applies where no country or regional observations exist. Main
roads without local counts use the global road model; the lack of counts is not an excuse to
omit its fitted tables. `road-class-priors` override the older global class medians described
on Data preparation; the two are not the same fit.

## Railway preparation inputs

`rail-model.json` is the single preparation model: category/source precedence, country ownership,
daily class priors, period fractions, spatial corridor rules and matching extents. Daily counts
are trains across the whole line, before sharing between parallel tracks. The ordered corridor
rules preserve the explicitly estimated counts documented on the country source pages; they
are not timetable observations. The six national main-line freight priors were fitted using
Eurostat train-kilometres and a later mapped network, so the historical model omits that fit.

The manifest's `railways.objects` chooses publisher adapters and input ids. Converters produce
one version-2 JSON-lines movement stream: a window header, then service paths, station observations,
directed station pairs, spatial observations or crossing soundings. Passenger and freight are
independent nullable counts: null is unknown and zero is an observed absence. Calendar coverage
keeps one bit per day of the flow window. A GTFS operator keeps its own observation horizon;
the span of its selected service calendars, with seasonal patterns kept together.
A complete `feed_info` interval can establish
known zero dates. `ignore_feed_window` selects the publisher adapter's service-calendar bounds
when that metadata overstates the archive's coverage; the input description records why.
Each timetable count is the sum of active departures
divided by the entire window's day count. `missing_calendar_estimate` stores the additional
passenger and freight counts inferred from that same observation's mean on covered dates.
It is computed before any station or interval aggregation, follows the winning source and
is shared across tracks with the observed count. A zero from partial calendar coverage remains
in the normalized observations but cannot claim annual silence: the matcher leaves that category
to its declared model. A complete calendar can establish an annual zero. Prepared rows retain six period components;
native display records retain the two estimated shares. Absent components in older files mean
zero; a partial set is invalid. The production converter has no selected-weekday option.
The annual assumption and its limits are described in Data preparation.
An absent optional shape keeps ordered-station matching with its ambiguity check. A GTFS service
with any unresolved stop keeps its count as an unlocated observation. CIS retains the ordered
pairs of resolved timing points; fewer than two resolved stations also keeps an unlocated count.
An unlocated observation adds no spatial claim and is reported separately from observed zero.
The converter prints structured `railways: operator-calendar` records with covered dates and
trip counts. These reproduce the observation bounds in each selected manifest; edition metadata
is committed with its inputs, rather than discovered from a current feed at build time.

Source ids, ranks and editions preserve provenance and precedence; they never choose input
files. Only manifest admission selects sources. `rail-movements` depends on exactly the admitted
publisher objects and any station lookup used by the conversion. Historical archives that are
absent or not admitted leave the declared class estimate. The movement stream's window must
match the selected manifest. Original OSM node identities and way distances remain authoritative
through graph matching, interval splitting and finalization.

## Thai highway counts and subdivision grids

`road-reference-counts` rows are country, highway number, subdivision, daily traffic across
both directions, four shares (light, medium, heavy, motorcycle), and the public provenance
code carried into the popup. Subdivision zero means outside the grid and is forbidden in a
count or vehicle-share row.

The Thai source reports annual vehicle-kilometres for 2,695 control sections, in ten vehicle
classes and motorcycles, with the province of each section. The supplied edition is labelled
2021; the catalogue later labelled it 2018, so 2021 is the conservative observation bound.
For each highway and province, divide annual vehicle-kilometres by 365 and the mapped highway
length there. Eligible mapped classes are motorway through tertiary and unclassified, using
the first numeric road reference; one-way carriageway length counts half. The denominator is
a dated input: rates must be refitted for a different mapped network.

A 3-MAD outlier rule rejects inconsistent control-section coverage. In the 2026 map, a whole
highway is rejected when vehicle-kilometres / the global major-road model's expectation lies
outside 0.23–26.9, or mapped km / control section outside 0.82–240. A province is rejected
when km/section or traffic/expectation differs from its highway's by more than factors 3.2 or
2.8, or when the department lists no section there. These are fitted screening thresholds,
not geographic exceptions. The resulting 1,675 highway/province rows cover 50,145 of the
53,510 mapped km with a department number. Against the separate 2025 survey of 61 stations
on 34 highways in Nakhon Sawan, the province means differ by 1.3 dB on average. That survey is
validation only, not a fitted input.

Light vehicles combine cars, vans, small buses and small trucks; medium combines medium buses
and trucks; heavy combines large buses, large trucks, trailers and semi-trailers. Shares sum
to one within the source's rounding precision. A road outside the measured coverage keeps
the ordinary counted-road or global-model path.

`subdivision-grids` rows have country, north, west and cell step in degrees, `row_starts`, and
`runs`. Rows go south, columns east. `row_starts` has one final sentinel; each run holds the
exclusive ending column and subdivision code. The Thai codes are ISO 3166-2:TH numbers; US
codes are FIPS. Grids were rasterised from administrative boundaries to avoid a polygon join
at every road segment. The boundary edition is separate from the observation year of counts.

## United States vehicle shares

`regional-vehicle-shares` rows hold country, subdivision, the mapped road-class indices, and
`[rural,urban][light,medium,heavy,motorcycle]` fractions. FHWA Highway Statistics VM-4 reports
annual vehicle distance by state and class. Passenger cars plus light trucks are light; buses
plus single-unit trucks are medium; combination trucks are heavy. Motorways use Interstate
shares, trunk through secondary use other arterials, and other roads use other-road shares.
Unknown surroundings use the arithmetic mean of rural and urban fractions. Outside a state's
grid coverage there is no state mix; the normal country/global vehicle model applies.

## Buildings, trips and activities

| Table | Units and derivation |
|---|---|
| building-plant | Heating and cooling outdoor units per household, central cooling fraction, optional fixed hours/day; EHPA, EurObserv'ER, RECS and national surveys |
| service-cooling | Fraction of service floor cooled; Heat Roadmap Europe D3.2 section 5.3, observed 2015 and published 2018 |
| dwelling-trips | Vehicle trips per dwelling per day; MiD 2017, National Travel Survey and NHTS 2022, including occupancy adjustment |
| hotel-trip-rate | Vehicle trips per 100 m² per day; mapped room density and ITE trip rates |
| building-road-models | Log-linear coefficients over building trip ends; distances in metres, traffic in vehicles/day, country shifts in dB; field names in `source_tables/models.rs` specify each fit |
| bus-models | Frequencies, exponents, minimum service and period fractions; fitted to timetable service and mapped routes |
| venue-seating | Expected and tagged seats by bar, pub, nightclub, restaurant, café and fast-food use; beer-garden median area in m²; predominant unmapped building use |
| venue-customs | Opening and terrace closing hours, with hours after midnight above 24; meal times and open-fronted practice |
| worship-calls | Prayer method, start offset and sounding duration in seconds, sound power dB(A), iqama/sala seconds, dawn/Friday/silent policy and horn height in metres |
| church-bells | Denomination default, clock events/hour, clock events/night and seconds/event |

Plants use their UN M49 regional medians where a country has no observation, then zero heating
units and 0.16 cooling units per household (the Brazil, Mexico and India surveys). South-East
Asia uses the regional survey's 0.29. Service cooling uses the European regional medians, then
HRE's 0.847 saturation from hot-climate US floor shares, also reached by Cyprus and Malta.
Country, regional and world values are distinct levels of the same table.

Venue seating uses Madrid and Melbourne inventories and mapped beer-garden areas. The choice
of restaurant hours for an unnamed hospitality building is in the table too: restaurants
were 69% of buildings with a named place in 227 squares counted on 19 October 2026. This
observation is a dependency, not a timeless compiled rule. Clock-event duration is measured
independently of clock-event frequency (25 seconds/hour divided by four measured events/hour).

Country practices and legal limits use their manifest validity intervals. A check in 2026
establishes validity at that check, not automatically in 2024; historical validity needs its
own documented input. Solar positions are analytic geometry sampled through a calendar cycle,
not future observations or movement counts. The annual cycle therefore differs deliberately
from the observed aircraft flow window.

The plant table contains 43 country rows and 11 regional rows. The service-cooling table contains
the 28 measured EU countries and four regional medians. Unmeasured countries use the region
directly; the same regional value is not copied into individual country rows. The regional
service medians are checked against their measured members, and all road period shares are
checked for non-negative values summing to one when loaded.

OSM preparation uses `prepare-geography.json` and, where admitted,
`prepare-osm-speeds.json` in the same year directories. Stage their bytes at the
manifest's `osm-geography-identities` and `osm-implicit-speeds` ids below the
external year source root; completion checks these published copies too.
The `osm-boundaries` id is the boundary edition's ADM0 FeatureCollection;
`boundaries` continues to identify the whole administrative edition.

The frozen factors and their observation vintages remain inputs, not a new fit to this port's
fixture squares.

`facility-model.json` selects publisher adapters and their geographic coverage, activity mappings,
source authority, name inferences and duplicate-site rules. `qm-build facility-observations`
normalizes each admitted `prepare-industrial-sites` input; `qm-build facilities` joins these
observations to `osm-geography` and writes the `facilities` input consumed by industry, leisure and
site outlines. Both Arrow stages use one schema, with WGS84 WKB geometry, stable publisher or OSM
identity, source, activity, capacity and its unit, and operating dates. A publisher without persistent
IDs gets a deterministic content identity; it cannot link a renamed or relocated record across
snapshots. Each raw observation has a separate, unique content identity, which the classification
and operation references name. Classification is mapped (0),
reported by a register (1), or inferred from a name (2); none is a sound measurement. Operation is
unknown (0), observed operating (1), or inactive (2). `classification_admitted` is separate: a register
can describe the sector without having observed operation. The [method](../../frontend/src/about/data-preparation.md)
describes temporal admission and conflicting evidence. Completion inventories and Arrow metadata
cover the selected dependency declarations, model bytes and normalized observation bytes; an
unrelated manifest entry does not invalidate facilities. A missing square is an error. Registered capacity is retained with the observation; the mapped
power capacity used by the existing emitter keeps its mapped value and unit. Wind rows remain, but
the obsolete parameter enrichment is omitted because the independent turbine input owns emission.
The facility builder reads one ring around `--squares` from `osm-geography`, but writes and lists
only the requested squares. For `sources`, prepare facilities for its targets and their two-square
halo: building sound uses the sites around neighbouring buildings. That requires three rings of
OSM geography around the source targets. Longitude wraps; latitude stops at the poles. The source
preflight rejects incomplete context before writing tiles. Empty prepared squares have explicit
empty Arrow files; a context-only square is not claimed as a completed output.

GPPD commissioning years are capacity-weighted means of generating-unit dates, not opening bounds; the converter leaves their operating intervals unknown. See [WRI’s calculation](https://github.com/wri/global-power-plant-database/blob/master/build_databases/build_database_USA.py#L178-L185).

Facility identity recipes name publisher business, unit or global keys. Export row counters (`OBJECTID`, `FID`) are rejected; a source without a persistent key uses the documented name/location fallback. Shared administrative docket numbers are not treated as plant identifiers. The observation input’s `depends_on` is the converter’s source list, and its register dependencies must equal the recipe list.

## Road preparation and historical fits

`road-conversion.json` selects publisher converters, exact objects and their schema
parameters. Every referenced raw or auxiliary input is a declared dependency of the
conversion input. Converters produce one Arrow observation contract: identity,
observed interval, location/reference/direction, annual/weekday/hourly/period basis,
individual optional vehicle counts, count scope, source priority and matching support.
Publisher column names and edition-specific objects occur in these dated recipes.
The Arrow metadata binds the observation file to the manifest's SHA-256.

`road-preparation-model.json` supplies physical carriageway priors, access factors,
building trip generation, local-street background and speed-transition parameters.
The preparation stage preserves the fallback columns consumed by the final road reader;
the reader's building, vehicle, speed and period enrichment still uses its admitted
model tables. The preparation demand calculation lives in the traffic module beside
that reader's routed building demand. A preparation run alone is not a complete map.

The historical calibration uses annual, measured, two-direction sections with a
reported sampling length. It fits length-weighted class medians; a missing or modeled
vehicle-class count cannot train that class's share. The admitted HPMS population
supplies the 2024 global and US section priors. Single-unit truck and bus AADT already
contains buses (FHWA classes 4–7); combination AADT covers classes 8–13.
`road-model-samples.json` records the support of every fitted cell.

Hourly fits require complete station-days, combine only disjoint intervals, and give
each supported station equal weight. The 2024 non-heavy profile uses TMAS all-vehicle
counts where no separate heavy series exists; Baden-Württemberg's classified hours
supply the heavy profiles. `road-weekday-factor.json` records the median ratio of
annual-daily to weekday-daily traffic and the contributing station-lane count.
These are the training populations, not a claim of representative counts everywhere.

`road-calibration.json` keeps the fit's support thresholds and its admitted trip/fleet
assumptions. Its preparation template has no later fitted priors; an unavailable street background is null and leaves the global class prior.
Service-road trip bounds are also absent in the historical template: all service roads retain the global class prior until an admitted service-demand model is available. The contemporary bounds are explicitly retained legacy assumptions.
The fitter writes the class and weekday values directly into the preparation model,
and obtains transition limits from the admitted legal-speed table.

`tagged-speeds` is fitted by the year run itself (its `tagged-speed-table` step, after
OSM geography and structures, before sources) from numeric limits in the manifest-selected
mapped OSM geometry. A mapped piece contributes one observation, classified by the same
settlement polygons and building-density model used by road preparation. A cell needs
at least 30 pieces; a selection whose roads fill no such cell has no fit and builds no sources.
The run publishes the fitted table and its support (`tagged-speeds.samples.json`) in the year
tree's `tables/`; the release commit copies the table here. A table fitted on a smaller selection is never pinned into a
later run.
Legal and heavy speed histories retain their source, applicability and validity
interval (exclusive end). A country lacking a documented historical row retains the
road model's global speed fallback; no later legal check is backdated. Japan's 2024
mixed lorry/bus cap retains 80 km/h to cover trailers; the rigid-lorry allowance of
90 km/h cannot be applied to that entire mixed class.
