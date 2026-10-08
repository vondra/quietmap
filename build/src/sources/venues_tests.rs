use super::*;

fn iso(code: &[u8; 2]) -> u16 {
    u16::from_le_bytes(*code)
}

fn open_hours(week: &WeekHours, day: usize) -> Vec<usize> {
    (0..24).filter(|&hour| week[day][hour]).collect()
}

#[test]
fn places_are_read_and_filed_by_square() {
    let venues = Venues::parse(
        "41.38\t2.17\tbar\tyes\t0\tMo-Su 18:00-02:00\tBar Marsella\n\
         41.3801\t2.1701\tfood_court\tno\t300\t\tMercat\n\
         41.3802\t2.1702\tbiergarten\tunknown\t400\t\t\n",
    )
    .unwrap();
    let (gx, gy) = degrees_to_z30(41.38, 2.17);
    let square = crate::dev4::Square::of_z30(gx, gy);
    let barcelona = venues.in_square(square.x, square.y);
    assert_eq!(barcelona.len(), 2, "food courts are indoors");
    assert_eq!(barcelona[0].kind, VenueKind::Bar);
    assert_eq!(barcelona[0].seating, Seating::Yes);
    assert_eq!(
        open_hours(&barcelona[0].hours.unwrap(), 2),
        [0, 1, 18, 19, 20, 21, 22, 23]
    );
    assert_eq!(seats(&barcelona[0]), 28.0);
    assert!(
        (seats(&barcelona[1]) - 280.0).abs() < 1e-9,
        "a beer garden of 400 m2"
    );
    assert!(Venues::parse("1\t2\tbar\n").is_err());
}

/// An untagged Barcelona bar opens at noon and closes at 02:00 (03:00 on Friday and Saturday
/// nights), its terrace at midnight (01:00); a London pub closes at 23:00 (midnight at weekends),
/// its garden at 23:00; a Thai bar keeps its open front as long as it is open.
#[test]
fn places_without_hours_keep_their_countrys_customs() {
    let bar = |kind| VenueSite {
        lat: 0.0,
        lon: 0.0,
        kind,
        seating: Seating::Unknown,
        area_m2: 0.0,
        hours: None,
        name: String::new(),
    };
    let (open, terrace) = hours(&bar(VenueKind::Bar), &customs(iso(b"ES")));
    assert_eq!(
        open_hours(&open, 1),
        [0, 1, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23]
    );
    assert_eq!(
        open_hours(&open, 5),
        [0, 1, 2, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23]
    );
    assert_eq!(open_hours(&terrace, 1)[..2], [12, 13]);
    assert!(
        !terrace[1][0] && terrace[5][0] && !terrace[5][1],
        "the terrace closes at 24/01 h"
    );
    let (pub_open, garden) = hours(&bar(VenueKind::Pub), &customs(iso(b"GB")));
    assert_eq!(*open_hours(&pub_open, 2).last().unwrap(), 22);
    assert!(pub_open[5][23] && !garden[5][23]);
    let (thai, front) = hours(&bar(VenueKind::Bar), &customs(iso(b"TH")));
    assert_eq!(thai, front);
    assert_eq!(seats(&bar(VenueKind::Bar)), 13.5);
}

/// A bar mapped as a node and as its 200 m2 outline 3 m away is one place, the outline's with the
/// node's terrace, hours and name; a bar 40 m away, a restaurant on the same outline and a bar of
/// another name on a named outline stay apart.
#[test]
fn a_node_on_its_outline_is_one_place() {
    let north = |metres: f64| 50.0 + metres / 111_195.0;
    let venues = Venues::parse(&format!(
        "50\t14\tbar\tunknown\t200\t\t\n\
         {}\t14\tbar\tyes\t0\tMo-Su 18:00-02:00\tU Tygra\n\
         {}\t14\tbar\tno\t0\t\tNext Door\n\
         {}\t14\trestaurant\tunknown\t0\t\t\n\
         {}\t14\tbar\tunknown\t100\t\tU Zlateho Tygra\n\
         {}\t14\tbar\tunknown\t0\t\tVedle\n",
        north(3.0),
        north(40.0),
        north(3.0),
        north(1_000.0),
        north(1_004.0),
    ))
    .unwrap();
    let (gx, gy) = degrees_to_z30(50.0, 14.0);
    let square = crate::dev4::Square::of_z30(gx, gy);
    let places = venues.in_square(square.x, square.y);
    assert_eq!(places.len(), 5, "{places:?}");
    let bar = places.iter().find(|p| p.area_m2 == 200.0).unwrap();
    assert_eq!((bar.lat, bar.seating), (50.0, Seating::Yes));
    assert_eq!(bar.name, "U Tygra");
    assert!(bar.hours.is_some());
    assert!(places.iter().any(|p| p.name == "Next Door"));
    assert!(places.iter().any(|p| p.kind == VenueKind::Restaurant));
    assert!(
        places.iter().any(|p| p.name == "Vedle"),
        "another name is another place"
    );
}
