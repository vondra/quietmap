"""Outdoor seats of each surveyed venue (`qm-build venue-seating-table` reads them), one line each:
`kind seats` (tab-separated, sorted), the kind an OpenStreetMap amenity (bar, pub, nightclub,
restaurant, cafe, fast_food) and seats its authorised or counted outdoor seats, 0 for a venue
without any. Two publishers, each for the kinds it describes as OpenStreetMap maps them:

- Madrid's census of premises and terraces (`censo de locales`, an edition's `locales.csv`,
  `actividades.csv` and `terrazas.csv`): every open premise of a hospitality activity, its kind by
  the first of nightclub, late bar, bar, restaurant, fast food and cafe among its activity codes
  (a late bar has no OpenStreetMap kind and is left out), its seats the seasonal chairs of its open
  terraces.
- Melbourne's Census of Land Use and Employment (`cafes-and-restaurants-with-seating-capacity`):
  the pubs, taverns and bars of one census year, a venue its property, trading name and business
  address, its seats its outdoor seats. Madrid's census has no pub; its bars are OpenStreetMap's.

    terrace_surveys.py MADRID_EDITION_DIR MELBOURNE_SEATS_CSV CENSUS_YEAR OUT
"""
import csv
import sys

# Madrid's activity codes (epigrafes) by kind, in the order a premise with several takes them.
MADRID_KINDS = (
    ('nightclub', {'932004', '932005', '932006'}),
    (None, {'563002', '563003', '563007'}),
    ('bar', {'561004', '561005', '563001', '563004', '563005'}),
    ('restaurant', {'561001'}),
    ('fast_food', {'561002'}),
    ('cafe', {'561006', '561007'}),
)
MELBOURNE_PUBS = 'Pubs, Taverns and Bars'


def rows(path, delimiter=';'):
    """A publisher's CSV rows as dicts, its byte-order mark and the cells' padding removed."""
    with open(path, encoding='utf-8-sig', newline='') as file:
        for row in csv.DictReader(file, delimiter=delimiter):
            yield {key.strip(): (value or '').strip() for key, value in row.items()}


def number(text):
    """A count as published (a decimal comma allowed); 0 when empty."""
    return float(text.replace(',', '.')) if text else 0.0


def madrid(edition):
    """(kind, seats) of every open hospitality premise of a census edition directory."""
    open_premises = {row['id_local'] for row in rows(f'{edition}/locales.csv')
                     if row['desc_situacion_local'] == 'Abierto'}
    codes = {}
    for row in rows(f'{edition}/actividades.csv'):
        codes.setdefault(row['id_local'], set()).add(row['id_epigrafe'])
    chairs = {}
    for row in rows(f'{edition}/terrazas.csv'):
        if row['desc_situacion_terraza'] == 'Abierta':
            chairs[row['id_local']] = chairs.get(row['id_local'], 0.0) + number(row['sillas_es'])
    venues = []
    for premise in sorted(open_premises & codes.keys()):
        kind = next((kind for kind, wanted in MADRID_KINDS if codes[premise] & wanted), False)
        if kind:
            venues.append((kind, chairs.get(premise, 0.0)))
    return venues


def melbourne(path, census_year):
    """(kind, seats) of every pub, tavern and bar with seats in one CLUE census year."""
    seats = {}
    for row in rows(path):
        if row['census_year'] != census_year or row['industry_anzsic4_description'] != MELBOURNE_PUBS:
            continue
        venue = (row['property_id'], row['trading_name'], row['business_address'])
        outdoor = number(row['number_of_seats']) if row['seating_type'] == 'Seats - Outdoor' else 0.0
        seats[venue] = seats.get(venue, 0.0) + outdoor
    return [('pub', seats[venue]) for venue in sorted(seats)]


def write(venues, out_path):
    with open(out_path, 'w') as out:
        for kind, seats in sorted(venues):
            out.write(f'{kind}\t{seats:g}\n')
    print(len(venues), 'venues,', sum(1 for _, seats in venues if seats > 0), 'with seats',
          file=sys.stderr)


def main(arguments):
    if len(arguments) != 4:
        sys.exit(__doc__)
    edition, seats, census_year, out_path = arguments
    write(madrid(edition) + melbourne(seats, census_year), out_path)


if __name__ == '__main__':
    main(sys.argv[1:])
