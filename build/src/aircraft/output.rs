//! The day's scratch files: `segments/<day>.arrow` (dev4's v16 columns plus the altitudes above
//! EGM2008) and `flights/<day>.arrow` ([`super::flight_table`]), zstd Arrow IPC written in batches
//! and renamed into place when complete.

use super::flight_table::{FlightColumns, FlightRow, flights_schema};
use super::segments::Segment;
use arrow_array::builder::{
    FixedSizeBinaryBuilder, Float32Builder, Int16Builder, StringBuilder, UInt8Builder,
    UInt64Builder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_ipc::CompressionType;
use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use arrow_schema::{DataType, Field, Schema};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Rows per record batch: a reader holds one batch (about 100 MB decoded) at a time.
const BATCH_ROWS: usize = 1 << 20;

const SEGMENT_U8_COLUMNS: [&str; 6] = [
    "profile_idx",
    "source_id",
    "origin",
    "veh_kind",
    "gse_class",
    "period",
];
const SEGMENT_F32_COLUMNS: [&str; 14] = [
    "start_lat",
    "start_lon",
    "start_alt_m",
    "end_lat",
    "end_lon",
    "end_alt_m",
    "speed_kt",
    "length_m",
    "agl_avg_m",
    "start_elev_m",
    "end_elev_m",
    "departure_field_elev_m",
    "start_alt_msl_m",
    "end_alt_msl_m",
];

pub fn field(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, false)
}

pub fn segments_schema(metadata: HashMap<String, String>) -> Schema {
    let mut fields = vec![
        field("flight_id", DataType::UInt64),
        field("callsign", DataType::Utf8),
        field("aircraft_type", DataType::FixedSizeBinary(4)),
    ];
    fields.extend(
        SEGMENT_U8_COLUMNS
            .iter()
            .map(|name| field(name, DataType::UInt8)),
    );
    fields.push(field("date_id", DataType::Int16));
    fields.push(field("phase", DataType::UInt8));
    fields.push(field("flags", DataType::UInt8));
    fields.extend(
        SEGMENT_F32_COLUMNS
            .iter()
            .map(|name| field(name, DataType::Float32)),
    );
    Schema::new(fields).with_metadata(metadata)
}

/// A file written batch by batch under a temporary name, renamed into place by `finish`.
struct BatchFile {
    writer: FileWriter<BufWriter<File>>,
    schema: Arc<Schema>,
    temporary: PathBuf,
    path: PathBuf,
}

impl BatchFile {
    fn create(path: &Path, schema: Schema) -> Result<Self, String> {
        let temporary = path.with_extension("arrow.partial");
        let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", temporary.display());
        let file = File::create(&temporary).map_err(|e| failed(&e))?;
        let options = IpcWriteOptions::default()
            .try_with_compression(Some(CompressionType::ZSTD))
            .map_err(|e| failed(&e))?;
        let writer = FileWriter::try_new_with_options(
            BufWriter::with_capacity(1 << 22, file),
            &schema,
            options,
        )
        .map_err(|e| failed(&e))?;
        Ok(BatchFile {
            writer,
            schema: Arc::new(schema),
            temporary,
            path: path.to_path_buf(),
        })
    }

    fn write(&mut self, columns: Vec<ArrayRef>) -> Result<(), String> {
        let batch =
            RecordBatch::try_new(self.schema.clone(), columns).map_err(|e| e.to_string())?;
        self.writer
            .write(&batch)
            .map_err(|e| format!("{}: {e}", self.temporary.display()))
    }

