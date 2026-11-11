//! A GTFS feed read for one week: its tables row by row from a folder or a zip, the bus trips
//! running on each day and the points of the shapes they follow.

use crate::rail_inputs::calendar::ServiceCalendar;
use anyhow::{Context, Result, anyhow, ensure};
use chrono::NaiveDate;
use csv::ByteRecord;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// GTFS route types that run buses: bus, trolleybus, the extended coach and bus families and
/// demand-responsive buses (the 2026 fit's `measure.py`).
fn runs_buses(route_type: u16) -> bool {
    matches!(route_type, 3 | 11 | 200..=209 | 700..=799 | 800)
}

/// The extended route types of coaches, the only GTFS mark of a coach line.
fn runs_coaches(route_type: u16) -> bool {
    (200..=209).contains(&route_type)
}

/// A GTFS feed: a folder of tables, or a zip holding them in any of its folders.
pub(super) enum Tables {
    Folder(PathBuf),
    Zip(PathBuf),
}

pub(super) struct Columns(HashMap<String, usize>);

impl Columns {
    pub fn index(&self, name: &str) -> Result<usize> {
        self.0
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("no column {name}"))
    }

    pub fn optional(&self, name: &str) -> Option<usize> {
        self.0.get(name).copied()
    }
}

pub(super) fn field(record: &ByteRecord, index: Option<usize>) -> &[u8] {
    index
        .and_then(|index| record.get(index))
        .unwrap_or(b"")
        .trim_ascii()
}

pub(super) fn number<T: std::str::FromStr>(record: &ByteRecord, index: usize) -> Option<T> {
    std::str::from_utf8(field(record, Some(index)))
        .ok()?
        .parse()
        .ok()
}

/// Visits every row of a table with the visitor `make` builds from its columns.
fn visit_rows<V: FnMut(&ByteRecord) -> Result<()>>(
    reader: impl Read,
    make: impl FnOnce(&Columns) -> Result<V>,
) -> Result<()> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(reader);
    let columns = Columns(
        reader
            .byte_headers()?
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let name = String::from_utf8_lossy(name);
                (
                    name.trim_start_matches('\u{feff}').trim().to_string(),
                    index,
                )
            })
            .collect(),
    );
    let mut visit = make(&columns)?;
    let mut record = ByteRecord::new();
    while reader.read_byte_record(&mut record)? {
        visit(&record)?;
    }
    Ok(())
}

/// The member of a zip whose file name is `name`, in whichever folder.
fn member(archive: &zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    archive
        .file_names()
        .find(|member| {
            Path::new(member)
                .file_name()
                .is_some_and(|file| file == name)
        })
        .map(str::to_string)
}

impl Tables {
    pub fn open(path: &Path) -> Self {
        if path.is_dir() {
            Tables::Folder(path.to_path_buf())
        } else {
            Tables::Zip(path.to_path_buf())
        }
    }

    /// Visits every row of the table `name`; false when the feed has none.
    pub fn rows<V: FnMut(&ByteRecord) -> Result<()>>(
        &self,
        name: &str,
        make: impl FnOnce(&Columns) -> Result<V>,
    ) -> Result<bool> {
        match self {
            Tables::Folder(folder) => {
                let path = folder.join(name);
                if !path.exists() {
                    return Ok(false);
                }
                visit_rows(std::fs::File::open(&path)?, make)
                    .with_context(|| path.display().to_string())?;
            }
            Tables::Zip(path) => {
                let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
                let Some(member) = member(&archive, name) else {
                    return Ok(false);
                };
                visit_rows(archive.by_name(&member)?, make)
                    .with_context(|| format!("{}: {member}", path.display()))?;
            }
        }
        Ok(true)
    }

    /// The services running on each of the seven days from `first_day`, read as the railway
    /// timetables read their calendars (the feed's declared window aside); a zip's calendar tables
    /// are read from a temporary copy.
    pub fn calendar(&self, first_day: NaiveDate) -> Result<ServiceCalendar> {
        let end = first_day
            .checked_add_days(chrono::Days::new(7))
            .ok_or_else(|| anyhow!("{first_day}: no week after it"))?;
        let read = |folder: &Path| {
            ServiceCalendar::read_selected(folder, first_day, end, None, true)
                .map_err(anyhow::Error::msg)
        };
        match self {
            Tables::Folder(folder) => read(folder),
            Tables::Zip(path) => {
                let copy = tempfile::tempdir()?;
                let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
                for name in ["calendar.txt", "calendar_dates.txt"] {
                    if let Some(member) = member(&archive, name) {
                        let mut target = std::fs::File::create(copy.path().join(name))?;
                        std::io::copy(&mut archive.by_name(&member)?, &mut target)?;
                    }
                }
                read(copy.path())
            }
        }
    }
}

