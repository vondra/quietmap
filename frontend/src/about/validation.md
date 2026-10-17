---
title: Validation
intro: How close the model comes to real measurements, network by network.
---

Before a version goes live, the click is run at the microphones of the monitoring networks and
research campaigns below and compared with what they measured. A gap is first traced to the input
data, a justified difference in method or a model defect; only a defect leads to a fix. Official
strategic noise maps serve as cross-checks, never as calibration targets.

## Model minus measurement

The median over the measurements, and the difference half of them stay within, on the current
version (October 2026):

| Measurements | n | Median | Half within |
|---|---:|---:|---:|
| Paris near its airports, Bruitparif, Lden 2024 | 22 | −0.5 dB | 1.5 dB |
| Paris streets, Ville de Paris (ODbL) by Bruitparif, Lden 2025 | 16 | −3.2 dB | 3.2 dB |
| Barcelona, Ajuntament de Barcelona (CC BY 4.0), Lden 2025 | 154 | −1.6 dB | 4.2 dB |
| Madrid, Ayuntamiento de Madrid (CC BY 4.0), Lden 2025 | 31 | +2.4 dB | 3.4 dB |
| Dublin, Dublin City Council (CC BY), Lden 2025 | 14 | −4.4 dB | 5.5 dB |
| Poland's roads, Chief Inspectorate of Environmental Protection ([dane.gov.pl](https://dane.gov.pl/pl/dataset/1404), CC BY 4.0), 06–22 h, 2018–2024, at 5,351 points | 6,616 | +0.4 dB | 3.0 dB |
| German railways, Eisenbahn-Bundesamt, 06–22 and 22–06 h, 2023 | 19 | −0.4 and −3.2 dB | 1.7 and 3.3 dB |
| Prague airport, Letiště Praha, aircraft, 06–22 and 22–06 h, January 2026 | 14 and 13 | −0.3 and +1.0 dB | 1.8 and 2.0 dB |
| Minneapolis–St Paul airport, Metropolitan Airports Commission (MACNOMS), aircraft, DNL 2025–26 | 39 | +0.2 dB | 1.1 dB |
| Zurich airport, Flughafen Zürich, aircraft, 06–22 h, 2024 | 4 | −5.9 dB | 5.9 dB |
| Accra, Clark et al. 2021 ([Zenodo](https://zenodo.org/records/11223686), CC BY 4.0), 24 h, 2019–2020 | 146 | −9.6 dB | 9.6 dB |

The rail emission takes its typical train from the same German monitoring report, so that row is
not a fully independent test.

## Quiet places

Nearly all of these stand in loud places; the quiet places the map is for are hardly checked yet.
The only set so far, Silenzi in Quota's 73 recordings of a few daytime minutes in quiet mountains
([Zenodo](https://zenodo.org/records/20719737), CC BY 4.0), measures everything heard, nature
included, and reads 13.0 dB above the model, as it should; but at 14 of the points the model is
louder than everything measured, at 13 of them because of a road.