    fn finish(mut self) -> Result<(), String> {
        let failed =
            |error: &dyn std::fmt::Display| format!("{}: {error}", self.temporary.display());
        self.writer.finish().map_err(|e| failed(&e))?;
        let buffered = self.writer.into_inner().map_err(|e| failed(&e))?;
        let file = buffered.into_inner().map_err(|e| failed(&e))?;
        file.sync_all().map_err(|e| failed(&e))?;
        std::fs::rename(&self.temporary, &self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

pub fn designators(values: &[[u8; 4]]) -> Result<ArrayRef, String> {
    let mut builder = FixedSizeBinaryBuilder::with_capacity(values.len(), 4);
    for value in values {
        builder.append_value(value).map_err(|e| e.to_string())?;
    }
    Ok(Arc::new(builder.finish()))
}

pub fn finished<B: arrow_array::builder::ArrayBuilder>(
    builders: &mut [B],
) -> impl Iterator<Item = ArrayRef> + '_ {
    builders.iter_mut().map(|builder| builder.finish())
}

#[derive(Default)]
struct SegmentColumns {
    rows: usize,
    flight_id: UInt64Builder,
    callsign: StringBuilder,
    aircraft_type: Vec<[u8; 4]>,
    bytes: [UInt8Builder; 6],
    date_id: Int16Builder,
    phase: UInt8Builder,
    flags: UInt8Builder,
    floats: [Float32Builder; 14],
}

/// Both files of one day.
pub struct DayWriter {
    segment_file: BatchFile,
    flight_file: BatchFile,
    segments: SegmentColumns,
    flights: FlightColumns,
    pub segment_rows: u64,
    pub flight_rows: u64,
}

impl DayWriter {
    /// Both files; `metadata` (day, scope) goes into both, each with its own `kind`.
    pub fn create(
        segments: &Path,
        flights: &Path,
        metadata: HashMap<String, String>,
    ) -> Result<Self, String> {
        let kind = |kind: &str| {
            let mut metadata = metadata.clone();
            metadata.insert("kind".into(), kind.into());
            metadata
        };
        Ok(DayWriter {
            segment_file: BatchFile::create(segments, segments_schema(kind("aircraft-segments")))?,
            flight_file: BatchFile::create(flights, flights_schema(kind("aircraft-flights")))?,
            segments: SegmentColumns::default(),
            flights: FlightColumns::default(),
            segment_rows: 0,
            flight_rows: 0,
        })
    }

    /// One flight's table row and segments; `date_id` is the day's.
    pub fn push(
        &mut self,
        row: &FlightRow,
        segments: &[Segment],
        date_id: i16,
    ) -> Result<(), String> {
        let s = &mut self.segments;
        for segment in segments {
            s.flight_id.append_value(row.flight_id);
            s.callsign.append_value(&row.callsign);
            s.aircraft_type.push(row.aircraft_type);
            let bytes = [
                row.profile,
                row.source_id,
                0,
                row.vehicle_kind,
                row.ground_vehicle_class,
                segment.period,
            ];
            for (builder, value) in s.bytes.iter_mut().zip(bytes) {
                builder.append_value(value);
            }
            s.date_id.append_value(date_id);
            s.phase.append_value(segment.phase as u8);
            s.flags.append_value(segment.flags);
            let floats = [
                segment.start_lat,
                segment.start_lon,
                segment.start_barometric_m,
                segment.end_lat,
                segment.end_lon,
                segment.end_barometric_m,
                segment.speed_kt,
                segment.length_m,
                segment.mean_height_m,
                segment.start_terrain_m,
                segment.end_terrain_m,
                row.departure_field_m,
                segment.start_altitude_m,
                segment.end_altitude_m,
            ];
            for (builder, value) in s.floats.iter_mut().zip(floats) {
                builder.append_value(value);
            }
        }
        s.rows += segments.len();
        self.segment_rows += segments.len() as u64;
        self.flights.push(row);
        self.flight_rows += 1;
        if self.segments.rows >= BATCH_ROWS {
            self.flush_segments()?;
        }
        if self.flights.rows() >= BATCH_ROWS {
            self.flush_flights()?;
        }
        Ok(())
    }

    fn flush_segments(&mut self) -> Result<(), String> {
        let mut s = std::mem::take(&mut self.segments);
        if s.rows == 0 {
            return Ok(());
        }
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(s.flight_id.finish()),
            Arc::new(s.callsign.finish()),
            designators(&s.aircraft_type)?,
        ];
        columns.extend(finished(&mut s.bytes));
        columns.push(Arc::new(s.date_id.finish()));
        columns.push(Arc::new(s.phase.finish()));
        columns.push(Arc::new(s.flags.finish()));
        columns.extend(finished(&mut s.floats));
        self.segment_file.write(columns)
    }

    fn flush_flights(&mut self) -> Result<(), String> {
        if self.flights.rows() == 0 {
            return Ok(());
        }
        let columns = std::mem::take(&mut self.flights).finish()?;
        self.flight_file.write(columns)
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.flush_segments()?;
        self.flush_flights()?;
        self.segment_file.finish()?;
        self.flight_file.finish()
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
