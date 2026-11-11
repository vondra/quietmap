//! A synthetic timetable week, as a zip with a folder and as a folder: calendars with exceptions,
//! bus, coach and tram route types, shapes near the place, stop calls by period inside it, and a
//! coach route direction's trips a day.
use super::*;
use std::io::Write;

const TABLES: [(&str, &str); 7] = [
    (
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
         week,1,1,1,1,1,0,0,20240101,20241231\nsun,0,0,0,0,0,0,1,20240101,20241231\n",
    ),
    // No service on Wednesday 18 September; an extra one on Saturday 21.
    (
        "calendar_dates.txt",
        "service_id,date,exception_type\nweek,20240918,2\nextra,20240921,1\n",
    ),
    (
        "routes.txt",
        "route_id,route_type\nbus,3\ncoach,202\ntram,0\n",
    ),
    (
        "trips.txt",
        "route_id,service_id,trip_id,shape_id,direction_id\n\
         bus,week,t1,s1,0\nbus,sun,t2,s1,1\ncoach,week,t3,s2,0\ncoach,week,t4,s2,1\n\
         tram,week,t5,s3,0\nbus,extra,t6,,0\ncoach,sun,t7,s2,0\n",
    ),
    (
        "shapes.txt",
        "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n\
         s1,50.001,14.002,2\ns1,50.001,14.0,1\ns2,50.0,14.0,1\ns2,50.0,14.1,2\n\
         s3,50.0,14.0,1\ns3,50.0,14.1,2\ns4,51.0,14.0,1\n",
    ),
    (
        "stops.txt",
        "stop_id,stop_lat,stop_lon\nA,50.0,14.0\nB,51.0,15.0\n",
    ),
    // Day, evening, night and past midnight; the tram and the stop outside do not count.
    (
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         t1,08:00:00,08:00:00,A,1\nt1,08:10:00,08:10:00,B,2\nt2,20:30:00,20:30:00,A,1\n\
         t3,23:30:00,23:30:00,A,1\nt4,25:10:00,,A,1\nt5,09:00:00,09:00:00,A,1\n\
         t6,07:00:00,07:00:00,A,1\nt7,10:00:00,10:00:00,B,1\n",
    ),
];

fn place() -> Place {
    Place {
        name: "fixture".into(),
        bounds: [49.99, 13.99, 50.01, 14.01],
        first_day: NaiveDate::from_ymd_opt(2024, 9, 16).unwrap(),
        timetables: vec!["feed".into()],
    }
}

fn check(week: &Week) {
    // Monday to Sunday: t1, t3 and t4 on the four weekdays left, t6 on Saturday, t2 and t7 Sunday.
    assert_eq!(week.bus_trips_by_day, [3.0, 3.0, 0.0, 3.0, 3.0, 1.0, 2.0]);
    assert_eq!(week.bus_trips_without_shape, 1.0);
    let trips: Vec<f64> = week.shapes.iter().map(|shape| shape.trips).collect();
    assert_eq!(trips, [5.0, 9.0]);
    assert_eq!(week.shapes[0].points, [[50.001, 14.0], [50.001, 14.002]]);
    assert_eq!(week.places[0].shapes, [0, 1]);
    assert_eq!(week.places[0].stop_calls, [5.0, 1.0, 8.0]);
    // Each coach direction calling inside the place on four days; t7 calls only outside.
    assert_eq!(week.coach_departures, [4.0 / 7.0, 4.0 / 7.0]);
}

#[test]
fn a_week_counts_running_bus_trips_their_shapes_stop_calls_by_period_and_coach_directions() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let archive = root.path().join("feed.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in TABLES {
        std::fs::write(folder.join(name), text).unwrap();
        zip.start_file(format!("gtfs/{name}"), options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let place = place();
    for path in [&archive, &folder] {
        check(&read_week(path, place.first_day, &[&place]).unwrap());
    }
}

#[test]
fn the_inventory_lists_each_place_its_bounds_week_and_timetables() {
    let text = "place\tsouth\twest\tnorth\teast\tfirst_day\ttimetables\n\
                fixture\t49.99\t13.99\t50.01\t14.01\t2024-09-16\tfeed\n";
    assert_eq!(parse_places(text).unwrap(), [place()]);
    assert!(place().contains(50.0, 14.0) && !place().contains(50.02, 14.0));
    assert!(parse_places("place\tbounds\n").is_err());
    assert!(parse_places(&text.replace("49.99", "50.02")).is_err());
}