/// A bus trip running in the week: its days and, for a coach, its route direction.
pub(super) struct Trip {
    pub days: u8,
    pub coach_direction: Option<usize>,
}

/// The bus trips running in the week, their trips a shape, the coach route directions and the
/// trips a day.
pub(super) struct Trips {
    pub trips: HashMap<Vec<u8>, Trip>,
    pub by_shape: HashMap<Vec<u8>, f64>,
    pub coach_directions: usize,
    pub by_day: [f64; 7],
    pub without_shape: f64,
}

pub(super) fn read_trips(tables: &Tables, calendar: &ServiceCalendar) -> Result<Trips> {
    let mut route_types = HashMap::<Vec<u8>, u16>::new();
    let types = &mut route_types;
    let found = tables.rows("routes.txt", |columns| {
        let (route, kind) = (columns.index("route_id")?, columns.index("route_type")?);
        Ok(move |row: &ByteRecord| {
            if let Some(kind) = number::<u16>(row, kind).filter(|&kind| runs_buses(kind)) {
                types.insert(field(row, Some(route)).to_vec(), kind);
            }
            Ok(())
        })
    })?;
    ensure!(found, "no routes.txt");
    let mut trips = Trips {
        trips: HashMap::new(),
        by_shape: HashMap::new(),
        coach_directions: 0,
        by_day: [0.0; 7],
        without_shape: 0.0,
    };
    let mut coach_directions = HashMap::<(Vec<u8>, Vec<u8>), usize>::new();
    let (state, directions, types) = (&mut trips, &mut coach_directions, &route_types);
    tables.rows("trips.txt", |columns| {
        let (route, service, trip) = (
            columns.index("route_id")?,
            columns.index("service_id")?,
            columns.index("trip_id")?,
        );
        let (shape, direction) = (
            columns.optional("shape_id"),
            columns.optional("direction_id"),
        );
        Ok(move |row: &ByteRecord| {
            let route = field(row, Some(route));
            let Some(&kind) = types.get(route) else {
                return Ok(());
            };
            let service = String::from_utf8_lossy(field(row, Some(service)));
            let Some(days) = calendar.active.get(service.as_ref()) else {
                return Ok(());
            };
            let running = days.iter().filter(|&&runs| runs).count();
            if running == 0 {
                return Ok(());
            }
            for (day, &runs) in days.iter().enumerate() {
                state.by_day[day] += f64::from(u8::from(runs));
            }
            match field(row, shape) {
                b"" => state.without_shape += running as f64,
                shape => *state.by_shape.entry(shape.to_vec()).or_default() += running as f64,
            }
            let coach_direction = runs_coaches(kind).then(|| {
                let next = directions.len();
                let key = (route.to_vec(), field(row, direction).to_vec());
                *directions.entry(key).or_insert(next)
            });
            let trip = field(row, Some(trip)).to_vec();
            state.trips.insert(
                trip,
                Trip {
                    days: running as u8,
                    coach_direction,
                },
            );
            Ok(())
        })
    })?;
    trips.coach_directions = coach_directions.len();
    Ok(trips)
}

/// A shape's identifier and its points (latitude, longitude) in sequence order.
pub(super) type ShapePoints = (Vec<u8>, Vec<[f64; 2]>);

/// The points of the shapes `wanted`, each in its sequence order.
pub(super) fn read_shapes(
    tables: &Tables,
    wanted: &HashMap<Vec<u8>, f64>,
) -> Result<Vec<ShapePoints>> {
    let mut points = HashMap::<Vec<u8>, Vec<(u32, [f64; 2])>>::new();
    let found = &mut points;
    tables.rows("shapes.txt", |columns| {
        let (shape, sequence) = (
            columns.index("shape_id")?,
            columns.index("shape_pt_sequence")?,
        );
        let (lat, lon) = (
            columns.index("shape_pt_lat")?,
            columns.index("shape_pt_lon")?,
        );
        Ok(move |row: &ByteRecord| {
            let shape = field(row, Some(shape));
            if !wanted.contains_key(shape) {
                return Ok(());
            }
            let (Some(sequence), Some(lat), Some(lon)) =
                (number(row, sequence), number(row, lat), number(row, lon))
            else {
                return Err(anyhow!(
                    "shape {}: bad point",
                    String::from_utf8_lossy(shape)
                ));
            };
            found
                .entry(shape.to_vec())
                .or_default()
                .push((sequence, [lat, lon]));
            Ok(())
        })
    })?;
    // In identifier order, so that a refit of the same feed gives the same shapes.
    let mut shapes: Vec<_> = points.into_iter().collect();
    shapes.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(shapes
        .into_iter()
        .map(|(id, mut vertices)| {
            vertices.sort_by_key(|&(sequence, _)| sequence);
            (id, vertices.into_iter().map(|(_, point)| point).collect())
        })
        .collect())
}
